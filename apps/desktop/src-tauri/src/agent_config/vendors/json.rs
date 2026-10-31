#[cfg(not(windows))]
use serde_json::Value;

#[cfg(not(windows))]
use crate::agent_config::ConfigUnavailableReason;
#[cfg(not(windows))]
use crate::agent_config::json::Document;
pub(in crate::agent_config) use crate::agent_config::json::{parse, parse_strict};

#[cfg(not(windows))]
pub(super) fn edit_top_level_string(
    bytes: &[u8],
    key: &str,
    proposed: &str,
) -> Result<Vec<u8>, ConfigUnavailableReason> {
    let document = Document::parse(bytes)?;
    if !document.value().is_object() {
        return Err(ConfigUnavailableReason::MalformedConfig);
    }
    let value = document
        .value_at(&[key.into()])
        .ok_or(ConfigUnavailableReason::MissingTarget)?;
    if !value.is_string() {
        return Err(ConfigUnavailableReason::MalformedConfig);
    }
    edit_path_value(bytes, &[key.into()], &Value::String(proposed.into()), false)
}

#[cfg(not(windows))]
pub(in crate::agent_config) fn edit_path_value(
    bytes: &[u8],
    path: &[String],
    proposed: &Value,
    insert_leaf: bool,
) -> Result<Vec<u8>, ConfigUnavailableReason> {
    let document = Document::parse(bytes)?;
    if path.is_empty() || path.len() > 16 {
        return Err(ConfigUnavailableReason::InvalidTarget);
    }
    let mut object_start = document
        .range(&[])?
        .ok_or(ConfigUnavailableReason::MalformedConfig)?
        .start;
    for (index, _) in path.iter().enumerate() {
        let found = document.range(&path[..=index])?;
        if index + 1 < path.len() {
            object_start = found.ok_or(ConfigUnavailableReason::MissingTarget)?.start;
            continue;
        }
        let encoded =
            serde_json::to_vec(proposed).map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
        let (range, replacement) = if let Some(range) = found {
            (range, encoded)
        } else if insert_leaf {
            let mut replacement = serde_json::to_vec(&path[index])
                .map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
            replacement.push(b':');
            replacement.extend(encoded);
            if bytes.get(document.next_token(object_start + 1)) != Some(&b'}') {
                replacement.push(b',');
            }
            (object_start + 1..object_start + 1, replacement)
        } else {
            return Err(ConfigUnavailableReason::MissingTarget);
        };
        let mut output = Vec::with_capacity(bytes.len() + replacement.len());
        output.extend_from_slice(&bytes[..range.start]);
        output.extend(replacement);
        output.extend_from_slice(&bytes[range.end..]);
        parse(&output)?;
        return Ok(output);
    }
    Err(ConfigUnavailableReason::InvalidTarget)
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;

    #[test]
    fn top_level_and_nested_edits_share_the_jsonc_member_scanner() {
        let bytes = br#"{
            // Keep this comment and escaped text.
            "other": {"model":"nested", "values":[1,2,{"x":"a\\\"b"}]},
            "model": "old" /* keep this comment */,
        }"#;
        let top = edit_top_level_string(bytes, "model", "new").unwrap();
        let nested = edit_path_value(
            bytes,
            &["model".into()],
            &Value::String("new".into()),
            false,
        )
        .unwrap();
        assert_eq!(top, nested);
        assert!(
            String::from_utf8(top.clone())
                .unwrap()
                .contains("/* keep this comment */")
        );
        assert_eq!(parse(&top).unwrap()["other"]["model"], "nested");
        assert_eq!(parse(&top).unwrap()["model"], "new");
        assert_eq!(
            edit_top_level_string(br#"{"model":false}"#, "model", "new"),
            Err(ConfigUnavailableReason::MalformedConfig)
        );
        assert_eq!(
            edit_top_level_string(br#"{"model":"one","model":"two"}"#, "model", "new"),
            Err(ConfigUnavailableReason::DuplicateDefinition)
        );
    }

    #[test]
    fn integration_and_vendor_parsing_share_jsonc_acceptance_and_values() {
        for bytes in [
            br#"{"hooks":[{"command":"https://example.test/*keep*/"},],}"#.as_slice(),
            b"{ // heading\r\n\"escaped\\u005fkey\":{\"value\":true,}, /* end */ }",
            "{\"text\":\"你好\",\"nested\":[[false,null,1.25],],}".as_bytes(),
        ] {
            let expected = parse(bytes).unwrap();
            assert_eq!(
                crate::agent_integrations::document::value(bytes, &[]).unwrap(),
                Some(expected)
            );
        }
        for (bytes, reason) in [
            (
                br#"{"a":1,"\u0061":2}"#.as_slice(),
                ConfigUnavailableReason::DuplicateDefinition,
            ),
            (
                br#"{"nested":[{"a":1,"a":2}]}"#.as_slice(),
                ConfigUnavailableReason::DuplicateDefinition,
            ),
            (
                b"{/* open".as_slice(),
                ConfigUnavailableReason::MalformedConfig,
            ),
            (
                br#"{"a":[1,,]}"#.as_slice(),
                ConfigUnavailableReason::MalformedConfig,
            ),
            (
                br#"{"a":true false}"#.as_slice(),
                ConfigUnavailableReason::MalformedConfig,
            ),
        ] {
            assert_eq!(parse(bytes), Err(reason));
            assert!(crate::agent_integrations::document::validate(bytes).is_err());
            assert_eq!(Document::parse(bytes).map(|_| ()), Err(reason));
        }
    }

    #[test]
    fn strict_parsing_and_object_edit_limits_stay_separate() {
        for bytes in [b"{/* comment */}".as_slice(), br#"{"a":true,}"#.as_slice()] {
            assert!(parse(bytes).is_ok());
            assert_eq!(
                parse_strict(bytes),
                Err(ConfigUnavailableReason::MalformedConfig)
            );
        }
        for bytes in [b"[]".as_slice(), b"true".as_slice()] {
            assert!(parse(bytes).is_ok());
            assert!(crate::agent_integrations::document::validate(bytes).is_err());
            assert_eq!(
                edit_top_level_string(bytes, "model", "new"),
                Err(ConfigUnavailableReason::MalformedConfig)
            );
            assert_eq!(
                edit_path_value(bytes, &["model".into()], &Value::Bool(true), true),
                Err(ConfigUnavailableReason::MalformedConfig)
            );
        }
    }
}
