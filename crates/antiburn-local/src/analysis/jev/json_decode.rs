//! Bounded duplicate-key checks and independent provider usage extraction.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fmt;

use serde::de::{self, DeserializeOwned, DeserializeSeed, MapAccess, SeqAccess, Visitor};

use super::{
    JevError, JevResponseObservation, JevUsage, MAX_RESPONSE_BYTES, NATIVE_MODEL_ALIAS,
    valid_response_model,
};

// Count the root as depth one and bound untyped envelope metadata as well.
const MAX_JSON_DEPTH: usize = 64;
const MAX_JSON_NODES: usize = 16_384;

#[derive(Clone, Copy)]
pub enum JevUsageLocation {
    Direct,
    CloudflareEnvelope,
}

/// Records unambiguous usage before rejecting duplicate keys or invalid answers.
pub fn decode_jev_json_with_usage<T: DeserializeOwned>(
    body: &[u8],
    location: JevUsageLocation,
    record_usage: impl Fn(JevUsage) -> Result<(), JevError>,
) -> Result<T, JevError> {
    decode_jev_json_with_observation(body, location, |observation| {
        record_usage(observation.usage)
    })
}

/// Record usage and safe served identity before typed answer validation.
/// The first pass retains keys and one model name, not a second value tree.
pub fn decode_jev_json_with_observation<T: DeserializeOwned>(
    body: &[u8],
    location: JevUsageLocation,
    record_observation: impl Fn(JevResponseObservation) -> Result<(), JevError>,
) -> Result<T, JevError> {
    if body.len() > MAX_RESPONSE_BYTES {
        return Err(JevError::ResponseTooLarge);
    }
    let mut state = ScanState::default();
    let mut decoder = serde_json::Deserializer::from_slice(body);
    let scanned = ScanSeed {
        state: &mut state,
        depth: 1,
        role: match location {
            JevUsageLocation::Direct => Role::Response,
            JevUsageLocation::CloudflareEnvelope => Role::Envelope,
        },
    }
    .deserialize(&mut decoder)
    .map_err(|_| JevError::ResponseDecode)?;
    decoder.end().map_err(|_| JevError::ResponseDecode)?;
    if let Some(usage) = scanned.usage {
        record_observation(JevResponseObservation {
            usage,
            served_model: scanned.model,
        })?;
    }
    if state.duplicate {
        return Err(JevError::ResponseDecode);
    }
    serde_json::from_slice(body).map_err(|_| JevError::ResponseDecode)
}

#[derive(Default)]
struct ScanState {
    nodes: usize,
    duplicate: bool,
}

#[derive(Clone, Copy)]
enum Role {
    Envelope,
    Response,
    Model,
    Usage,
    Token,
    Other,
}

#[derive(Default)]
struct ScannedValue {
    usage: Option<JevUsage>,
    token: Option<u64>,
    model: Option<String>,
}

struct ScanSeed<'a> {
    state: &'a mut ScanState,
    depth: usize,
    role: Role,
}

