use std::io::{self, Write};

use serde::Serialize;
use serde::ser::{
    self, SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple,
    SerializeTupleStruct, SerializeTupleVariant,
};

use super::{BoundedJsonError, JsonLimits, JsonMeasurement, NodeCounter};

struct ByteCounter {
    observed: u64,
    maximum: u64,
}

impl Write for ByteCounter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.observed = self
            .observed
            .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        if self.observed > self.maximum {
            return Err(io::Error::other("JSON byte limit reached"));
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Measures serialized JSON without creating a JSON value, body, or payload copy.
/// Both passes stop at their limit. Root depth is one; keys do not add nodes.
pub fn measure_json<T: Serialize + ?Sized>(
    value: &T,
    limits: JsonLimits,
) -> Result<JsonMeasurement, BoundedJsonError> {
    let mut counter = NodeCounter::new(limits);
    value.serialize(NodeSerializer {
        counter: &mut counter,
        depth: 1,
    })?;
    let mut bytes = ByteCounter {
        observed: 0,
        maximum: limits.max_bytes,
    };
    if let Err(error) = serde_json::to_writer(&mut bytes, value) {
        return Err(if bytes.observed > limits.max_bytes {
            BoundedJsonError::Bytes {
                observed: bytes.observed,
                maximum: limits.max_bytes,
            }
        } else {
            BoundedJsonError::Serialization(error.to_string())
        });
    }
    Ok(JsonMeasurement {
        bytes: bytes.observed,
        nodes: counter.nodes,
        depth: counter.depth,
    })
}

struct NodeSerializer<'a> {
    counter: &'a mut NodeCounter,
    depth: usize,
}

impl<'a> NodeSerializer<'a> {
    fn scalar(self) -> Result<(), BoundedJsonError> {
        self.counter.charge(self.depth)
    }

    fn compound(self) -> Result<Compound<'a>, BoundedJsonError> {
        self.counter.charge(self.depth)?;
        Ok(Compound {
            counter: self.counter,
            depth: self.depth,
        })
    }

    fn variant(self) -> Result<NodeSerializer<'a>, BoundedJsonError> {
        self.counter.charge(self.depth)?;
        Ok(NodeSerializer {
            counter: self.counter,
            depth: self.depth + 1,
        })
    }
}

macro_rules! scalar {
    ($($method:ident($ty:ty)),* $(,)?) => {$(
        fn $method(self, _: $ty) -> Result<(), Self::Error> { self.scalar() }
    )*};
}

impl<'a> ser::Serializer for NodeSerializer<'a> {
    type Ok = ();
    type Error = BoundedJsonError;
    type SerializeSeq = Compound<'a>;
    type SerializeTuple = Compound<'a>;
    type SerializeTupleStruct = Compound<'a>;
    type SerializeTupleVariant = Compound<'a>;
    type SerializeMap = Compound<'a>;
    type SerializeStruct = Compound<'a>;
    type SerializeStructVariant = Compound<'a>;

    scalar!(
        serialize_bool(bool),
        serialize_i8(i8),
        serialize_i16(i16),
        serialize_i32(i32),
        serialize_i64(i64),
        serialize_i128(i128),
        serialize_u8(u8),
        serialize_u16(u16),
        serialize_u32(u32),
        serialize_u64(u64),
        serialize_u128(u128),
        serialize_char(char),
        serialize_str(&str)
    );

    fn collect_str<T: std::fmt::Display + ?Sized>(self, _: &T) -> Result<(), Self::Error> {
        self.scalar()
    }

    fn serialize_f32(self, value: f32) -> Result<(), Self::Error> {
        if !value.is_finite() {
            return Err(BoundedJsonError::Serialization(
                "JSON numbers must be finite.".into(),
            ));
        }
        self.scalar()
    }
    fn serialize_f64(self, value: f64) -> Result<(), Self::Error> {
        if !value.is_finite() {
            return Err(BoundedJsonError::Serialization(
                "JSON numbers must be finite.".into(),
            ));
        }
        self.scalar()
    }
    fn serialize_bytes(self, value: &[u8]) -> Result<(), Self::Error> {
        let mut compound = self.compound()?;
        for byte in value {
            compound.value(byte)?;
        }
        Ok(())
    }
    fn serialize_none(self) -> Result<(), Self::Error> {
        self.scalar()
    }
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<(), Self::Error> {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<(), Self::Error> {
        self.scalar()
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<(), Self::Error> {
        self.scalar()
    }
    fn serialize_unit_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
    ) -> Result<(), Self::Error> {
        self.scalar()
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        value.serialize(self.variant()?)
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        self.compound()
    }
    fn serialize_tuple(self, _: usize) -> Result<Self::SerializeTuple, Self::Error> {
        self.compound()
    }
    fn serialize_tuple_struct(
        self,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        self.compound()
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        self.variant()?.compound()
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        self.compound()
    }
    fn serialize_struct(
        self,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        self.compound()
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        self.variant()?.compound()
    }
}

struct Compound<'a> {
    counter: &'a mut NodeCounter,
    depth: usize,
}

impl Compound<'_> {
    fn value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), BoundedJsonError> {
        value.serialize(NodeSerializer {
            counter: self.counter,
            depth: self.depth + 1,
        })
    }
}

macro_rules! sequence {
    ($trait:ident, $method:ident) => {
        impl $trait for Compound<'_> {
            type Ok = ();
            type Error = BoundedJsonError;
            fn $method<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
                self.value(value)
            }
            fn end(self) -> Result<(), Self::Error> {
                Ok(())
            }
        }
    };
}
sequence!(SerializeSeq, serialize_element);
sequence!(SerializeTuple, serialize_element);
sequence!(SerializeTupleStruct, serialize_field);
sequence!(SerializeTupleVariant, serialize_field);

impl SerializeMap for Compound<'_> {
    type Ok = ();
    type Error = BoundedJsonError;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, _: &T) -> Result<(), Self::Error> {
        Ok(())
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.value(value)
    }
    fn end(self) -> Result<(), Self::Error> {
        Ok(())
    }
}

impl SerializeStruct for Compound<'_> {
    type Ok = ();
    type Error = BoundedJsonError;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        _: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        self.value(value)
    }
    fn end(self) -> Result<(), Self::Error> {
        Ok(())
    }
}

impl SerializeStructVariant for Compound<'_> {
    type Ok = ();
    type Error = BoundedJsonError;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        _: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        self.value(value)
    }
    fn end(self) -> Result<(), Self::Error> {
        Ok(())
    }
}
