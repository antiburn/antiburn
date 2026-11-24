//! Validate one Cursor v18 header sequence without rewriting its native rows.

use super::sqlite::{exact_row, proof_bytes};
use crate::contract::safe_id;
use crate::opencode::check_deadline;
use crate::{Error, Limits, Result};
use rusqlite::Connection;
use serde::Deserialize;
use std::collections::BTreeSet;
use std::time::Instant;

pub(super) const MAX_BUBBLES: usize = 16_384;
const MAX_CHUNKS: usize = 256;
const CHUNK_SIZE: usize = 256;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct HeaderLog {
    chunk_size: usize,
    chunk_count: usize,
    count: usize,
    generation: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Header {
    bubble_id: String,
    #[serde(rename = "type")]
    role: u8,
}

impl HeaderLog {
    /// Append exact chunk rows, then their resolved bubble rows, from one snapshot.
    pub(super) fn select_rows(
        &self,
        db: &Connection,
        locator: &str,
        composer: &str,
        limits: &Limits,
        deadline: Instant,
        rows: &mut Vec<i64>,
    ) -> Result<()> {
        if self.chunk_count > MAX_CHUNKS || self.count > MAX_BUBBLES {
            return Err(Error::Limit);
        }
        if self.chunk_size != CHUNK_SIZE
            || self.chunk_count == 0
            || self.count == 0
            || self.count > self.chunk_count * self.chunk_size
            || !safe_id(&self.generation)
        {
            return Err(Error::UnsupportedSource);
        }
        let mut seen = BTreeSet::new();
        let mut bubbles = Vec::with_capacity(self.count);
        for index in 0..self.chunk_count {
            check_deadline(deadline)?;
            let key = format!("composerConvHeaders:{composer}:{index:06}");
            let row = exact_row(db, "cursorDiskKV", locator, &key)?;
            let bytes = proof_bytes(db, "cursorDiskKV", locator, row, limits.source_bytes)?;
            let chunk: Vec<Header> =
                serde_json::from_slice(&bytes).map_err(|_| Error::UnsupportedSource)?;
            if chunk.len() > self.chunk_size || bubbles.len() + chunk.len() > self.count {
                return Err(Error::UnsupportedSource);
            }
            rows.push(row);
            for header in chunk {
                check_deadline(deadline)?;
                if !safe_id(&header.bubble_id)
                    || !matches!(header.role, 1 | 2)
                    || !seen.insert(header.bubble_id.clone())
                {
                    return Err(Error::UnsupportedSource);
                }
                let key = format!("bubbleId:{composer}:{}", header.bubble_id);
                let row = exact_row(db, "cursorDiskKV", locator, &key)?;
                let bytes = proof_bytes(db, "cursorDiskKV", locator, row, limits.source_bytes)?;
                let bubble: Header =
                    serde_json::from_slice(&bytes).map_err(|_| Error::UnsupportedSource)?;
                if bubble.bubble_id != header.bubble_id || bubble.role != header.role {
                    return Err(Error::UnsupportedSource);
                }
                bubbles.push(row);
            }
        }
        if bubbles.len() != self.count {
            return Err(Error::UnsupportedSource);
        }
        rows.extend(bubbles);
        Ok(())
    }
}
