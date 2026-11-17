use serde_json::Value;

use crate::bounded_json::{DuplicateKeys, JsonLimits, decode_json_with};

use super::functions::append;
use super::value::ValueProvenance;
use super::{EvaluationBudget, EvaluationError, EvaluationTrace, invalid_error, limit_error};

pub(super) fn serialize(
    value: &Value,
    provenance: &ValueProvenance,
    budget: &mut EvaluationBudget,
    depth: usize,
) -> Result<(String, EvaluationTrace), EvaluationError> {
    let mut text = String::new();
    let mut sources = EvaluationTrace::default();
    write_json(value, provenance, &mut text, &mut sources, budget, depth)?;
    Ok((text, sources))
}

fn write_json(
    value: &Value,
    provenance: &ValueProvenance,
    output: &mut String,
    sources: &mut EvaluationTrace,
    budget: &mut EvaluationBudget,
    depth: usize,
) -> Result<(), EvaluationError> {
    budget.step(depth)?;
    provenance.collect_sources(value, sources);
    match value {
        Value::Null => append(output, "null", budget),
        Value::Bool(value) => append(output, if *value { "true" } else { "false" }, budget),
        Value::Number(value) => {
            let text = super::scalar_text(&Value::Number(value.clone()), budget)?;
            append(output, &text, budget)
        }
        Value::String(value) => write_string(value, output, budget),
        Value::Array(items) => {
            budget.visit_items(items.len())?;
            append(output, "[", budget)?;
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    append(output, ",", budget)?;
                }
                let child = provenance.child(&super::PathSegment::Index(index));
                write_json(item, &child, output, sources, budget, depth + 1)?;
            }
            append(output, "]", budget)
        }
        Value::Object(object) => {
            budget.visit_items(object.len())?;
            let mut entries: Vec<_> = object.iter().collect();
            entries.sort_unstable_by_key(|(key, _)| *key);
            append(output, "{", budget)?;
            for (index, (key, value)) in entries.into_iter().enumerate() {
                if index > 0 {
                    append(output, ",", budget)?;
                }
                write_string(key, output, budget)?;
                append(output, ":", budget)?;
                let child = provenance.child(&super::PathSegment::Property(key.clone()));
                write_json(value, &child, output, sources, budget, depth + 1)?;
            }
            append(output, "}", budget)
        }
    }
}

fn write_string(
    value: &str,
    output: &mut String,
    budget: &mut EvaluationBudget,
) -> Result<(), EvaluationError> {
    append(output, "\"", budget)?;
    let mut start = 0;
    for (index, character) in value.char_indices() {
        let escape = match character {
            '"' => "\\\"",
            '\\' => "\\\\",
            '\n' => "\\n",
            '\r' => "\\r",
            '\t' => "\\t",
            '\u{08}' => "\\b",
            '\u{0c}' => "\\f",
            character if character <= '\u{1f}' => {
                append(output, &value[start..index], budget)?;
                let code = u32::from(character);
                let digits = b"0123456789abcdef";
                let high = usize::try_from(code / 16).expect("control character");
                let low = usize::try_from(code % 16).expect("control character");
                let bytes = [b'\\', b'u', b'0', b'0', digits[high], digits[low]];
                append(
                    output,
                    std::str::from_utf8(&bytes).expect("ASCII escape"),
                    budget,
                )?;
                start = index + character.len_utf8();
                continue;
            }
            _ => continue,
        };
        append(output, &value[start..index], budget)?;
        append(output, escape, budget)?;
        start = index + character.len_utf8();
    }
    append(output, &value[start..], budget)?;
    append(output, "\"", budget)
}

/// Parses JSON with the shared budget and keeps the last value for duplicate keys.
/// Charges overwritten values too; object keys do not count as visited items.
pub(super) fn parse(
    text: &str,
    budget: &mut EvaluationBudget,
    depth: usize,
) -> Result<Value, EvaluationError> {
    budget.reserve_generated_bytes(text.len())?;
    let limits = JsonLimits {
        max_bytes: u64::try_from(text.len())
            .map_err(|_| invalid_error("fromJSON requires valid finite JSON."))?,
        max_nodes: usize::MAX,
        max_depth: usize::MAX,
    };
    let result = decode_json_with(
        text.as_bytes(),
        limits,
        DuplicateKeys::LastWins,
        |relative_depth| {
            if budget.step(depth + (relative_depth - 1)).is_err() {
                return false;
            }
            relative_depth == 1 || budget.visit_items(1).is_ok()
        },
    );
    match result {
        Ok(value) => Ok(value),
        Err(_) if budget.exhausted() => Err(limit_error(
            "JSON parsing exhausted the shared evaluation budget.",
        )),
        Err(_) => Err(invalid_error("fromJSON requires valid finite JSON.")),
    }
}
