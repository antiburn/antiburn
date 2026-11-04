//! Bounded Cursor 3.25.6 native Read/Grep result bindings.

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
use serde_json::Value;

const MAX_BINARY_BYTES: usize = 256 * 1024;
const MAX_ENCODED_BYTES: usize = 384 * 1024;
const MAX_FIELDS: usize = 4096;
const MAX_TEXT_BYTES: usize = MAX_BINARY_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NativeStatus {
    Success,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DecodeError {
    Malformed,
    UnsafeIdentity,
}

#[derive(Clone, Copy)]
enum FieldValue<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
    Fixed,
}

#[derive(Clone, Copy)]
struct Field<'a> {
    number: u32,
    value: FieldValue<'a>,
}

pub(super) fn decode_completed_call(
    name: &str,
    call_id: &str,
    life_status: &str,
    params: &Value,
    encoded: &str,
) -> Result<NativeStatus, DecodeError> {
    if encoded.len() > MAX_ENCODED_BYTES {
        return Err(DecodeError::Malformed);
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .or_else(|_| STANDARD_NO_PAD.decode(encoded))
        .or_else(|_| URL_SAFE.decode(encoded))
        .or_else(|_| STANDARD.decode(encoded))
        .map_err(|_| DecodeError::Malformed)?;
    if bytes.len() > MAX_BINARY_BYTES {
        return Err(DecodeError::Malformed);
    }

    let fields = parse_fields(&bytes)?;
    require_known(&fields, &[5, 8, 57, 59, 60])?;
    let read = name == "read_file_v2";
    let grep = name == "ripgrep_raw_search";
    if !read && !grep {
        return Err(DecodeError::UnsafeIdentity);
    }
    let tool_call =
        one_bytes(&fields, if read { 8 } else { 5 }, true)?.ok_or(DecodeError::Malformed)?;
    let other = if read { 5 } else { 8 };
    if one_bytes(&fields, other, false)?.is_some() {
        return Err(DecodeError::UnsafeIdentity);
    }
    let stored_id = one_bytes(&fields, 57, true)?.ok_or(DecodeError::Malformed)?;
    if decode_text(stored_id)? != call_id {
        return Err(DecodeError::UnsafeIdentity);
    }
    let _started = one_varint(&fields, 59, false)?;
    let completed = one_varint(&fields, 60, true)?.ok_or(DecodeError::Malformed)?;
    if life_status != "completed" || completed == 0 {
        return Err(DecodeError::Malformed);
    }

    let call_fields = parse_fields(tool_call)?;
    require_known(&call_fields, &[1, 2])?;
    let args = one_bytes(&call_fields, 1, true)?.ok_or(DecodeError::Malformed)?;
    let result = one_bytes(&call_fields, 2, true)?.ok_or(DecodeError::Malformed)?;
    if read {
        read_args_match(args, params)?;
        read_result_status(result)
    } else {
        grep_args_match(args, call_id, params)?;
        grep_result_status(result, params)
    }
}

fn read_args_match(bytes: &[u8], params: &Value) -> Result<(), DecodeError> {
    let fields = parse_fields(bytes)?;
    require_known(&fields, &[1, 2, 3, 5])?;
    let path = decode_text(one_bytes(&fields, 1, true)?.ok_or(DecodeError::Malformed)?)?;
    let params = params.as_object().ok_or(DecodeError::Malformed)?;
    require_param_keys(
        params,
        &[
            "targetFile",
            "effectiveUri",
            "charsLimit",
            "offset",
            "limit",
            "includeLineNumbers",
        ],
    )?;
    if string_param(params, "targetFile")? != path || string_param(params, "effectiveUri")? != path
    {
        return Err(DecodeError::UnsafeIdentity);
    }
    for (field, key) in [(2, "offset"), (3, "limit")] {
        let binary_value = one_varint(&fields, field, false)?;
        let json_value = number_param(params, key)?;
        if binary_value.unwrap_or(0) != json_value.unwrap_or(0) {
            return Err(DecodeError::UnsafeIdentity);
        }
    }
    if number_param(params, "charsLimit")? != Some(1_000_000) {
        return Err(DecodeError::UnsafeIdentity);
    }
    let binary_lines_value = one_varint(&fields, 5, false)?.unwrap_or(0);
    if binary_lines_value > 1 {
        return Err(DecodeError::Malformed);
    }
    let binary_lines = binary_lines_value != 0;
    let json_lines = bool_param(params, "includeLineNumbers")?.unwrap_or(false);
    if binary_lines != json_lines {
        return Err(DecodeError::UnsafeIdentity);
    }
    Ok(())
}

fn read_result_status(bytes: &[u8]) -> Result<NativeStatus, DecodeError> {
    let fields = parse_fields(bytes)?;
    require_known(&fields, &[1, 2])?;
    match (one_bytes(&fields, 1, false)?, one_bytes(&fields, 2, false)?) {
        (Some(success), None) => {
            let fields = parse_fields(success)?;
            require_known(&fields, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13])?;
            let mut outputs = 0;
            for field in [1, 6, 9, 10] {
                outputs += usize::from(one_bytes(&fields, field, false)?.is_some());
            }
            if outputs > 1 {
                return Err(DecodeError::Malformed);
            }
            for field in [2, 3, 11] {
                let _ = one_varint(&fields, field, false)?;
            }
            for field in [4, 5] {
                let _ = one_varint(&fields, field, false)?;
            }
            for field in [7, 9, 10] {
                if let Some(value) = one_bytes(&fields, field, false)? {
                    let _ = decode_text(value)?;
                }
            }
            if let Some(range) = one_bytes(&fields, 8, false)? {
                let range = parse_fields(range)?;
                require_known(&range, &[1, 2])?;
                let _ = one_varint(&range, 1, false)?;
                let _ = one_varint(&range, 2, false)?;
            }
            for field in [12, 13] {
                for value in repeated_bytes(&fields, field)? {
                    if field == 12 {
                        let _ = decode_text(value)?;
                    } else {
                        require_known(
                            &parse_fields(value)?,
                            &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
                        )?;
                    }
                }
            }
            Ok(NativeStatus::Success)
        }
        (None, Some(error)) => {
            let fields = parse_fields(error)?;
            require_known(&fields, &[1])?;
            if let Some(message) = one_bytes(&fields, 1, false)? {
                let _ = decode_text(message)?;
            }
            Ok(NativeStatus::Error)
        }
        _ => Err(DecodeError::Malformed),
    }
}

