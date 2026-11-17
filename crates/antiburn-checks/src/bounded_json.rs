//! Strict JSON decoding with byte, value-count, and complete-wrapper depth bounds.

use std::fmt;

use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

mod measure;
pub use measure::measure_json;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JsonLimits {
    pub max_bytes: u64,
    pub max_nodes: usize,
    pub max_depth: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JsonMeasurement {
    pub bytes: u64,
    pub nodes: usize,
    pub depth: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BoundedJsonError {
    Bytes { observed: u64, maximum: u64 },
    Nodes { observed: usize, maximum: usize },
    Depth { observed: usize, maximum: usize },
    DuplicateKey(String),
    InvalidJson(String),
    Serialization(String),
}

impl fmt::Display for BoundedJsonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bytes { observed, maximum } => {
                write!(formatter, "JSON bytes {observed} exceed {maximum}.")
            }
            Self::Nodes { observed, maximum } => {
                write!(formatter, "JSON values {observed} exceed {maximum}.")
            }
            Self::Depth { observed, maximum } => {
                write!(formatter, "JSON depth {observed} exceeds {maximum}.")
            }
            Self::DuplicateKey(key) => write!(formatter, "Duplicate JSON key: {key}."),
            Self::InvalidJson(message) | Self::Serialization(message) => {
                formatter.write_str(message)
            }
        }
    }
}

impl std::error::Error for BoundedJsonError {}

impl serde::ser::Error for BoundedJsonError {
    fn custom<T: fmt::Display>(message: T) -> Self {
        Self::Serialization(message.to_string())
    }
}

pub(super) struct NodeCounter {
    limits: JsonLimits,
    nodes: usize,
    depth: usize,
}

impl NodeCounter {
    fn new(limits: JsonLimits) -> Self {
        Self {
            limits,
            nodes: 0,
            depth: 0,
        }
    }

    fn charge(&mut self, depth: usize) -> Result<(), BoundedJsonError> {
        if depth > self.limits.max_depth {
            return Err(BoundedJsonError::Depth {
                observed: depth,
                maximum: self.limits.max_depth,
            });
        }
        if self.nodes >= self.limits.max_nodes {
            return Err(BoundedJsonError::Nodes {
                observed: self.nodes.saturating_add(1),
                maximum: self.limits.max_nodes,
            });
        }
        self.nodes += 1;
        self.depth = self.depth.max(depth);
        Ok(())
    }
}

struct DecodeState {
    counter: NodeCounter,
}

struct ValueSeed<'a> {
    state: &'a mut DecodeState,
    depth: usize,
}

impl<'de> DeserializeSeed<'de> for ValueSeed<'_> {
    type Value = Value;

    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        self.state
            .counter
            .charge(self.depth)
            .map_err(serde::de::Error::custom)?;
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for ValueSeed<'_> {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("strict bounded JSON")
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
            .ok_or_else(|| E::custom("JSON numbers must be finite."))
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
            state: self.state,
            depth: self.depth + 1,
        })? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut mapping: A) -> Result<Value, A::Error> {
        let mut fields = Map::new();
        while let Some(key) = mapping.next_key::<String>()? {
            if fields.contains_key(&key) {
                let error = BoundedJsonError::DuplicateKey(key);
                return Err(serde::de::Error::custom(error));
            }
            let value = mapping.next_value_seed(ValueSeed {
                state: self.state,
                depth: self.depth + 1,
            })?;
            fields.insert(key, value);
        }
        Ok(Value::Object(fields))
    }
}

/// Select how repeated object members affect a bounded decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DuplicateKeys {
    Reject,
    LastWins,
}

/// Read containers once and let serde_json decode each scalar token.
///
/// Dispatch uses source bytes, not object keys, so optional JSON number features
/// cannot turn numbers into objects or reinterpret literal marker-key objects.
struct JsonReader<'a, F> {
    remaining: &'a [u8],
    counter: NodeCounter,
    duplicate_keys: DuplicateKeys,
    before_value: F,
}

