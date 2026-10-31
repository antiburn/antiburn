//! Lossless JSONC insertion and exact owned-delta removal.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent_config::ConfigUnavailableReason;
use crate::agent_config::filesystem::MAX_CONFIG_BYTES;
use crate::agent_config::json::Document;

pub type Result<T> = std::result::Result<T, &'static str>;

fn json_error(reason: ConfigUnavailableReason) -> &'static str {
    match reason {
        ConfigUnavailableReason::DuplicateDefinition => "duplicate_json_key",
        _ => "malformed_jsonc",
    }
}

fn parse(bytes: &[u8]) -> Result<Document> {
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err("config_too_large");
    }
    std::str::from_utf8(bytes).map_err(|_| "malformed_jsonc")?;
    let document = Document::parse(bytes).map_err(json_error)?;
    if !document.value().is_object() {
        return Err("malformed_jsonc");
    }
    check_depth(document.value(), 0)?;
    Ok(document)
}

fn check_depth(value: &Value, depth: usize) -> Result<()> {
    if depth > 32 {
        return Err("json_depth_limit");
    }
    match value {
        Value::Object(object) => {
            for child in object.values() {
                check_depth(child, depth + 1)?;
            }
        }
        Value::Array(array) => {
            for child in array {
                check_depth(child, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Delta {
    pub parent: Vec<String>,
    pub inserted: String,
    pub value: Value,
}

/// Add one array entry, or the smallest missing object that contains it.
pub fn append(bytes: &[u8], path: &[String], value: Value) -> Result<(Vec<u8>, Delta)> {
    let root = parse(bytes)?;
    if path.is_empty() || path.len() > 4 {
        return Err("invalid_config_path");
    }
    let mut parent = root.value();
    let mut prefix = Vec::new();
    for (index, key) in path.iter().enumerate() {
        let object = parent.as_object().ok_or("config_schema_conflict")?;
        if let Some(child) = object.get(key) {
            if index + 1 == path.len() {
                let array = child.as_array().ok_or("config_schema_conflict")?;
                let encoded =
                    serde_json::to_string(&value).map_err(|_| "config_schema_conflict")?;
                // Insert before the first item so all original comments and commas remain.
                let inserted = format!("{encoded}{}", if array.is_empty() { "" } else { "," });
                let position = root
                    .range(path)
                    .map_err(json_error)?
                    .ok_or("config_schema_conflict")?
                    .start
                    + 1;
                let mut result = bytes.to_vec();
                result.splice(position..position, inserted.bytes());
                parse(&result)?;
                let delta = Delta {
                    parent: path.to_vec(),
                    inserted,
                    value,
                };
                return Ok((result, delta));
            }
            parent = child;
            prefix.push(key.clone());
        } else {
            let mut nested = Value::Array(vec![value.clone()]);
            for missing in path[index + 1..].iter().rev() {
                nested = serde_json::json!({ missing: nested });
            }
            let encoded_key = serde_json::to_string(key).map_err(|_| "invalid_config_path")?;
            let inserted = format!(
                "{encoded_key}:{nested}{}",
                if object.is_empty() { "" } else { "," }
            );
            let mut result = bytes.to_vec();
            let position = root
                .range(&prefix)
                .map_err(json_error)?
                .ok_or("config_schema_conflict")?
                .start
                + 1;
            result.splice(position..position, inserted.bytes());
            parse(&result)?;
            return Ok((
                result,
                Delta {
                    parent: prefix,
                    inserted,
                    value,
                },
            ));
        }
    }
    Err("invalid_config_path")
}

/// Remove only the unchanged bytes at the recorded parent boundary.
pub fn remove(bytes: &[u8], delta: &Delta) -> Result<Vec<u8>> {
    let root = parse(bytes)?;
    root.value_at(&delta.parent).ok_or("owned_entry_removed")?;
    let parent = root
        .range(&delta.parent)
        .map_err(json_error)?
        .ok_or("owned_entry_removed")?;
    let start = parent.start + 1;
    let inserted = delta.inserted.as_bytes();
    if bytes.get(start..start + inserted.len()) != Some(inserted) {
        return Err("owned_entry_changed");
    }
    let mut output = bytes.to_vec();
    if !delta.inserted.ends_with(',') {
        let next = root.next_token(start + inserted.len());
        if bytes.get(next) == Some(&b',') {
            output.remove(next);
        }
    }
    output.drain(start..start + inserted.len());
    parse(&output)?;
    Ok(output)
}

pub fn validate(bytes: &[u8]) -> Result<()> {
    parse(bytes).map(|_| ())
}

pub fn boolean(bytes: &[u8], path: &[String]) -> Result<Option<bool>> {
    let root = parse(bytes)?;
    let Some(value) = root.value_at(path) else {
        return Ok(None);
    };
    value.as_bool().map(Some).ok_or("config_schema_conflict")
}

pub fn strings(bytes: &[u8], path: &[String]) -> Result<Vec<String>> {
    let root = parse(bytes)?;
    let Some(value) = root.value_at(path) else {
        return Ok(Vec::new());
    };
    value
        .as_array()
        .ok_or("config_schema_conflict")?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or("config_schema_conflict")
        })
        .collect()
}

pub fn value(bytes: &[u8], path: &[String]) -> Result<Option<Value>> {
    let root = parse(bytes)?;
    Ok(root.value_at(path).cloned())
}

pub fn insert_member(bytes: &[u8], key: &str, value: Value) -> Result<(Vec<u8>, Delta)> {
    let root = parse(bytes)?;
    let object = root.value().as_object().ok_or("config_schema_conflict")?;
    if object.contains_key(key) {
        return Err("config_schema_conflict");
    }
    let inserted = format!(
        "{}:{value}{}",
        serde_json::to_string(key).map_err(|_| "invalid_config_path")?,
        if object.is_empty() { "" } else { "," }
    );
    let mut output = bytes.to_vec();
    let position = root
        .range(&[])
        .map_err(json_error)?
        .ok_or("config_schema_conflict")?
        .start
        + 1;
    output.splice(position..position, inserted.bytes());
    parse(&output)?;
    Ok((
        output,
        Delta {
            parent: Vec::new(),
            inserted,
            value,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insertion_and_removal_preserve_all_original_bytes() {
        for original in [
            "{ // greeting\n \"other\": true, /* trailing */ }\n",
            "{\"hooks\":{\"UserPromptSubmit\":[ // old hook\n {\"hooks\":[]},],},\"other\":42}",
            "{\"hooks\":{}}",
            "{}",
        ] {
            let path = vec!["hooks".into(), "UserPromptSubmit".into()];
            let (output, delta) = append(
                original.as_bytes(),
                &path,
                serde_json::json!({"hooks": [{"command":"test"}]}),
            )
            .expect("insert the hook into each valid JSONC fixture");
            assert_eq!(
                remove(&output, &delta).expect("remove the hook without changing the fixture"),
                original.as_bytes()
            );
            assert!(
                String::from_utf8(output)
                    .expect("decode the inserted-hook fixture as UTF-8")
                    .contains("test")
            );
        }
    }

    #[test]
    fn removal_preserves_later_unrelated_edits_and_refuses_owned_edits() {
        let (output, delta) = append(
            br#"{"hooks":{"test":[]},"unrelated":true}"#,
            &["hooks".into(), "test".into()],
            serde_json::json!({"command":"owned"}),
        )
        .expect("insert the owned hook beside an unrelated setting");
        let updated = String::from_utf8(output)
            .expect("decode the fixture before editing the unrelated setting")
            .replace("true", "false");
        assert_eq!(
            String::from_utf8(
                remove(updated.as_bytes(), &delta)
                    .expect("remove the owned hook after the unrelated setting changes")
            )
            .expect("decode the fixture after removing the owned hook"),
            r#"{"hooks":{"test":[]},"unrelated":false}"#
        );
        assert!(remove(updated.replace("owned", "changed").as_bytes(), &delta).is_err());
    }

    #[test]
    fn removal_preserves_siblings_added_after_an_initially_sole_entry() {
        let (array, delta) = append(
            br#"{"hooks":{"test":[]}}"#,
            &["hooks".into(), "test".into()],
            serde_json::json!({"command":"owned"}),
        )
        .expect("insert the first owned hook into the empty array");
        let array = String::from_utf8(array)
            .expect("decode the array fixture before adding a user hook")
            .replace(
                "]}",
                " /* keep before */ , // keep after\n{\"command\":\"user\"}]}",
            );
        assert_eq!(
            String::from_utf8(
                remove(array.as_bytes(), &delta)
                    .expect("remove the owned hook while keeping the later user hook")
            )
            .expect("decode the array fixture after removing the owned hook"),
            "{\"hooks\":{\"test\":[ /* keep before */  // keep after\n{\"command\":\"user\"}]}}"
        );
        let (object, delta) = insert_member(b"{}", "owned", Value::Bool(true))
            .expect("insert the first owned member into the empty object");
        let object = String::from_utf8(object)
            .expect("decode the object fixture before adding a user member")
            .replace("}", " /* keep before */ , // keep after\n\"user\":true}");
        assert_eq!(
            String::from_utf8(
                remove(object.as_bytes(), &delta)
                    .expect("remove the owned member while keeping the later user member")
            )
            .expect("decode the object fixture after removing the owned member"),
            "{ /* keep before */  // keep after\n\"user\":true}"
        );
    }

    #[test]
    fn malformed_duplicate_and_conflicting_documents_are_refused() {
        for bytes in [
            r#"{"hooks":{},"hooks":{}}"#,
            "{broken}",
            "{/*open",
            r#"{"hooks":false}"#,
        ] {
            assert!(
                append(
                    bytes.as_bytes(),
                    &["hooks".into(), "test".into()],
                    Value::Null
                )
                .is_err(),
                "{bytes}"
            );
        }
    }

    #[test]
    fn validation_and_append_reject_commas_without_entries() {
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
                validate(bytes.as_bytes()),
                Err("malformed_jsonc"),
                "{bytes}"
            );
            assert_eq!(
                append(
                    bytes.as_bytes(),
                    &["hooks".into(), "test".into()],
                    Value::Null
                )
                .map(|_| ()),
                Err("malformed_jsonc"),
                "{bytes}"
            );
        }
    }

    #[test]
    fn removal_keeps_comments_when_a_sole_entry_gets_a_trailing_comma() {
        let (output, delta) = insert_member(b"{}", "owned", Value::Bool(true))
            .expect("insert the sole owned member before adding a trailing comma");
        let updated = String::from_utf8(output)
            .expect("decode the object fixture before adding a trailing comma")
            .replace('}', " /* keep , */ , // keep after\r\n}");
        assert_eq!(
            remove(updated.as_bytes(), &delta)
                .expect("remove the sole owned member and keep the trailing comments"),
            b"{ /* keep , */  // keep after\r\n}"
        );
        let (output, delta) = append(
            br#"{"hooks":[]}"#,
            &["hooks".into()],
            Value::String("owned".into()),
        )
        .expect("insert the sole owned array item before adding a trailing comma");
        let updated = String::from_utf8(output)
            .expect("decode the array fixture before adding a trailing comma")
            .replace(']', " /* keep , */ ,]");
        assert_eq!(
            remove(updated.as_bytes(), &delta)
                .expect("remove the sole owned array item and keep the trailing comment"),
            br#"{"hooks":[ /* keep , */ ]}"#
        );
    }

    #[test]
    fn integration_depth_and_size_limits_stay_in_force() {
        let within_depth = format!("{}true{}", "{\"nested\":".repeat(32), "}".repeat(32));
        validate(within_depth.as_bytes()).expect("accept the fixture at the 32-level depth limit");
        let too_deep = format!("{}true{}", "{\"nested\":".repeat(33), "}".repeat(33));
        assert_eq!(validate(too_deep.as_bytes()), Err("json_depth_limit"));
        let mut bytes = b"{}".to_vec();
        bytes.resize(MAX_CONFIG_BYTES as usize, b' ');
        validate(&bytes).expect("accept the fixture at the configuration byte limit");
        bytes.push(b' ');
        assert_eq!(validate(&bytes), Err("config_too_large"));
    }
}
