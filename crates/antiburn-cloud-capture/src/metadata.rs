//! Common metadata sections for file and SQLite native captures.
//!
//! Sections add bounded producer claims without changing native stream bytes.
//! Their data accepts any JSON value. Unknown envelope fields survive decoding,
//! but cannot replace reserved fields or grant access to another source.
//! [`crate::CaptureMetadata::stable_fingerprint`] includes section content except
//! for typed observation times. Keep the full envelope frozen during retries.
//!
//! Decode sidecars as [`crate::CaptureMetadata`]. The typed section decoder keeps
//! object keys literal; generic `serde_json::Value` decoding can treat library
//! marker keys as type tags.

use crate::contract::{digest, encode_bounded, safe_id};
use crate::{CapturedStream, Error, Result};
use serde::de::{DeserializeOwned, MapAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::{Value, value::RawValue};
use std::collections::BTreeMap;
use std::fmt;

const SECTION_BYTES: u64 = 16 * 1024;
const SECTIONS_BYTES: u64 = 32 * 1024;
const MAX_UNIX_MILLIS: u64 = 253_402_300_799_999;
const MAX_JSON_DEPTH: usize = 128;

/// Optional sections keyed by names such as `example.session-billing`.
///
/// At most eight sections may use 32 KiB of compact UTF-8 JSON in total. Each
/// section is limited to 16 KiB, including unknown fields and its JSON data.
pub type MetadataSections = BTreeMap<String, MetadataSection>;

/// A bounded producer claim with its observation boundary and typed evidence.
///
/// Unknown fields round-trip within the same size bounds. They do not authorize
/// extra collection. The host owns schema review, consent, and minimal data
/// collection; validation cannot establish that a producer's claim is true.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "MetadataObject")]
pub struct MetadataSection {
    /// Identifies the producer's data contract, not a schema to fetch or execute.
    pub schema: String,
    /// Attributes this section to its producer, not the native session runtime.
    pub producer: MetadataProducer,
    /// Limits the claim to a native position or a capture-time observation.
    pub observation: MetadataObservation,
    /// One through eight items that bind native bytes or this section's data.
    pub evidence: Vec<MetadataEvidence>,
    /// Producer-defined JSON: an object, array, scalar, or null.
    pub data: Value,
    /// Preserved extension fields that must not collide with declared fields.
    #[serde(flatten)]
    pub unknown_fields: BTreeMap<String, Value>,
}

/// Producer attribution, not evidence of a historical native runtime version.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "MetadataObject")]
pub struct MetadataProducer {
    /// Bounded ASCII identifier for the code or host that creates the section.
    pub name: String,
    /// Dotted ASCII identifier for that producer's version.
    pub version: String,
    /// Preserved fields other than `name` and `version`, under the section bound.
    #[serde(flatten)]
    pub unknown_fields: BTreeMap<String, Value>,
}

/// When or where the producer observed section data.
///
/// Source positions must be strictly inside a native stream and have matching
/// digest evidence. Observation times require local evidence and do not prove
/// historical session state. Freeze this envelope with the section for retries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", try_from = "MetadataObject")]
pub enum MetadataObservation {
    /// Binds the claim to a byte position in an exact native stream.
    SourcePosition {
        /// Native stream identifier, not a file path or remote location.
        stream_id: String,
        /// Zero-based expanded byte offset, strictly less than the stream size.
        expanded_offset: u64,
        /// Preserved fields that cannot replace this variant's declared fields.
        #[serde(flatten)]
        unknown_fields: BTreeMap<String, Value>,
    },
    /// Records observation time without claiming historical session state.
    ObservedAt {
        /// Positive Unix milliseconds through `253_402_300_799_999`.
        ///
        /// Only this typed time field is excluded from the stable fingerprint.
        unix_millis: u64,
        /// Preserved fields that remain part of the stable fingerprint.
        #[serde(flatten)]
        unknown_fields: BTreeMap<String, Value>,
    },
}

/// Digest evidence for native framing or a capture-time local observation.
///
/// A local observation binds compact JSON data, not a historical native fact.
/// Neither evidence kind grants access to any referenced source.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", try_from = "MetadataObject")]
pub enum MetadataEvidence {
    /// Binds a claim to a full expanded native stream, not generated metadata.
    NativeStream {
        /// Exact stream identifier used by its native descriptor.
        stream_id: String,
        /// Lowercase SHA-256 of all expanded bytes in that stream.
        expanded_sha256: String,
        /// Preserved fields that cannot replace this variant's declared fields.
        #[serde(flatten)]
        unknown_fields: BTreeMap<String, Value>,
    },
    /// Binds locally observed data without proving a historical native fact.
    LocalObservation {
        /// Bounded ASCII identifier for the observation method.
        method: String,
        /// Lowercase SHA-256 of the crate's compact JSON encoding of `data`.
        data_sha256: String,
        /// Preserved fields that cannot replace this variant's declared fields.
        #[serde(flatten)]
        unknown_fields: BTreeMap<String, Value>,
    },
}

/// Read envelope fields without Serde's flattened-value buffer changing JSON.
struct MetadataObject(BTreeMap<String, Box<RawValue>>);

impl<'de> Deserialize<'de> for MetadataObject {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct ObjectVisitor;

        impl<'de> Visitor<'de> for ObjectVisitor {
            type Value = MetadataObject;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a metadata object")
            }

            fn visit_map<A: MapAccess<'de>>(
                self,
                mut input: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut fields = BTreeMap::new();
                while let Some(key) = input.next_key::<String>()? {
                    if fields.insert(key, input.next_value()?).is_some() {
                        return Err(serde::de::Error::custom("duplicate metadata field"));
                    }
                }
                Ok(MetadataObject(fields))
            }
        }

        deserializer.deserialize_map(ObjectVisitor)
    }
}