impl<'de> DeserializeSeed<'de> for ScanSeed<'_> {
    type Value = ScannedValue;

    fn deserialize<D: de::Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        if self.depth > MAX_JSON_DEPTH || self.state.nodes >= MAX_JSON_NODES {
            return Err(de::Error::custom("response JSON limit exceeded"));
        }
        self.state.nodes += 1;
        decoder.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for ScanSeed<'_> {
    type Value = ScannedValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded response JSON")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut keys = BTreeSet::new();
        let mut usage = None;
        let mut model = None;
        let mut input_tokens = None;
        let mut output_tokens = None;
        let mut ambiguous_usage = false;
        let mut ambiguous_model = false;
        while let Some(key) = map.next_key_seed(KeySeed)? {
            let duplicate = keys.contains(&key);
            if duplicate {
                self.state.duplicate = true;
                ambiguous_usage |= match self.role {
                    Role::Envelope | Role::Usage => true,
                    Role::Response => key == "usage",
                    _ => false,
                };
                ambiguous_model |= matches!(self.role, Role::Envelope)
                    || matches!(self.role, Role::Response) && key == "model";
            }
            let role = match (self.role, key.as_ref()) {
                (Role::Envelope, "result") => Role::Response,
                (Role::Response, "usage") => Role::Usage,
                (Role::Response, "model") => Role::Model,
                (Role::Usage, "input_tokens" | "output_tokens") => Role::Token,
                _ => Role::Other,
            };
            let child = map.next_value_seed(ScanSeed {
                state: self.state,
                depth: self.depth + 1,
                role,
            })?;
            if !duplicate {
                match (self.role, key.as_ref()) {
                    (Role::Envelope, "result") => {
                        usage = child.usage;
                        model = child.model;
                    }
                    (Role::Response, "usage") => usage = child.usage,
                    (Role::Response, "model") => model = child.model,
                    (Role::Usage, "input_tokens") => input_tokens = child.token,
                    (Role::Usage, "output_tokens") => output_tokens = child.token,
                    _ => {}
                }
            }
            keys.insert(key);
        }
        if matches!(self.role, Role::Usage) {
            usage = input_tokens
                .zip(output_tokens)
                .map(|(input_tokens, output_tokens)| JevUsage {
                    input_tokens,
                    output_tokens,
                });
        }
        Ok(ScannedValue {
            usage: if ambiguous_usage { None } else { usage },
            token: None,
            model: if ambiguous_model { None } else { model },
        })
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
        while sequence
            .next_element_seed(ScanSeed {
                state: self.state,
                depth: self.depth + 1,
                role: Role::Other,
            })?
            .is_some()
        {}
        Ok(ScannedValue::default())
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
        Ok(ScannedValue {
            token: matches!(self.role, Role::Token).then_some(value),
            ..ScannedValue::default()
        })
    }

    fn visit_i64<E: de::Error>(self, _value: i64) -> Result<Self::Value, E> {
        Ok(ScannedValue::default())
    }

    fn visit_f64<E: de::Error>(self, _value: f64) -> Result<Self::Value, E> {
        Ok(ScannedValue::default())
    }

    fn visit_bool<E: de::Error>(self, _value: bool) -> Result<Self::Value, E> {
        Ok(ScannedValue::default())
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        Ok(ScannedValue {
            model: (matches!(self.role, Role::Model)
                && value != NATIVE_MODEL_ALIAS
                && valid_response_model(value))
            .then(|| value.to_owned()),
            ..ScannedValue::default()
        })
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(ScannedValue::default())
    }
}

struct KeySeed;

impl<'de> DeserializeSeed<'de> for KeySeed {
    type Value = Cow<'de, str>;

    fn deserialize<D: de::Deserializer<'de>>(self, decoder: D) -> Result<Self::Value, D::Error> {
        decoder.deserialize_str(self)
    }
}

