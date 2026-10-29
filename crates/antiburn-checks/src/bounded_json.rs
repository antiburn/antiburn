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
    error: Option<BoundedJsonError>,
}

struct ValueSeed<'a> {
    state: &'a mut DecodeState,
    depth: usize,
}

impl<'de> DeserializeSeed<'de> for ValueSeed<'_> {
    type Value = Value;

    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        if let Err(error) = self.state.counter.charge(self.depth) {
            let message = error.to_string();
            self.state.error = Some(error);
            return Err(serde::de::Error::custom(message));
        }
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
                let message = error.to_string();
                self.state.error = Some(error);
                return Err(serde::de::Error::custom(message));
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

/// Counts the root as depth one and each JSON value as one node. Object keys do
/// not add nodes. The decoder checks a node before it allocates that value.
pub fn decode_strict_json(bytes: &[u8], limits: JsonLimits) -> Result<Value, BoundedJsonError> {
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
    let mut state = DecodeState {
        counter: NodeCounter::new(limits),
        error: None,
    };
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = ValueSeed {
        state: &mut state,
        depth: 1,
    }
    .deserialize(&mut deserializer)
    .map_err(|error| {
        state
            .error
            .take()
            .unwrap_or_else(|| BoundedJsonError::InvalidJson(error.to_string()))
    })?;
    deserializer
        .end()
        .map_err(|error| BoundedJsonError::InvalidJson(error.to_string()))?;
    Ok(value)
}

pub(crate) fn deserialize_strict_value<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
    limits: JsonLimits,
) -> Result<Value, D::Error> {
    let mut state = DecodeState {
        counter: NodeCounter::new(limits),
        error: None,
    };
    ValueSeed {
        state: &mut state,
        depth: 1,
    }
    .deserialize(deserializer)
}