impl MetadataObject {
    /// Consume a required raw field before collecting extensions.
    fn take(&mut self, name: &'static str) -> serde_json::Result<Box<RawValue>> {
        self.0
            .remove(name)
            .ok_or_else(|| serde::de::Error::missing_field(name))
    }

    /// Decode a declared field through its normal typed JSON contract.
    fn required<T: DeserializeOwned>(&mut self, name: &'static str) -> serde_json::Result<T> {
        serde_json::from_str(self.take(name)?.get())
    }

    /// Keep all remaining field names and literal JSON values.
    fn extensions(self) -> serde_json::Result<BTreeMap<String, Value>> {
        self.0
            .into_iter()
            .map(|(key, raw)| Ok((key, decode_raw_value(&raw, 0)?)))
            .collect()
    }
}

/// Decode section data without interpreting library-private object keys.
impl TryFrom<MetadataObject> for MetadataSection {
    type Error = serde_json::Error;

    fn try_from(mut fields: MetadataObject) -> serde_json::Result<Self> {
        Ok(Self {
            schema: fields.required("schema")?,
            producer: fields.required("producer")?,
            observation: fields.required("observation")?,
            evidence: fields.required("evidence")?,
            data: decode_raw_value(&fields.take("data")?, 0)?,
            unknown_fields: fields.extensions()?,
        })
    }
}

/// Retain literal producer extensions beside the required attribution.
impl TryFrom<MetadataObject> for MetadataProducer {
    type Error = serde_json::Error;

    fn try_from(mut fields: MetadataObject) -> serde_json::Result<Self> {
        Ok(Self {
            name: fields.required("name")?,
            version: fields.required("version")?,
            unknown_fields: fields.extensions()?,
        })
    }
}

/// Select a known observation kind without buffering arbitrary JSON values.
impl TryFrom<MetadataObject> for MetadataObservation {
    type Error = serde_json::Error;

    fn try_from(mut fields: MetadataObject) -> serde_json::Result<Self> {
        match fields.required::<String>("kind")?.as_str() {
            "source_position" => Ok(Self::SourcePosition {
                stream_id: fields.required("stream_id")?,
                expanded_offset: fields.required("expanded_offset")?,
                unknown_fields: fields.extensions()?,
            }),
            "observed_at" => Ok(Self::ObservedAt {
                unix_millis: fields.required("unix_millis")?,
                unknown_fields: fields.extensions()?,
            }),
            _ => Err(serde::de::Error::custom(
                "invalid metadata observation kind",
            )),
        }
    }
}

/// Select a known evidence kind and retain its literal extension values.
impl TryFrom<MetadataObject> for MetadataEvidence {
    type Error = serde_json::Error;

