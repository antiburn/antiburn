use std::collections::{BTreeMap, HashSet};

use rusqlite::{Connection, params};
use serde_json::Value;

const MAX_CHUNKS: u64 = 256;
const MAX_HEADERS: u64 = 16_384;
const MAX_CHUNK_BYTES: usize = 256 * 1024;
const MAX_TOTAL_BYTES: usize = 16 * 1024 * 1024;

/// Read exact chunk keys inside the caller's composer snapshot transaction.
pub(super) fn read_header_ids(
    db: &Connection,
    composer_id: &str,
    metadata: &Value,
) -> Option<Vec<String>> {
    if metadata.get("chunkSize")?.as_u64()? != 256 {
        return None;
    }
    let chunks = metadata.get("chunkCount")?.as_u64()?;
    let count = metadata.get("count")?.as_u64()?;
    let generation = metadata.get("generation")?.as_str()?;
    if !(1..=MAX_CHUNKS).contains(&chunks)
        || !(1..=MAX_HEADERS).contains(&count)
        || generation.is_empty()
        || generation.len() > crate::analysis::EVIDENCE_STRING_CAP
    {
        return None;
    }
    let mut statement = db
        .prepare("SELECT value FROM cursorDiskKV WHERE key = ?1 AND typeof(value) = 'text' AND length(CAST(value AS BLOB)) <= ?2")
        .ok()?;
    let mut ids = Vec::with_capacity(count as usize);
    let mut seen = HashSet::with_capacity(count as usize);
    let mut bytes = 0;
    for index in 0..chunks {
        let raw = statement
            .query_row(
                params![
                    format!("composerConvHeaders:{composer_id}:{index:06}"),
                    MAX_CHUNK_BYTES
                ],
                |row| row.get::<_, String>(0),
            )
            .ok()?;
        bytes += raw.len();
        if bytes > MAX_TOTAL_BYTES {
            return None;
        }
        let value = serde_json::from_str::<Value>(&raw).ok()?;
        let headers = value.as_array()?;
        if headers.len() > 256 || ids.len() + headers.len() > count as usize {
            return None;
        }
        for header in headers {
            let id = header.get("bubbleId")?.as_str()?;
            if id.is_empty()
                || id.len() > crate::analysis::EVIDENCE_STRING_CAP
                || !seen.insert(id.to_owned())
            {
                return None;
            }
            ids.push(id.to_owned());
        }
    }
    (ids.len() == count as usize).then_some(ids)
}

/// Cursor copies bubble timestamps when it forks a chat and resets the composer timestamp.
pub(super) fn bubble_ownership_proven(
    composer: &Value,
    ids: &[String],
    bubbles: &BTreeMap<String, Value>,
    now: i64,
) -> bool {
    let Some(created_at) = composer.get("createdAt").and_then(Value::as_i64) else {
        return false;
    };
    created_at > 0
        && ids.iter().all(|id| {
            bubbles
                .get(id)
                .and_then(|bubble| bubble.get("createdAt"))
                .filter(|value| {
                    value
                        .as_str()
                        .is_some_and(|text| text.len() == 24 && text.ends_with('Z'))
                })
                .and_then(crate::analysis::records::parse_ts)
                .is_some_and(|timestamp| timestamp > created_at && timestamp / 1000 <= now)
        })
}

#[cfg(test)]
mod tests;