fn grep_args_match(bytes: &[u8], call_id: &str, params: &Value) -> Result<(), DecodeError> {
    let fields = parse_fields(bytes)?;
    require_known(
        &fields,
        &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
    )?;
    let params = params.as_object().ok_or(DecodeError::Malformed)?;
    require_param_keys(
        params,
        &[
            "pattern",
            "path",
            "outputMode",
            "caseInsensitive",
            "multiline",
        ],
    )?;
    let pattern = decode_text(one_bytes(&fields, 1, true)?.ok_or(DecodeError::Malformed)?)?;
    let path = decode_text(one_bytes(&fields, 2, true)?.ok_or(DecodeError::Malformed)?)?;
    let mode = decode_text(one_bytes(&fields, 4, true)?.ok_or(DecodeError::Malformed)?)?;
    if string_param(params, "pattern")? != pattern
        || string_param(params, "path")? != path
        || string_param(params, "outputMode")? != mode
    {
        return Err(DecodeError::UnsafeIdentity);
    }
    let case_insensitive_value = one_varint(&fields, 8, false)?.unwrap_or(0);
    let multiline_value = one_varint(&fields, 11, false)?.unwrap_or(0);
    if case_insensitive_value > 1 || multiline_value > 1 {
        return Err(DecodeError::Malformed);
    }
    let case_insensitive = case_insensitive_value != 0;
    let multiline = multiline_value != 0;
    if bool_param(params, "caseInsensitive")?.unwrap_or(false) != case_insensitive
        || bool_param(params, "multiline")?.unwrap_or(false) != multiline
    {
        return Err(DecodeError::UnsafeIdentity);
    }
    let stored_id = decode_text(one_bytes(&fields, 14, true)?.ok_or(DecodeError::Malformed)?)?;
    if stored_id != call_id {
        return Err(DecodeError::UnsafeIdentity);
    }
    for field in [3, 9, 12] {
        if one_bytes(&fields, field, false)?.is_some_and(|value| !value.is_empty()) {
            return Err(DecodeError::Malformed);
        }
    }
    for field in [5, 6, 7, 10, 16] {
        if one_varint(&fields, field, false)?.is_some_and(|value| value != 0) {
            return Err(DecodeError::Malformed);
        }
    }
    if one_varint(&fields, 13, false)?.is_some_and(|value| value != 0)
        || one_bytes(&fields, 15, false)?.is_some()
    {
        return Err(DecodeError::Malformed);
    }
    Ok(())
}