    fn try_from(mut fields: MetadataObject) -> serde_json::Result<Self> {
        match fields.required::<String>("kind")?.as_str() {
            "native_stream" => Ok(Self::NativeStream {
                stream_id: fields.required("stream_id")?,
                expanded_sha256: fields.required("expanded_sha256")?,
                unknown_fields: fields.extensions()?,
            }),
            "local_observation" => Ok(Self::LocalObservation {
                method: fields.required("method")?,
                data_sha256: fields.required("data_sha256")?,
                unknown_fields: fields.extensions()?,
            }),
            _ => Err(serde::de::Error::custom("invalid metadata evidence kind")),
        }
    }
}

/// Decode bounded metadata without treating object keys as library type tags.
///
/// The caller bounds input and encoded output with the existing metadata budget.
/// Standard JSON deserializers read each raw node. The tree walk retains exact
/// numbers, uses decoded object keys, and limits nesting before recursive calls.
pub(crate) fn decode_metadata_value(bytes: &[u8]) -> serde_json::Result<Value> {
    let raw: &RawValue = serde_json::from_slice(bytes)?;
    decode_raw_value(raw, 0)
}

/// Read containers and typed scalars without `Value::deserialize` type aliases.
fn decode_raw_value(raw: &RawValue, depth: usize) -> serde_json::Result<Value> {
    if depth >= MAX_JSON_DEPTH {
        return Err(serde::de::Error::custom(
            "metadata JSON nesting limit exceeded",
        ));
    }
    let text = raw.get().trim_start();
    match text.as_bytes()[0] {
        b'{' => {
            let fields: BTreeMap<String, Box<RawValue>> = serde_json::from_str(text)?;
            fields
                .into_iter()
                .map(|(key, value)| Ok((key, decode_raw_value(&value, depth + 1)?)))
                .collect::<serde_json::Result<serde_json::Map<String, Value>>>()
                .map(Value::Object)
        }
        b'[' => {
            let items: Vec<Box<RawValue>> = serde_json::from_str(text)?;
            items
                .iter()
                .map(|value| decode_raw_value(value, depth + 1))
                .collect::<serde_json::Result<Vec<Value>>>()
                .map(Value::Array)
        }
        b'"' => serde_json::from_str(text).map(Value::String),
        b't' | b'f' => serde_json::from_str(text).map(Value::Bool),
        b'n' => Ok(Value::Null),
        _ => serde_json::from_str(text).map(Value::Number),
    }
}

/// Validate names, bounded compact JSON, reserved fields, and typed envelopes.
///
/// Size, count, or nesting overflow returns [`Error::Limit`]; invalid fields return
/// [`Error::InvalidMetadata`]. This checks local data digests but cannot bind
/// native evidence without stream descriptors; use [`validate_section_bindings`].
pub fn validate_sections(sections: &MetadataSections) -> Result<()> {
    if sections.len() > 8 {
        return Err(Error::Limit);
    }
    for (key, section) in sections {
        if !dotted_id(key) || key.split('.').count() < 2 {
            return Err(Error::InvalidMetadata);
        }
        encode_bounded(section, SECTION_BYTES)?;
        validate_section(section)?;
    }
    let bytes = encode_bounded(sections, SECTIONS_BYTES)?;
    let raw = serde_json::from_slice(&bytes)?;
    // The full fingerprint decoder also counts the enclosing metadata object.
    decode_raw_value(raw, 1).map_err(|_| Error::Limit)?;
    Ok(())
}