impl<F: FnMut(usize) -> bool> JsonReader<'_, F> {
    fn peek(&mut self) -> Option<u8> {
        while matches!(self.remaining.first(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.remaining = &self.remaining[1..];
        }
        self.remaining.first().copied()
    }

    fn take(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.remaining = &self.remaining[1..];
            true
        } else {
            false
        }
    }

    fn require(&mut self, byte: u8) -> Result<(), BoundedJsonError> {
        if self.take(byte) {
            Ok(())
        } else {
            Err(BoundedJsonError::InvalidJson(
                "Invalid JSON container separator.".into(),
            ))
        }
    }

    fn scalar<T: serde::de::DeserializeOwned>(&mut self) -> Result<T, BoundedJsonError> {
        let mut stream = serde_json::Deserializer::from_slice(self.remaining).into_iter::<T>();
        let value = stream
            .next()
            .ok_or_else(|| BoundedJsonError::InvalidJson("Expected a JSON value.".into()))?
            .map_err(|error| BoundedJsonError::InvalidJson(error.to_string()))?;
        self.remaining = &self.remaining[stream.byte_offset()..];
        Ok(value)
    }

    /// Keep the existing integer ranges, signed zero, and finite-f64 fallback.
    fn number(&mut self) -> Result<Value, BoundedJsonError> {
        let input = self.remaining;
        let number: Number = self.scalar()?;
        let consumed = input.len() - self.remaining.len();
        if &input[..consumed] != b"-0" {
            if let Some(value) = number.as_u64() {
                return Ok(Value::Number(value.into()));
            }
            if let Some(value) = number.as_i64() {
                return Ok(Value::Number(value.into()));
            }
        }
        number
            .as_f64()
            .and_then(Number::from_f64)
            .map(Value::Number)
            .ok_or_else(|| BoundedJsonError::InvalidJson("JSON numbers must be finite.".into()))
    }

    fn value(&mut self, depth: usize) -> Result<Value, BoundedJsonError> {
        self.counter.charge(depth)?;
        if !(self.before_value)(depth) {
            return Err(BoundedJsonError::InvalidJson(
                "JSON value admission rejected.".into(),
            ));
        }
        let token = self.peek();
        // Keep serde_json's existing default ceiling for nested containers.
        if depth >= 128 && matches!(token, Some(b'[' | b'{')) {
            return Err(BoundedJsonError::InvalidJson(
                "JSON recursion limit exceeded.".into(),
            ));
        }
        match token {
            Some(b'{') => {
                self.remaining = &self.remaining[1..];
                let mut fields = Map::new();
                if self.take(b'}') {
                    return Ok(Value::Object(fields));
                }
                loop {
                    let key: String = self.scalar()?;
                    if self.duplicate_keys == DuplicateKeys::Reject && fields.contains_key(&key) {
                        return Err(BoundedJsonError::DuplicateKey(key));
                    }
                    self.require(b':')?;
                    let value = self.value(depth + 1)?;
                    fields.insert(key, value);
                    if self.take(b'}') {
                        return Ok(Value::Object(fields));
                    }
                    self.require(b',')?;
                }
            }
            Some(b'[') => {
                self.remaining = &self.remaining[1..];
                let mut values = Vec::new();
                if self.take(b']') {
                    return Ok(Value::Array(values));
                }
                loop {
                    if self.peek().is_none() {
                        return Err(BoundedJsonError::InvalidJson(
                            "Unexpected end of JSON array.".into(),
                        ));
                    }
                    values.push(self.value(depth + 1)?);
                    if self.take(b']') {
                        return Ok(Value::Array(values));
                    }
                    self.require(b',')?;
                    if self.peek() == Some(b']') {
                        return Err(BoundedJsonError::InvalidJson(
                            "JSON arrays cannot end with a comma.".into(),
                        ));
                    }
                }
            }
            Some(b'"') => self.scalar().map(Value::String),
            Some(b't' | b'f') => self.scalar().map(Value::Bool),
            Some(b'n') => self.scalar::<()>().map(|()| Value::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => Err(BoundedJsonError::InvalidJson(
                "Expected a JSON value.".into(),
            )),
        }
    }
}

/// Counts the root as depth one and each JSON value as one node. Object keys do
/// not add nodes. The decoder checks a node before it allocates that value.
pub fn decode_strict_json(bytes: &[u8], limits: JsonLimits) -> Result<Value, BoundedJsonError> {
    decode_json_with(bytes, limits, DuplicateKeys::Reject, |_| true)
}

/// Decode bounded JSON with a duplicate-key policy and per-value admission.
///
/// Each value first consumes its JSON node/depth limits, then calls
/// `before_value` before decoding or allocating that value. Depth starts at one;
/// object keys do not call the callback. The caller must retain its own error
/// state when returning false. `LastWins` still parses and charges every value.
pub(crate) fn decode_json_with(
    bytes: &[u8],
    limits: JsonLimits,
    duplicate_keys: DuplicateKeys,
    before_value: impl FnMut(usize) -> bool,
) -> Result<Value, BoundedJsonError> {
    let observed = u64::try_from(bytes.len()).map_err(|_| BoundedJsonError::Bytes {
        observed: u64::MAX,
        maximum: limits.max_bytes,
    })?;
    if observed > limits.max_bytes {
        return Err(BoundedJsonError::Bytes {
            observed,
            maximum: limits.max_bytes,
        });
    }
    let mut reader = JsonReader {
        remaining: bytes,
        counter: NodeCounter::new(limits),
        duplicate_keys,
        before_value,
    };
    let value = reader.value(1)?;
    if reader.peek().is_some() {
        return Err(BoundedJsonError::InvalidJson(
            "Trailing JSON characters.".into(),
        ));
    }
    Ok(value)
}