fn grep_result_status(bytes: &[u8], params: &Value) -> Result<NativeStatus, DecodeError> {
    let fields = parse_fields(bytes)?;
    require_known(&fields, &[1, 2])?;
    match (one_bytes(&fields, 1, false)?, one_bytes(&fields, 2, false)?) {
        (Some(success), None) => {
            let fields = parse_fields(success)?;
            require_known(&fields, &[1, 2, 3, 4, 5])?;
            let pattern = decode_text(one_bytes(&fields, 1, true)?.ok_or(DecodeError::Malformed)?)?;
            let path = decode_text(one_bytes(&fields, 2, true)?.ok_or(DecodeError::Malformed)?)?;
            let mode = decode_text(one_bytes(&fields, 3, true)?.ok_or(DecodeError::Malformed)?)?;
            let params = params.as_object().ok_or(DecodeError::Malformed)?;
            if string_param(params, "pattern")? != pattern
                || string_param(params, "path")? != path
                || string_param(params, "outputMode")? != mode
                || mode != "content"
            {
                return Err(DecodeError::UnsafeIdentity);
            }
            let mut seen_paths = std::collections::HashSet::new();
            for entry in repeated_bytes(&fields, 4)? {
                let entry = parse_fields(entry)?;
                require_known(&entry, &[1, 2])?;
                let key = decode_text(one_bytes(&entry, 1, true)?.ok_or(DecodeError::Malformed)?)?;
                if !seen_paths.insert(key) {
                    return Err(DecodeError::Malformed);
                }
                let value = one_bytes(&entry, 2, true)?.ok_or(DecodeError::Malformed)?;
                validate_grep_workspace_result(value)?;
            }
            if let Some(active) = one_bytes(&fields, 5, false)? {
                validate_grep_workspace_result(active)?;
            }
            Ok(NativeStatus::Success)
        }
        (None, Some(error)) => {
            let fields = parse_fields(error)?;
            require_known(&fields, &[1])?;
            if let Some(message) = one_bytes(&fields, 1, false)? {
                let _ = decode_text(message)?;
            }
            Ok(NativeStatus::Error)
        }
        _ => Err(DecodeError::Malformed),
    }
}

fn validate_grep_workspace_result(bytes: &[u8]) -> Result<(), DecodeError> {
    let fields = parse_fields(bytes)?;
    require_known(&fields, &[1, 2, 3])?;
    let mut variants = 0;
    for field in [1, 2, 3] {
        variants += usize::from(one_bytes(&fields, field, false)?.is_some());
    }
    if variants != 1 {
        return Err(DecodeError::Malformed);
    }
    let Some(content) = one_bytes(&fields, 3, false)? else {
        return Err(DecodeError::Malformed);
    };
    let content = parse_fields(content)?;
    require_known(&content, &[1, 2, 3, 4, 5, 6, 7])?;
    for field in [2, 3, 6, 7] {
        let _ = one_varint(&content, field, false)?;
    }
    for field in [4, 5] {
        if one_varint(&content, field, false)?.is_some_and(|value| value > 1) {
            return Err(DecodeError::Malformed);
        }
    }
    for file in repeated_bytes(&content, 1)? {
        let file = parse_fields(file)?;
        require_known(&file, &[1, 2, 3, 4, 5])?;
        let _ = decode_text(one_bytes(&file, 1, true)?.ok_or(DecodeError::Malformed)?)?;
        for line in repeated_bytes(&file, 2)? {
            let line = parse_fields(line)?;
            require_known(&line, &[1, 2, 3, 4])?;
            let _ = one_varint(&line, 1, false)?;
            if let Some(text) = one_bytes(&line, 2, false)? {
                let _ = decode_text(text)?;
            }
            for field in [3, 4] {
                if one_varint(&line, field, false)?.is_some_and(|value| value > 1) {
                    return Err(DecodeError::Malformed);
                }
            }
        }
        for field in [3, 4] {
            if one_varint(&file, field, false)?.is_some_and(|value| value > 1) {
                return Err(DecodeError::Malformed);
            }
        }
        if let Some(path) = one_bytes(&file, 5, false)? {
            let _ = decode_text(path)?;
        }
    }
    Ok(())
}

