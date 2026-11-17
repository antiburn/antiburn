use crate::contract::{digest, encode_bounded, safe_id};
use crate::{CapturedStream, Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

const SECTION_BYTES: u64 = 16 * 1024;
const SECTIONS_BYTES: u64 = 32 * 1024;
const MAX_UNIX_MILLIS: u64 = 253_402_300_799_999;

pub type MetadataSections = BTreeMap<String, MetadataSection>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetadataSection {
    pub schema: String,
    pub producer: MetadataProducer,
    pub observation: MetadataObservation,
    pub evidence: Vec<MetadataEvidence>,
    pub data: Value,
    #[serde(flatten)]
    pub unknown_fields: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetadataProducer {
    pub name: String,
    pub version: String,
    #[serde(flatten)]
    pub unknown_fields: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MetadataObservation {
    SourcePosition {
        stream_id: String,
        expanded_offset: u64,
        #[serde(flatten)]
        unknown_fields: BTreeMap<String, Value>,
    },
    ObservedAt {
        unix_millis: u64,
        #[serde(flatten)]
        unknown_fields: BTreeMap<String, Value>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MetadataEvidence {
    NativeStream {
        stream_id: String,
        expanded_sha256: String,
        #[serde(flatten)]
        unknown_fields: BTreeMap<String, Value>,
    },
    LocalObservation {
        method: String,
        data_sha256: String,
        #[serde(flatten)]
        unknown_fields: BTreeMap<String, Value>,
    },
}

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
    encode_bounded(sections, SECTIONS_BYTES)?;
    Ok(())
}

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

fn native_stream<'a>(streams: &'a [CapturedStream], id: &str) -> Result<&'a CapturedStream> {
    let mut matches = streams.iter().filter(|stream| stream.stream_id == id);
    let stream = matches.next().ok_or(Error::InvalidMetadata)?;
    if matches.next().is_some()
        || !matches!(
            stream.role.as_str(),
            "native_transcript" | "native_companion"
        )
    {
        return Err(Error::InvalidMetadata);
    }
    Ok(stream)
}

fn dotted_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && value.split('.').all(safe_id)
}

fn sha256_id(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn no_reserved_fields(fields: &BTreeMap<String, Value>, reserved: &[&str]) -> Result<()> {
    if reserved.iter().any(|key| fields.contains_key(*key)) {
        return Err(Error::InvalidMetadata);
    }
    Ok(())
}
