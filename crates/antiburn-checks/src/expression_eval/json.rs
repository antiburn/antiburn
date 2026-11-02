use std::fmt;

use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

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

pub(super) fn parse(
    text: &str,
    budget: &mut EvaluationBudget,
    depth: usize,
) -> Result<Value, EvaluationError> {
    budget.reserve_generated_bytes(text.len())?;
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let result = ValueSeed {
        budget,
        depth,
        item: false,
    }
    .deserialize(&mut deserializer);
    let result = result.and_then(|value| deserializer.end().map(|()| value));
    match result {
        Ok(value) => Ok(value),
        Err(_) if budget.exhausted() => Err(limit_error(
            "JSON parsing exhausted the shared evaluation budget.",
        )),
        Err(_) => Err(invalid_error("fromJSON requires valid finite JSON.")),
    }
}

struct ValueSeed<'a> {
    budget: &'a mut EvaluationBudget,
    depth: usize,
    item: bool,
}

impl<'de> DeserializeSeed<'de> for ValueSeed<'_> {
    type Value = Value;

    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        self.budget
            .step(self.depth)
            .map_err(|error| serde::de::Error::custom(error.message))?;
        if self.item {
            self.budget
                .visit_items(1)
                .map_err(|error| serde::de::Error::custom(error.message))?;
        }
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for ValueSeed<'_> {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("finite JSON")
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }
    fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }
    fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }
    fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Value, E> {
        Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("JSON number must be finite."))
    }
    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Value, E> {
        Ok(Value::String(value.to_owned()))
    }
    fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Value, E> {
        Ok(Value::String(value))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Value, A::Error> {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(ValueSeed {
            budget: self.budget,
            depth: self.depth + 1,
            item: true,
        })? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut object: A) -> Result<Value, A::Error> {
        let mut values = Map::new();
        while let Some(key) = object.next_key::<String>()? {
            let value = object.next_value_seed(ValueSeed {
                budget: self.budget,
                depth: self.depth + 1,
                item: true,
            })?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}