fn parse_fields(bytes: &[u8]) -> Result<Vec<Field<'_>>, DecodeError> {
    let mut fields = Vec::new();
    let mut position = 0;
    while position < bytes.len() {
        if fields.len() == MAX_FIELDS {
            return Err(DecodeError::Malformed);
        }
        let tag = varint(bytes, &mut position).ok_or(DecodeError::Malformed)?;
        let number = u32::try_from(tag >> 3).map_err(|_| DecodeError::Malformed)?;
        if number == 0 {
            return Err(DecodeError::Malformed);
        }
        let value = match tag & 7 {
            0 => FieldValue::Varint(varint(bytes, &mut position).ok_or(DecodeError::Malformed)?),
            1 => {
                take(bytes, &mut position, 8)?;
                FieldValue::Fixed
            }
            2 => {
                let length =
                    usize::try_from(varint(bytes, &mut position).ok_or(DecodeError::Malformed)?)
                        .map_err(|_| DecodeError::Malformed)?;
                FieldValue::Bytes(take(bytes, &mut position, length)?)
            }
            5 => {
                take(bytes, &mut position, 4)?;
                FieldValue::Fixed
            }
            _ => return Err(DecodeError::Malformed),
        };
        fields.push(Field { number, value });
    }
    Ok(fields)
}

fn varint(bytes: &[u8], position: &mut usize) -> Option<u64> {
    let mut result = 0u64;
    for shift in (0..=63).step_by(7) {
        let byte = *bytes.get(*position)?;
        *position += 1;
        if shift == 63 && byte > 1 {
            return None;
        }
        result |= u64::from(byte & 0x7f).checked_shl(shift)?;
        if byte & 0x80 == 0 {
            return Some(result);
        }
    }
    None
}

fn take<'a>(bytes: &'a [u8], position: &mut usize, length: usize) -> Result<&'a [u8], DecodeError> {
    let end = position.checked_add(length).ok_or(DecodeError::Malformed)?;
    let value = bytes.get(*position..end).ok_or(DecodeError::Malformed)?;
    *position = end;
    Ok(value)
}

fn require_known(fields: &[Field<'_>], allowed: &[u32]) -> Result<(), DecodeError> {
    if fields.iter().any(|field| !allowed.contains(&field.number)) {
        return Err(DecodeError::Malformed);
    }
    Ok(())
}

fn one_bytes<'a>(
    fields: &[Field<'a>],
    number: u32,
    required: bool,
) -> Result<Option<&'a [u8]>, DecodeError> {
    let mut matching = fields.iter().filter(|field| field.number == number);
    let Some(field) = matching.next() else {
        return if required {
            Err(DecodeError::Malformed)
        } else {
            Ok(None)
        };
    };
    if matching.next().is_some() {
        return Err(DecodeError::Malformed);
    }
    match field.value {
        FieldValue::Bytes(value) => Ok(Some(value)),
        _ => Err(DecodeError::Malformed),
    }
}

fn one_varint(
    fields: &[Field<'_>],
    number: u32,
    required: bool,
) -> Result<Option<u64>, DecodeError> {
    let mut matching = fields.iter().filter(|field| field.number == number);
    let Some(field) = matching.next() else {
        return if required {
            Err(DecodeError::Malformed)
        } else {
            Ok(None)
        };
    };
    if matching.next().is_some() {
        return Err(DecodeError::Malformed);
    }
    match field.value {
        FieldValue::Varint(value) => Ok(Some(value)),
        _ => Err(DecodeError::Malformed),
    }
}

fn repeated_bytes<'a>(fields: &[Field<'a>], number: u32) -> Result<Vec<&'a [u8]>, DecodeError> {
    fields
        .iter()
        .filter(|field| field.number == number)
        .map(|field| match field.value {
            FieldValue::Bytes(value) => Ok(value),
            _ => Err(DecodeError::Malformed),
        })
        .collect()
}

