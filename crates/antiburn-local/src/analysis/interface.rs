// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The vendor interface layer.
//!
//! This is the seam that lets *every* agent vendor be analyzed through one
//! pipeline. Each vendor implements [`VendorAdapter`] to turn its raw
//! transcript (JSONL text, a SQLite database, …) into a
//! [`NormalizedSession`]. The engine never knows which vendor it is looking at.

use std::path::PathBuf;

use crate::analysis::model::NormalizedSession;

/// Where a session's raw bytes come from. Adapters choose how to read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawSource {
    /// Transcript content already in memory (line-delimited JSON for most agents).
    Jsonl(String),
    /// A transcript file on disk the adapter should read itself.
    File(PathBuf),
    /// A SQLite database file (Codex, OpenCode, …).
    Sqlite(PathBuf),
}

/// One unit of work handed to the analysis pipeline: a single live session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInput {
    /// Vendor label, e.g. `"claude"`, `"codex"`, `"opencode"`. Used to pick an
    /// adapter; unrecognized labels fall back to the generic JSONL adapter.
    pub agent: String,
    pub session_id: String,
    pub source: RawSource,
}

/// Implemented once per vendor format. Stateless and `Sync` so adapters can be
/// stored as `&'static dyn VendorAdapter` in the registry.
pub trait VendorAdapter: Sync {
    /// Stable label this adapter handles (for diagnostics/tests).
    fn agent(&self) -> &'static str;

    /// Parse one raw session into the normalized model. Implementations should
    /// be resilient: skip malformed records rather than failing the whole
    /// session, and only return `Err` for unreadable sources (missing file,
    /// unopenable DB).
    fn normalize(&self, input: &SessionInput) -> anyhow::Result<NormalizedSession>;
}
