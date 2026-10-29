use std::collections::BTreeSet;
use std::fmt;
#[cfg(not(windows))]
use std::ops::Range;

use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};

use crate::agent_config::ConfigUnavailableReason;

pub(in crate::agent_config) fn parse(bytes: &[u8]) -> Result<Value, ConfigUnavailableReason> {
    parse_json(&strip_comments(bytes)?)
}

pub(in crate::agent_config) fn parse_strict(
    bytes: &[u8],
) -> Result<Value, ConfigUnavailableReason> {
    parse_json(bytes)
}

#[cfg(not(windows))]
pub(super) fn edit_top_level_string(
    bytes: &[u8],
    key: &str,
    proposed: &str,
) -> Result<Vec<u8>, ConfigUnavailableReason> {
    let sanitized = strip_comments(bytes)?;
    parse_json(&sanitized)?;
    let range = object_member(&sanitized, skip_space(&sanitized, 0), key)?
        .ok_or(ConfigUnavailableReason::MissingTarget)?;
    if sanitized.get(range.start) != Some(&b'"') {
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
    let sanitized = strip_comments(bytes)?;
    parse_json(&sanitized)?;
    if path.is_empty() || path.len() > 16 {
        return Err(ConfigUnavailableReason::InvalidTarget);
    }
    let mut object_start = skip_space(&sanitized, 0);
    for (index, key) in path.iter().enumerate() {
        let found = object_member(&sanitized, object_start, key)?;
        if index + 1 < path.len() {
            object_start = found.ok_or(ConfigUnavailableReason::MissingTarget)?.start;
            continue;
        }
        let encoded =
            serde_json::to_vec(proposed).map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
        let (range, replacement) = if let Some(range) = found {
            (range, encoded)
        } else if insert_leaf {
            let mut replacement =
                serde_json::to_vec(key).map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
            replacement.push(b':');
            replacement.extend(encoded);
            if sanitized.get(skip_space(&sanitized, object_start + 1)) != Some(&b'}') {
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

#[cfg(not(windows))]
fn object_member(
    bytes: &[u8],
    start: usize,
    wanted: &str,
) -> Result<Option<Range<usize>>, ConfigUnavailableReason> {
    if bytes.get(start) != Some(&b'{') {
        return Err(ConfigUnavailableReason::MalformedConfig);
    }
    let mut index = start + 1;
    loop {
        index = skip_space(bytes, index);
        if bytes.get(index) == Some(&b'}') {
            return Ok(None);
        }
        let (key_range, next) = string_range(bytes, index)?;
        let key: String = serde_json::from_slice(&bytes[key_range])
            .map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
        index = skip_space(bytes, next);
        if bytes.get(index) != Some(&b':') {
            return Err(ConfigUnavailableReason::MalformedConfig);
        }
        let value_start = skip_space(bytes, index + 1);
        index = value_end(bytes, value_start)?;
        if key == wanted {
            let mut end = index;
            while end > value_start && bytes[end - 1].is_ascii_whitespace() {
                end -= 1;
            }
            return Ok(Some(value_start..end));
        }
        index = skip_space(bytes, index);
        match bytes.get(index) {
            Some(b',') => index += 1,
            Some(b'}') => return Ok(None),
            _ => return Err(ConfigUnavailableReason::MalformedConfig),
        }
    }
}

fn strip_comments(bytes: &[u8]) -> Result<Vec<u8>, ConfigUnavailableReason> {
    let mut output = bytes.to_vec();
    let mut index = 0;
    let mut in_string = false;
    let mut escaped = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        if byte == b'"' {
            in_string = true;
            index += 1;
        } else if byte == b'/' && bytes.get(index + 1) == Some(&b'/') {
            output[index] = b' ';
            output[index + 1] = b' ';
            index += 2;
            while index < bytes.len() && !matches!(bytes[index], b'\n' | b'\r') {
                output[index] = b' ';
                index += 1;
            }
        } else if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
            output[index] = b' ';
            output[index + 1] = b' ';
            index += 2;
            let mut closed = false;
            while index < bytes.len() {
                if bytes[index] == b'*' && bytes.get(index + 1) == Some(&b'/') {
                    output[index] = b' ';
                    output[index + 1] = b' ';
                    index += 2;
                    closed = true;
                    break;
                }
                if !matches!(bytes[index], b'\n' | b'\r') {
                    output[index] = b' ';
                }
                index += 1;
            }
            if !closed {
                return Err(ConfigUnavailableReason::MalformedConfig);
            }
        } else {
            index += 1;
        }
    }
    if in_string {
        return Err(ConfigUnavailableReason::MalformedConfig);
    }
    let mut index = 0;
    let mut in_string = false;
    let mut escaped = false;
    while index < output.len() {
        if in_string {
            if escaped {
                escaped = false;
            } else if output[index] == b'\\' {
                escaped = true;
            } else if output[index] == b'"' {
                in_string = false;
            }
        } else if output[index] == b'"' {
            in_string = true;
        } else if output[index] == b',' {
            let next = skip_space(&output, index + 1);
            if matches!(output.get(next), Some(b'}' | b']')) {
                output[index] = b' ';
            }
        }
        index += 1;
    }
    Ok(output)
}

fn skip_space(bytes: &[u8], mut index: usize) -> usize {
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    index
}

#[cfg(not(windows))]
fn string_range(
    bytes: &[u8],
    start: usize,
) -> Result<(Range<usize>, usize), ConfigUnavailableReason> {
    if bytes.get(start) != Some(&b'"') {
        return Err(ConfigUnavailableReason::MalformedConfig);
    }
    let mut index = start + 1;
    let mut escaped = false;
    while let Some(byte) = bytes.get(index) {
        if escaped {
            escaped = false;
        } else if *byte == b'\\' {
            escaped = true;
        } else if *byte == b'"' {
            return Ok((start..index + 1, index + 1));
        }
        index += 1;
    }
    Err(ConfigUnavailableReason::MalformedConfig)
}

#[cfg(not(windows))]
fn value_end(bytes: &[u8], start: usize) -> Result<usize, ConfigUnavailableReason> {
    if bytes.get(start) == Some(&b'"') {
        return string_range(bytes, start).map(|(_, end)| end);
    }
    let mut index = start;
    let mut object_depth = 0usize;
    let mut array_depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    while let Some(byte) = bytes.get(index) {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
        } else {
            match byte {
                b'"' => in_string = true,
                b'{' => object_depth += 1,
                b'}' if object_depth > 0 => object_depth -= 1,
                b'[' => array_depth += 1,
                b']' if array_depth > 0 => array_depth -= 1,
                b',' | b'}' if object_depth == 0 && array_depth == 0 => return Ok(index),
                _ => {}
            }
        }
        index += 1;
    }
    Err(ConfigUnavailableReason::MalformedConfig)
}

fn parse_json(bytes: &[u8]) -> Result<Value, ConfigUnavailableReason> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = UniqueJson::deserialize(&mut deserializer)
        .map_err(|error| {
            if error.to_string().contains("duplicate object key") {
                ConfigUnavailableReason::DuplicateDefinition
            } else {
                ConfigUnavailableReason::MalformedConfig
            }
        })?
        .0;
    deserializer
        .end()
        .map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
    Ok(value)
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
}

struct UniqueJson(Value);
struct UniqueJsonVisitor;

impl<'de> Deserialize<'de> for UniqueJson {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(UniqueJsonVisitor)
    }
}

impl<'de> Visitor<'de> for UniqueJsonVisitor {
    type Value = UniqueJson;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value without duplicate keys")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(UniqueJson(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(UniqueJson(Value::Number(value.into())))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(UniqueJson(Value::Number(value.into())))
    }

    fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Self::Value, E> {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .map(UniqueJson)
            .ok_or_else(|| E::custom("invalid number"))
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
        Ok(UniqueJson(Value::String(value.into())))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(UniqueJson(Value::String(value)))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(UniqueJson(Value::Null))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(UniqueJson(Value::Null))
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        UniqueJson::deserialize(deserializer)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element::<UniqueJson>()? {
            values.push(value.0);
        }
        Ok(UniqueJson(Value::Array(values)))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut values = Map::new();
        let mut keys = BTreeSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !keys.insert(key.clone()) {
                return Err(serde::de::Error::custom("duplicate object key"));
            }
            values.insert(key, map.next_value::<UniqueJson>()?.0);
        }
        Ok(UniqueJson(Value::Object(values)))
    }
}