fn decode_text(bytes: &[u8]) -> Result<&str, DecodeError> {
    if bytes.len() > MAX_TEXT_BYTES {
        return Err(DecodeError::Malformed);
    }
    std::str::from_utf8(bytes).map_err(|_| DecodeError::Malformed)
}

fn string_param<'a>(
    params: &'a serde_json::Map<String, Value>,
    key: &str,
) -> Result<&'a str, DecodeError> {
    params
        .get(key)
        .and_then(Value::as_str)
        .ok_or(DecodeError::Malformed)
}

fn number_param(
    params: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<u64>, DecodeError> {
    match params.get(key) {
        None => Ok(None),
        Some(value) => value.as_u64().map(Some).ok_or(DecodeError::Malformed),
    }
}

fn bool_param(
    params: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<bool>, DecodeError> {
    match params.get(key) {
        None => Ok(None),
        Some(value) => value.as_bool().map(Some).ok_or(DecodeError::Malformed),
    }
}

fn require_param_keys(
    params: &serde_json::Map<String, Value>,
    allowed: &[&str],
) -> Result<(), DecodeError> {
    if params.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(DecodeError::Malformed);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(tool_name: &str) -> (String, Value, String) {
        let record = include_str!(
            "../../../../tests/fixtures/cursor_characterization/native_read_grep_binary.jsonl"
        )
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|record| {
            record
                .pointer("/cursor_tool_result/name")
                .and_then(Value::as_str)
                == Some(tool_name)
        })
        .expect("fixture has requested native tool call");
        let tool = &record["cursor_tool_result"];
        (
            tool["toolCallId"].as_str().unwrap().to_owned(),
            serde_json::from_str(tool["params"].as_str().unwrap()).unwrap(),
            tool["toolCallBinary"].as_str().unwrap().to_owned(),
        )
    }

    #[test]
    fn captured_calls_bind_their_native_args_and_success_oneof() {
        for name in ["read_file_v2", "ripgrep_raw_search"] {
            let (id, params, binary) = call(name);
            assert_eq!(
                decode_completed_call(name, &id, "completed", &params, &binary),
                Ok(NativeStatus::Success)
            );
        }
    }

    #[test]
    fn mismatched_ids_and_args_do_not_claim_success() {
        let (id, params, binary) = call("ripgrep_raw_search");
        assert_eq!(
            decode_completed_call(
                "ripgrep_raw_search",
                "other-call",
                "completed",
                &params,
                &binary
            ),
            Err(DecodeError::UnsafeIdentity)
        );
        assert_eq!(
            decode_completed_call(
                "ripgrep_raw_search",
                &id,
                "completed",
                &json!({"pattern":"different","path":params["path"],"outputMode":"content","caseInsensitive":false,"multiline":false}),
                &binary,
            ),
            Err(DecodeError::UnsafeIdentity)
        );
    }

    #[test]
    fn uncharacterized_read_character_limits_do_not_claim_verified_arguments() {
        let (id, params, binary) = call("read_file_v2");
        for chars_limit in [None, Some(0), Some(1), Some(999_999), Some(1_000_001)] {
            let mut params = params.clone();
            if let Some(limit) = chars_limit {
                params["charsLimit"] = json!(limit);
            } else {
                params.as_object_mut().unwrap().remove("charsLimit");
            }
            assert_eq!(
                decode_completed_call("read_file_v2", &id, "completed", &params, &binary),
                Err(DecodeError::UnsafeIdentity)
            );
        }
    }

    #[test]
    fn duplicate_or_conflicting_tool_oneofs_are_rejected() {
        let (id, params, binary) = call("read_file_v2");
        let decoded = URL_SAFE_NO_PAD.decode(&binary).unwrap();
        let mut duplicate = decoded.clone();
        duplicate.extend_from_slice(&[0x42, 0x00]);
        let duplicate = URL_SAFE_NO_PAD.encode(duplicate);
        assert_eq!(
            decode_completed_call("read_file_v2", &id, "completed", &params, &duplicate),
            Err(DecodeError::Malformed)
        );
        let mut conflicting = decoded;
        conflicting.extend_from_slice(&[0x2a, 0x00]);
        let conflicting = URL_SAFE_NO_PAD.encode(conflicting);
        assert_eq!(
            decode_completed_call("read_file_v2", &id, "completed", &params, &conflicting),
            Err(DecodeError::UnsafeIdentity)
        );
    }
}