/// Recheck decoded values inside a prepared record. Raw JSON must first pass
/// through `decode_strict_json` for byte bounds and feature-independent scalars.
pub(crate) fn deserialize_strict_value<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
    limits: JsonLimits,
) -> Result<Value, D::Error> {
    let mut state = DecodeState {
        counter: NodeCounter::new(limits),
    };
    ValueSeed {
        state: &mut state,
        depth: 1,
    }
    .deserialize(deserializer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const LIMITS: JsonLimits = JsonLimits {
        max_bytes: 512,
        max_nodes: 16,
        max_depth: 8,
    };

    #[test]
    fn admission_visits_each_value_in_source_order_without_visiting_keys() {
        let mut depths = Vec::new();
        let value = decode_json_with(
            br#"{"values":[0.5,{"nested":true}]}"#,
            LIMITS,
            DuplicateKeys::Reject,
            |depth| {
                depths.push(depth);
                true
            },
        )
        .unwrap();
        assert_eq!(value, json!({"values": [0.5, {"nested": true}]}));
        assert_eq!(depths, [1, 2, 3, 3, 4]);
    }

    #[test]
    fn admission_rejection_precedes_value_decoding_and_keeps_limits() {
        let closed = BoundedJsonError::InvalidJson("JSON value admission rejected.".into());
        for bytes in [b"null".as_slice(), b"{invalid", b"[invalid"] {
            let mut depths = Vec::new();
            assert_eq!(
                decode_json_with(bytes, LIMITS, DuplicateKeys::Reject, |depth| {
                    depths.push(depth);
                    false
                }),
                Err(closed.clone())
            );
            assert_eq!(depths, [1]);
        }
        let mut depths = Vec::new();
        assert_eq!(
            decode_json_with(
                br#"{"value":invalid}"#,
                LIMITS,
                DuplicateKeys::Reject,
                |depth| {
                    depths.push(depth);
                    depth == 1
                }
            ),
            Err(closed)
        );
        assert_eq!(depths, [1, 2]);

        depths.clear();
        assert!(matches!(
            decode_json_with(
                b"[null]",
                JsonLimits {
                    max_nodes: 1,
                    ..LIMITS
                },
                DuplicateKeys::Reject,
                |depth| {
                    depths.push(depth);
                    true
                }
            ),
            Err(BoundedJsonError::Nodes {
                observed: 2,
                maximum: 1
            })
        ));
        assert_eq!(depths, [1]);
    }

    #[test]
    fn last_wins_parses_and_charges_overwritten_values() {
        let bytes = br#"{"same":0.5,"\u0073ame":[0.2]}"#;
        let mut depths = Vec::new();
        let value = decode_json_with(bytes, LIMITS, DuplicateKeys::LastWins, |depth| {
            depths.push(depth);
            true
        })
        .unwrap();
        assert_eq!(value, json!({"same": [0.2]}));
        assert_eq!(depths, [1, 2, 2, 3]);
        assert!(matches!(
            decode_json_with(
                bytes,
                JsonLimits {
                    max_nodes: 3,
                    ..LIMITS
                },
                DuplicateKeys::LastWins,
                |_| true
            ),
            Err(BoundedJsonError::Nodes {
                observed: 4,
                maximum: 3
            })
        ));
        assert!(matches!(
            decode_json_with(
                bytes,
                JsonLimits {
                    max_depth: 2,
                    ..LIMITS
                },
                DuplicateKeys::LastWins,
                |_| true
            ),
            Err(BoundedJsonError::Depth {
                observed: 3,
                maximum: 2
            })
        ));

        let invalid = br#"{"same":0.5,"same":invalid}"#;
        depths.clear();
        assert!(matches!(
            decode_json_with(invalid, LIMITS, DuplicateKeys::Reject, |depth| {
                depths.push(depth);
                true
            }),
            Err(BoundedJsonError::DuplicateKey(key)) if key == "same"
        ));
        assert_eq!(depths, [1, 2]);
        depths.clear();
        assert!(matches!(
            decode_json_with(invalid, LIMITS, DuplicateKeys::LastWins, |depth| {
                depths.push(depth);
                true
            }),
            Err(BoundedJsonError::InvalidJson(_))
        ));
        assert_eq!(depths, [1, 2, 2]);
    }
}
