//! JSONC validation and byte ranges for lossless configuration edits.

use std::collections::BTreeSet;
use std::fmt;
use std::ops::Range;

use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};

use super::ConfigUnavailableReason;

pub(crate) fn parse(bytes: &[u8]) -> Result<Value, ConfigUnavailableReason> {
    let mut sanitized = strip_comments(bytes)?;
    strip_trailing_commas(&mut sanitized);
    parse_strict(&sanitized)
}

pub(crate) fn parse_strict(bytes: &[u8]) -> Result<Value, ConfigUnavailableReason> {
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

pub(crate) struct Document {
    sanitized: Vec<u8>,
    value: Value,
}

impl Document {
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self, ConfigUnavailableReason> {
        let sanitized = strip_comments(bytes)?;
        let mut json = sanitized.clone();
        strip_trailing_commas(&mut json);
        let value = parse_strict(&json)?;
        Ok(Self { sanitized, value })
    }

    pub(crate) fn value(&self) -> &Value {
        &self.value
    }

    pub(crate) fn value_at(&self, path: &[String]) -> Option<&Value> {
        path.iter()
            .try_fold(&self.value, |value, key| value.get(key))
    }

    pub(crate) fn range(
        &self,
        path: &[String],
    ) -> Result<Option<Range<usize>>, ConfigUnavailableReason> {
        let start = self.next_token(0);
        let end = self
            .sanitized
            .iter()
            .rposition(|byte| !byte.is_ascii_whitespace())
            .ok_or(ConfigUnavailableReason::MalformedConfig)?
            + 1;
        let mut range = start..end;
        for key in path {
            let Some(member) = object_member(&self.sanitized, range.start, key)? else {
                return Ok(None);
            };
            range = member;
        }
        Ok(Some(range))
    }

    pub(crate) fn next_token(&self, index: usize) -> usize {
        skip_space(&self.sanitized, index)
    }
}

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
    Ok(output)
}

fn strip_trailing_commas(output: &mut [u8]) {
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
            let next = skip_space(output, index + 1);
            let previous = output[..index]
                .iter()
                .rfind(|byte| !byte.is_ascii_whitespace());
            if matches!(output.get(next), Some(b'}' | b']'))
                && previous.is_some_and(|byte| !matches!(byte, b'{' | b'[' | b',' | b':'))
            {
                output[index] = b' ';
            }
        }
        index += 1;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commas_without_entries_stay_malformed() {
        for bytes in [
            "{,}",
            "[,]",
            r#"{"other":[,]}"#,
            r#"{"other":{,}}"#,
            "{ /* before */ , /* after */ }",
            "[ // before\r\n, // after\r\n]",
            r#"{"other":[ /* before */ , /* after */ ]}"#,
            "{\"other\":{ // before\n, // after\n}}",
        ] {
            assert_eq!(
                parse(bytes.as_bytes()),
                Err(ConfigUnavailableReason::MalformedConfig),
                "{bytes}"
            );
            assert_eq!(
                Document::parse(bytes.as_bytes()).map(|_| ()),
                Err(ConfigUnavailableReason::MalformedConfig),
                "{bytes}"
            );
        }
        for bytes in [
            "[true,]",
            "{\"other\":[ /* before */ null, /* after */ ],}",
            "{\"other\":{ // before\n\"value\":false, // after\n},}",
        ] {
            let expected = parse(bytes.as_bytes()).unwrap();
            assert_eq!(
                Document::parse(bytes.as_bytes()).unwrap().value(),
                &expected
            );
        }
    }

    #[test]
    fn ranges_keep_byte_offsets_and_exclude_surrounding_comments() {
        let bytes = r#"{
            "前置": {"list":["a\\\"b", {"x":true,},],},
            "nested": {"\u006bey": /* before */ ["//text", /* inner */ false,] /* after */,},
            "last": true /* keep */ ,
        }"#
        .as_bytes();
        let document = Document::parse(bytes).unwrap();
        assert_eq!(document.value(), &parse(bytes).unwrap());
        let range = document
            .range(&["nested".into(), "key".into()])
            .unwrap()
            .unwrap();
        assert_eq!(&bytes[range], br#"["//text", /* inner */ false,]"#);
        let range = document.range(&["last".into()]).unwrap().unwrap();
        assert_eq!(&bytes[range.clone()], b"true");
        assert_eq!(bytes[document.next_token(range.end)], b',');
        assert_eq!(document.range(&["missing".into()]).unwrap(), None);
    }
}
