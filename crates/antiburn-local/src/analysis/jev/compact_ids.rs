//! Compact storage for SHA-256 work IDs, with legacy plain-list support.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum StoredIds {
    Packed(String),
    Plain(Vec<String>),
}

pub fn serialize<S: Serializer>(ids: &[String], serializer: S) -> Result<S::Ok, S::Error> {
    if ids
        .iter()
        .any(|id| id.len() != 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return ids.serialize(serializer);
    }
    let bytes = ids
        .iter()
        .flat_map(|id| {
            id.as_bytes().chunks_exact(2).map(|pair| {
                let hex = std::str::from_utf8(pair).expect("ASCII hex pair");
                u8::from_str_radix(hex, 16).expect("valid hex pair")
            })
        })
        .collect::<Vec<_>>();
    STANDARD.encode(bytes).serialize(serializer)
}

pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
    match StoredIds::deserialize(deserializer)? {
        StoredIds::Plain(ids) => Ok(ids),
        StoredIds::Packed(packed) => {
            let bytes = STANDARD.decode(packed).map_err(D::Error::custom)?;
            if bytes.len() % 32 != 0 {
                return Err(D::Error::custom("invalid packed comparison ids"));
            }
            Ok(bytes
                .chunks_exact(32)
                .map(|chunk| chunk.iter().map(|byte| format!("{byte:02x}")).collect())
                .collect())
        }
    }
}
