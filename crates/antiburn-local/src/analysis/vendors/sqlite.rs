// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Generic SQLite adapter — the schema-agnostic fallback for DB-backed agents.
//!
//! Rather than hard-code each vendor's evolving schema, this adapter walks
//! every table and feeds any text cell that looks like a JSON object through
//! the shared record parser. That recovers message/usage/tool records wherever
//! the agent stores them, with zero schema knowledge — a deliberate v1 trade
//! that keeps coverage broad. A vendor can graduate to a schema-aware query
//! later without touching the engine.
//!
//! Codex and OpenCode have since done exactly that ([`super::codex`],
//! [`super::opencode`]). This walk now serves only as OpenCode's fallback when a
//! raw [`RawSource::Sqlite`] is handed in instead of the discovery layer's
//! synthesized `message`/`part` JSONL.

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::path::Path;

use super::jsonl::{parse_jsonl, parse_record};
use crate::analysis::interface::{RawSource, SessionInput, VendorAdapter};
use crate::analysis::model::{NormalizedEvent, NormalizedSession};

pub struct SqliteAdapter {
    pub agent: &'static str,
}

impl VendorAdapter for SqliteAdapter {
    fn agent(&self) -> &'static str {
        self.agent
    }

    fn normalize(&self, input: &SessionInput) -> anyhow::Result<NormalizedSession> {
        let events = match &input.source {
            RawSource::Sqlite(path) => extract_sqlite(path)?,
            RawSource::Jsonl(content) => parse_jsonl(content),
            RawSource::File(path) => {
                // Could be a DB or a JSONL export; try SQLite first.
                extract_sqlite(path).unwrap_or_else(|_| {
                    parse_jsonl(&std::fs::read_to_string(path).unwrap_or_default())
                })
            }
        };
        Ok(NormalizedSession {
            agent: input.agent.clone(),
            session_id: input.session_id.clone(),
            events,
            context_window: None,
            model: None,
        })
    }
}

fn extract_sqlite(path: &Path) -> anyhow::Result<Vec<NormalizedEvent>> {
    // Read-only so we never disturb a live session's database.
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;

    let tables: Vec<String> = {
        let mut stmt = conn.prepare("SELECT name FROM sqlite_master WHERE type = 'table'")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.filter_map(Result::ok).collect()
    };

    let mut events = Vec::new();
    for table in tables {
        let sql = format!("SELECT * FROM \"{table}\"");
        let Ok(mut stmt) = conn.prepare(&sql) else {
            continue;
        };
        let col_count = stmt.column_count();
        let Ok(mut rows) = stmt.query([]) else {
            continue;
        };
        while let Ok(Some(row)) = rows.next() {
            for i in 0..col_count {
                if let Ok(ValueRef::Text(bytes)) = row.get_ref(i)
                    && let Ok(text) = std::str::from_utf8(bytes)
                    && text.trim_start().starts_with('{')
                    && let Ok(value) = serde_json::from_str::<Value>(text)
                    && let Some(ev) = parse_record(&value)
                {
                    events.push(ev);
                }
            }
        }
    }
    Ok(events)
}