/// Check envelope fields and require evidence for the observation kind.
///
/// Local evidence hashes only `data`; native digest and offset checks also need
/// stream descriptors. Unknown fields consume the same serialized size budget.
fn validate_section(section: &MetadataSection) -> Result<()> {
    if !dotted_id(&section.schema)
        || !safe_id(&section.producer.name)
        || !dotted_id(&section.producer.version)
        || section.evidence.is_empty()
    {
        return Err(Error::InvalidMetadata);
    }
    if section.evidence.len() > 8 {
        return Err(Error::Limit);
    }
    no_reserved_fields(
        &section.unknown_fields,
        &["schema", "producer", "observation", "evidence", "data"],
    )?;
    no_reserved_fields(&section.producer.unknown_fields, &["name", "version"])?;
    let data_digest = digest(&encode_bounded(&section.data, SECTION_BYTES)?);
    for evidence in &section.evidence {
        match evidence {
            MetadataEvidence::NativeStream {
                stream_id,
                expanded_sha256,
                unknown_fields,
            } => {
                if !safe_id(stream_id) || !sha256_id(expanded_sha256) {
                    return Err(Error::InvalidMetadata);
                }
                no_reserved_fields(unknown_fields, &["kind", "stream_id", "expanded_sha256"])?;
            }
            MetadataEvidence::LocalObservation {
                method,
                data_sha256,
                unknown_fields,
            } => {
                if !safe_id(method) || !sha256_id(data_sha256) || *data_sha256 != data_digest {
                    return Err(Error::InvalidMetadata);
                }
                no_reserved_fields(unknown_fields, &["kind", "method", "data_sha256"])?;
            }
        }
    }
    match &section.observation {
        MetadataObservation::SourcePosition {
            stream_id,
            unknown_fields,
            ..
        } => {
            if !safe_id(stream_id)
                || !section.evidence.iter().any(|evidence| {
                    matches!(evidence, MetadataEvidence::NativeStream { stream_id: id, .. } if id == stream_id)
                })
            {
                return Err(Error::InvalidMetadata);
            }
            no_reserved_fields(unknown_fields, &["kind", "stream_id", "expanded_offset"])?;
        }
        MetadataObservation::ObservedAt {
            unix_millis,
            unknown_fields,
        } => {
            if !(1..=MAX_UNIX_MILLIS).contains(unix_millis)
                || !section
                    .evidence
                    .iter()
                    .any(|evidence| matches!(evidence, MetadataEvidence::LocalObservation { .. }))
            {
                return Err(Error::InvalidMetadata);
            }
            no_reserved_fields(unknown_fields, &["kind", "unix_millis"])?;
        }
    }
    Ok(())
}

/// Validate sections against unique native stream descriptors and offsets.
///
/// Generated metadata cannot serve as native evidence. The caller must verify
/// that descriptors match actual expanded bytes; this function checks the
/// supplied descriptors, not file content or the truth of a producer claim.
pub fn validate_section_bindings(
    sections: &MetadataSections,
    streams: &[CapturedStream],
) -> Result<()> {
    validate_sections(sections)?;
    for section in sections.values() {
        for evidence in &section.evidence {
            if let MetadataEvidence::NativeStream {
                stream_id,
                expanded_sha256,
                ..
            } = evidence
            {
                let stream = native_stream(streams, stream_id)?;
                if stream.expanded_sha256 != *expanded_sha256 {
                    return Err(Error::InvalidMetadata);
                }
            }
        }
        if let MetadataObservation::SourcePosition {
            stream_id,
            expanded_offset,
            ..
        } = &section.observation
        {
            let stream = native_stream(streams, stream_id)?;
            if *expanded_offset >= stream.expanded_bytes {
                return Err(Error::InvalidMetadata);
            }
        }
    }
    Ok(())
}

/// Resolve one unique native descriptor and reject generated metadata evidence.
fn native_stream<'a>(streams: &'a [CapturedStream], id: &str) -> Result<&'a CapturedStream> {
    let mut matches = streams.iter().filter(|stream| stream.stream_id == id);
    let stream = matches.next().ok_or(Error::InvalidMetadata)?;
    if matches.next().is_some()
        || !matches!(
            stream.role.as_str(),
            "native_transcript" | "native_companion" | "native_rows"
        )
    {
        return Err(Error::InvalidMetadata);
    }
    Ok(stream)
}

/// Accept bounded dot-separated ASCII identifiers without empty segments.
fn dotted_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && value.split('.').all(safe_id)
}

/// Require the lowercase wire form before comparing a SHA-256 digest.
fn sha256_id(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Prevent flattened extensions from serializing a second declared field.
fn no_reserved_fields(fields: &BTreeMap<String, Value>, reserved: &[&str]) -> Result<()> {
    if reserved.iter().any(|key| fields.contains_key(*key)) {
        return Err(Error::InvalidMetadata);
    }
    Ok(())
}
