use rusqlite::OptionalExtension;
use std::collections::VecDeque;
use std::sync::Arc;

use crate::store::{BurnCheckCandidate, BurnCheckInput, Store};

use super::{InputLoadError, digest};

const MAX_PREPARED_INPUTS: usize = 64;
const MAX_PREPARED_BYTES: usize = 64 * 1024 * 1024;

struct Entry<T> {
    // Keep the connection alive so its identity cannot be reused.
    _store: Store,
    identity: usize,
    key: String,
    bytes: usize,
    value: Arc<T>,
}

pub(crate) struct PreparedInputCache<T> {
    entries: VecDeque<Entry<T>>,
    bytes: usize,
}

impl<T> Default for PreparedInputCache<T> {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
            bytes: 0,
        }
    }
}

pub(crate) fn store_identity(store: &Store) -> usize {
    (&*store.lock() as *const rusqlite::Connection) as usize
}

impl<T> PreparedInputCache<T> {
    pub(crate) fn get(&mut self, store: &Store, key: &str) -> Option<Arc<T>> {
        let identity = store_identity(store);
        let index = self
            .entries
            .iter()
            .position(|entry| entry.key == key && entry.identity == identity)?;
        let entry = self.entries.remove(index).expect("cache index");
        let value = Arc::clone(&entry.value);
        self.entries.push_back(entry);
        Some(value)
    }

    pub(crate) fn insert(&mut self, store: &Store, key: String, bytes: usize, value: Arc<T>) {
        if bytes > MAX_PREPARED_BYTES {
            return;
        }
        while self.entries.len() >= MAX_PREPARED_INPUTS || self.bytes + bytes > MAX_PREPARED_BYTES {
            let entry = self.entries.pop_front().expect("bounded cache entry");
            self.bytes -= entry.bytes;
        }
        self.bytes += bytes;
        self.entries.push_back(Entry {
            _store: store.clone(),
            identity: store_identity(store),
            key,
            bytes,
            value,
        });
    }
}

pub(crate) fn source_fence(
    candidate: &BurnCheckCandidate,
    check_id: &str,
    evaluator_revision: String,
) -> BurnCheckInput {
    BurnCheckInput {
        key: candidate.session.key.clone(),
        check_id: check_id.into(),
        incarnation: candidate.incarnation,
        source_generation: candidate.source_generation,
        source_fingerprint: candidate.source_fingerprint.clone(),
        activity_cursor: candidate.activity_cursor.clone(),
        published_fence: candidate.published_fence,
        input_revision: String::new(),
        evaluator_revision,
        boundary_at_epoch: candidate.boundary_at_epoch,
    }
}

pub(crate) fn source_is_current(
    store: &Store,
    candidate: &BurnCheckCandidate,
    check_id: &str,
    evaluator_revision: String,
) -> anyhow::Result<bool> {
    let fence = source_fence(candidate, check_id, evaluator_revision);
    let connection = store.lock();
    if !Store::burn_check_input_is_current(&connection, &fence)? {
        return Ok(false);
    }
    Ok(connection.query_row(
        "SELECT cwd IS ?4 AND wsl_distro IS ?5 FROM session WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
        rusqlite::params![candidate.session.key.environment_key, candidate.session.key.agent, candidate.session.key.session_id, candidate.session.cwd, candidate.session.wsl_distro],
        |row| row.get(0),
    ).optional()?.unwrap_or(false))
}

pub(crate) fn source_key(
    candidate: &BurnCheckCandidate,
    check_id: &str,
    evaluator_revision: &str,
) -> Result<String, InputLoadError> {
    digest(&serde_json::json!((
        (
            &candidate.session.key.environment_key,
            &candidate.session.key.agent,
            &candidate.session.key.session_id
        ),
        candidate.incarnation,
        candidate.source_generation,
        &candidate.source_fingerprint,
        &candidate.activity_cursor,
        candidate.published_fence,
        &candidate.boundary_positions,
        candidate.boundary_at_epoch,
        candidate.historical,
        &candidate.session.cwd,
        check_id,
        evaluator_revision,
        (
            antiburn_local::analysis::PARSER_REVISION,
            antiburn_local::analysis::ANALYZER_REVISION,
            antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION
        ),
    )))
}