impl<'de> Visitor<'de> for KeySeed {
    type Value = Cow<'de, str>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an object key")
    }

    fn visit_borrowed_str<E: de::Error>(self, value: &'de str) -> Result<Self::Value, E> {
        Ok(Cow::Borrowed(value))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        Ok(Cow::Owned(value.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use serde_json::Value;

    fn decode(
        body: &str,
        location: JevUsageLocation,
        expected: Option<JevUsage>,
    ) -> Result<Value, JevError> {
        let calls = std::cell::Cell::new(0);
        let result = decode_jev_json_with_usage(body.as_bytes(), location, |usage| {
            calls.set(calls.get() + 1);
            assert_eq!(Some(usage), expected);
            Ok(())
        });
        assert_eq!(calls.get(), usize::from(expected.is_some()));
        result
    }

    const USAGE: &str = r#"{"input_tokens":12,"output_tokens":1}"#;
    const KNOWN: Option<JevUsage> = Some(JevUsage {
        input_tokens: 12,
        output_tokens: 1,
    });

    fn decode_observed_response(
        response: &str,
        expected_model: Option<&str>,
        expect_success: bool,
    ) {
        #[derive(Deserialize)]
        struct Envelope {
            result: super::super::JevResponse,
        }

        for envelope in [false, true] {
            let body = if envelope {
                format!(
                    r#"{{"success":true,"model":"ignored-envelope-model","result":{response}}}"#
                )
            } else {
                response.to_owned()
            };
            let calls = std::cell::Cell::new(0);
            let record = |observation: JevResponseObservation| {
                calls.set(calls.get() + 1);
                assert_eq!(Some(observation.usage), KNOWN);
                assert_eq!(observation.served_model.as_deref(), expected_model);
                Ok(())
            };
            let result = if envelope {
                decode_jev_json_with_observation::<Envelope>(
                    body.as_bytes(),
                    JevUsageLocation::CloudflareEnvelope,
                    record,
                )
                .map(|envelope| envelope.result)
            } else {
                decode_jev_json_with_observation::<super::super::JevResponse>(
                    body.as_bytes(),
                    JevUsageLocation::Direct,
                    record,
                )
            };
            assert_eq!(calls.get(), 1);
            if expect_success {
                assert!(result.is_ok(), "{response}");
            } else {
                assert_eq!(result, Err(JevError::ResponseDecode), "{response}");
            }
        }
    }

    #[test]
    fn observations_keep_only_safe_unambiguous_nonalias_model_names() {
        for (model, expected) in [
            ("jev-1.14.0", Some("jev-1.14.0")),
            ("future/jev:release_2040", Some("future/jev:release_2040")),
            (NATIVE_MODEL_ALIAS, None),
            ("", None),
            ("unsafe name", None),
            ("unsafe\nname", None),
            ("漢字", None),
            ("unsafe?name", None),
        ] {
            let body = format!(
                r#"{{"model":{},"answers":{{"q":{{"type":"noul","noul":0.5}}}},"usage":{USAGE}}}"#,
                serde_json::to_string(model).unwrap()
            );
            decode_observed_response(&body, expected, true);
        }
        for length in [128, 129] {
            let model = "a".repeat(length);
            let body = format!(r#"{{"model":"{model}","answers":{{}},"usage":{USAGE}}}"#);
            decode_observed_response(&body, (length == 128).then_some(model.as_str()), true);
        }
    }

    #[test]
    fn invalid_answers_do_not_erase_usage_or_observed_model() {
        for answers in [
            r#"{"q":{}}"#,
            r#"{"q":{"type":"noul","noul":"invalid"}}"#,
            r#"{"q":{"type":"noul","noul":0.5,"extra":true}}"#,
            r#"{"q":{"type":"noul","noul":0.5,"noul":0.8}}"#,
            r#"{"q":{},"q":{}}"#,
        ] {
            let body = format!(
                r#"{{"model":"future/jev:release_2040","answers":{answers},"usage":{USAGE}}}"#
            );
            decode_observed_response(&body, Some("future/jev:release_2040"), false);
        }
    }

    #[test]
    fn missing_malformed_and_duplicate_models_do_not_erase_independent_usage() {
        for fields in [
            "".to_owned(),
            r#""model":null,"#.to_owned(),
            r#""model":42,"#.to_owned(),
            r#""model":[],"#.to_owned(),
            r#""model":{"model":"nested-model"},"#.to_owned(),
            r#""model":"jev-1.14.0","model":"jev-1.14.0","#.to_owned(),
            r#""model":"jev-1.14.0","model":"future-model","#.to_owned(),
            r#""model":"jev-1.14.0","\u006dodel":"future-model","#.to_owned(),
            r#""model":"jev-1.14.0","model":null,"#.to_owned(),
        ] {
            let body = format!(r#"{{{fields}"answers":{{}},"usage":{USAGE}}}"#);
            decode_observed_response(&body, None, false);
        }
    }

    #[test]
    fn model_observation_uses_only_the_selected_response_path() {
        let body = format!(
            r#"{{"model":"jev-1.14.0","metadata":{{"model":"ignored-metadata-model"}},"answers":{{}},"usage":{USAGE}}}"#
        );
        decode_observed_response(&body, Some("jev-1.14.0"), true);
        let body = format!(
            r#"{{"metadata":{{"model":"ignored-metadata-model"}},"result":{{"model":"ignored-result-model"}},"answers":{{}},"usage":{USAGE}}}"#
        );
        decode_observed_response(&body, None, false);
    }

    #[test]
    fn duplicate_usage_never_records_an_observation_from_a_valid_model() {
        for fields in [
            format!(r#""usage":{USAGE},"usage":{USAGE}"#),
            r#""usage":{"input_tokens":12,"input_tokens":12,"output_tokens":1}"#.into(),
            r#""usage":{"input_tokens":12,"output_tokens":1,"output_tokens":1}"#.into(),
        ] {
            let body = format!(r#"{{"model":"jev-1.14.0","answers":{{}},{fields}}}"#);
            let result: Result<Value, _> =
                decode_jev_json_with_observation(body.as_bytes(), JevUsageLocation::Direct, |_| {
                    panic!("ambiguous usage cannot settle an observation")
                });
            assert_eq!(result, Err(JevError::ResponseDecode));
        }
    }

    #[test]
    fn observation_storage_failure_precedes_model_and_answer_decode_errors() {
        let body = format!(
            r#"{{"model":"jev-1.14.0","model":"future-model","answers":{{"q":{{}}}},"usage":{USAGE}}}"#
        );
        let result: Result<super::super::JevResponse, _> = decode_jev_json_with_observation(
            body.as_bytes(),
            JevUsageLocation::Direct,
            |observation| {
                assert_eq!(Some(observation.usage), KNOWN);
                assert_eq!(observation.served_model, None);
                Err(JevError::ProgressStorageFailure)
            },
        );
        assert_eq!(result, Err(JevError::ProgressStorageFailure));
    }

    #[test]
    fn duplicates_at_every_level_fail_without_losing_independent_usage() {
        for answer in [
            r#"{"q":{},"q":{}}"#,
            r#"{"q":{"type":"noul","noul":0.2,"noul":0.9}}"#,
            r#"{"q":{"probabilities":{"0":0.2,"0":0.8}}}"#,
            r#"{"q":{"legend":{"0":"low","0":"high"}}}"#,
            r#"{"q":{"nested":[{"key":1,"key":2}]}}"#,
            r#"{"q":{"probabilities":{"yes":0.2,"\u0079es":0.8}}}"#,
        ] {
            let direct = format!(r#"{{"model":"test","answers":{answer},"usage":{USAGE}}}"#);
            for (body, location) in [
                (direct.clone(), JevUsageLocation::Direct),
                (
                    format!(r#"{{"success":true,"result":{direct}}}"#),
                    JevUsageLocation::CloudflareEnvelope,
                ),
            ] {
                assert_eq!(
                    decode(&body, location, KNOWN),
                    Err(JevError::ResponseDecode)
                );
            }
        }
        let duplicate_answers = format!(r#"{{"answers":{{}},"answers":{{}},"usage":{USAGE}}}"#);
        assert_eq!(
            decode(&duplicate_answers, JevUsageLocation::Direct, KNOWN),
            Err(JevError::ResponseDecode)
        );
    }

    #[test]
    fn ambiguous_usage_and_envelope_members_never_select_a_value() {
        for usage in [
            r#"{"input_tokens":12,"input_tokens":99,"output_tokens":1}"#,
            r#"{"input_tokens":12,"output_tokens":1,"output_tokens":99}"#,
            r#"{"input_tokens":12,"\u0069nput_tokens":99,"output_tokens":1}"#,
        ] {
            let body = format!(r#"{{"usage":{usage}}}"#);
            assert_eq!(
                decode(&body, JevUsageLocation::Direct, None),
                Err(JevError::ResponseDecode)
            );
        }
        let result = format!(r#"{{"usage":{USAGE}}}"#);
        let duplicate_usage = format!(r#"{{"usage":{USAGE},"usage":{USAGE}}}"#);
        for (body, location) in [
            (duplicate_usage.clone(), JevUsageLocation::Direct),
            (
                format!(r#"{{"success":true,"result":{duplicate_usage}}}"#),
                JevUsageLocation::CloudflareEnvelope,
            ),
            (
                format!(r#"{{"success":true,"result":{result},"result":{result}}}"#),
                JevUsageLocation::CloudflareEnvelope,
            ),
            (
                format!(r#"{{"success":true,"success":false,"result":{result}}}"#),
                JevUsageLocation::CloudflareEnvelope,
            ),
            (
                format!(r#"{{"success":true,"errors":[],"errors":[],"result":{result}}}"#),
                JevUsageLocation::CloudflareEnvelope,
            ),
        ] {
            assert_eq!(decode(&body, location, None), Err(JevError::ResponseDecode));
        }
    }

    #[test]
    fn usage_requires_unsigned_integer_tokens_at_the_selected_path() {
        for usage in [
            r#"{"input_tokens":-1,"output_tokens":1}"#,
            r#"{"input_tokens":12.0,"output_tokens":1}"#,
            r#"{"input_tokens":"12","output_tokens":1}"#,
            r#"{"input_tokens":12}"#,
            r#"{"input_tokens":18446744073709551616,"output_tokens":1}"#,
            "null",
        ] {
            let body = format!(r#"{{"usage":{usage}}}"#);
            assert!(decode(&body, JevUsageLocation::Direct, None).is_ok());
        }
        assert!(
            decode(
                &format!(r#"{{"result":{{"usage":{USAGE}}}}}"#),
                JevUsageLocation::Direct,
                None
            )
            .is_ok()
        );
        assert!(
            decode(
                &format!(r#"{{"usage":{USAGE}}}"#),
                JevUsageLocation::CloudflareEnvelope,
                None
            )
            .is_ok()
        );
    }

    #[test]
    fn usage_storage_failure_precedes_duplicate_and_typed_answer_errors() {
        let body = format!(r#"{{"answers":{{"q":{{}},"q":{{}}}},"usage":{USAGE}}}"#);
        let result: Result<super::super::JevResponse, _> =
            decode_jev_json_with_usage(body.as_bytes(), JevUsageLocation::Direct, |_| {
                Err(JevError::ProgressStorageFailure)
            });
        assert_eq!(result, Err(JevError::ProgressStorageFailure));
    }

    #[test]
    fn byte_depth_and_node_limits_apply_before_typed_allocation() {
        let body = format!(
            "{}0{}",
            "[".repeat(MAX_JSON_DEPTH - 1),
            "]".repeat(MAX_JSON_DEPTH - 1)
        );
        assert!(decode(&body, JevUsageLocation::Direct, None).is_ok());
        let body = format!("[{body}]");
        assert_eq!(
            decode(&body, JevUsageLocation::Direct, None),
            Err(JevError::ResponseDecode)
        );

        let body = format!("[{}]", vec!["0"; MAX_JSON_NODES - 1].join(","));
        assert!(body.len() <= MAX_RESPONSE_BYTES);
        assert!(decode(&body, JevUsageLocation::Direct, None).is_ok());
        let body = body.replacen('[', "[0,", 1);
        assert!(body.len() <= MAX_RESPONSE_BYTES);
        assert_eq!(
            decode(&body, JevUsageLocation::Direct, None),
            Err(JevError::ResponseDecode)
        );

        let body = " ".repeat(MAX_RESPONSE_BYTES + 1);
        assert_eq!(
            decode(&body, JevUsageLocation::Direct, None),
            Err(JevError::ResponseTooLarge)
        );
        assert_eq!(
            decode("{}{}", JevUsageLocation::Direct, None),
            Err(JevError::ResponseDecode)
        );
    }
}
