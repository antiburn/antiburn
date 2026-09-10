use std::collections::BTreeSet;
use std::fmt;
use std::ops::Range;

use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};

use crate::agent_config::ConfigUnavailableReason;

pub(super) fn parse(bytes: &[u8]) -> Result<Value, ConfigUnavailableReason> {
    parse_json(&strip_comments(bytes)?)
}

pub(super) fn parse_strict(bytes: &[u8]) -> Result<Value, ConfigUnavailableReason> {
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
    let range =
        top_level_string_range(&sanitized, key)?.ok_or(ConfigUnavailableReason::MissingTarget)?;
    let encoded =
        serde_json::to_vec(proposed).map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
    let mut output = Vec::with_capacity(bytes.len() + encoded.len());
    output.extend_from_slice(&bytes[..range.start]);
    output.extend_from_slice(&encoded);
    output.extend_from_slice(&bytes[range.end..]);
    Ok(output)
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

fn top_level_string_range(
    bytes: &[u8],
    wanted: &str,
) -> Result<Option<Range<usize>>, ConfigUnavailableReason> {
    let mut index = skip_space(bytes, 0);
    if bytes.get(index) != Some(&b'{') {
        return Err(ConfigUnavailableReason::MalformedConfig);
    }
    index += 1;
    loop {
        index = skip_space(bytes, index);
        if bytes.get(index) == Some(&b'}') {
            return Ok(None);
        }
        let (key_range, next) = string_range(bytes, index)?;
        let key: String = serde_json::from_slice(&bytes[key_range.clone()])
            .map_err(|_| ConfigUnavailableReason::MalformedConfig)?;
        index = skip_space(bytes, next);
        if bytes.get(index) != Some(&b':') {
            return Err(ConfigUnavailableReason::MalformedConfig);
        }
        index = skip_space(bytes, index + 1);
        let value_start = index;
        index = value_end(bytes, index)?;
        if key == wanted {
            if bytes.get(value_start) != Some(&b'"') {
                return Err(ConfigUnavailableReason::MalformedConfig);
            }
            return Ok(Some(value_start..index));
        }
        index = skip_space(bytes, index);
        match bytes.get(index) {
            Some(b',') => index += 1,
            Some(b'}') => return Ok(None),
            _ => return Err(ConfigUnavailableReason::MalformedConfig),
        }
    }
}

fn skip_space(bytes: &[u8], mut index: usize) -> usize {
    while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    index
}

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
