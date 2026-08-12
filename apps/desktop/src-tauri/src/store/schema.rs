// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The app database's schema and its migration ladder.
//!
//! Migrations are embedded, ordered, and applied inside one transaction each.
//! The applied version is the SQLite `user_version` pragma — the database is
//! app-private and single-writer, so a pragma is the whole mechanism a
//! migration table would otherwise provide.
//!
//! Adding a migration means appending one entry to [`MIGRATIONS`]. Never edit
//! an entry that has shipped: an installed database has already run it.

/// Every migration, in order. The index of an entry plus one is the
/// `user_version` it leaves behind.
pub const MIGRATIONS: &[&str] = &[V1];

/// v1 — sessions, derived analysis, relations, settings, sources.
///
/// # Data policy (schema-level contract)
///
/// **No raw transcript content is ever stored in this database.** Every table
/// below holds normalized identity (agent, session id, environment), locations
/// (paths that point *at* the provider's own file), or values derived by the
/// engine's analysis (counts, durations, token totals, phase distributions,
/// cost estimates). The transcripts themselves stay where their vendor wrote
/// them and are re-read on demand. A column that would carry message text,
/// prompts, tool arguments, or file contents does not belong in this schema.
///
/// `session_analysis.metrics_json` is the one place that could drift, so it is
/// pinned here explicitly: it holds `antiburn_local::analysis::SessionMetrics`,
/// which is counts, timings, distributions, token totals, and skill *names* —
/// never transcript text.
const V1: &str = r#"
-- App settings, one row per key. Values are JSON scalars so a new preference
-- is additive and needs no migration.
CREATE TABLE setting (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT;

-- Extra directories the user pointed the scanner at, beyond the agents' own
-- default stores. Mirrored into the engine's `scan-roots.json` on every edit
-- (the engine owns that file's format; this table owns ordering and removal).
CREATE TABLE scan_root (
    path     TEXT PRIMARY KEY,
    added_at TEXT NOT NULL
) STRICT;

-- One discovered coding session. Identity is (environment, agent, session id):
-- agent session ids are unique only within an agent, and only within one
-- execution environment, because a WSL install may reuse its native twin's ids.
CREATE TABLE session (
    environment_key  TEXT NOT NULL,
    agent            TEXT NOT NULL,
    session_id       TEXT NOT NULL,
    -- Where the transcript lives. A REFERENCE to the provider's file, never a
    -- copy of it.
    source_kind      TEXT NOT NULL,
    source_label     TEXT NOT NULL,
    wsl_distro       TEXT,
    title            TEXT,
    title_source     TEXT,
    cwd              TEXT,
    surface          TEXT NOT NULL DEFAULT 'unknown',
    -- Transcript heartbeat (unix seconds), as discovery reported it.
    updated_at_epoch INTEGER,
    subagent_count   INTEGER NOT NULL DEFAULT 0,
    first_seen_at    TEXT NOT NULL,
    last_seen_at     TEXT NOT NULL,
    PRIMARY KEY (environment_key, agent, session_id)
) STRICT;

CREATE INDEX session_recency ON session (updated_at_epoch DESC);

-- Engine-derived analysis for one session. Rebuilt whenever the transcript's
-- fingerprint or the pricing generation changes.
CREATE TABLE session_analysis (
    environment_key    TEXT NOT NULL,
    agent              TEXT NOT NULL,
    session_id         TEXT NOT NULL,
    -- `antiburn_local::analysis::SessionMetrics` as camelCase JSON: derived
    -- counts and distributions only (see the module contract above).
    metrics_json       TEXT NOT NULL,
    -- `analysis::SessionCost` components, or NULL when nothing priced.
    cost_json          TEXT,
    -- Billable tokens per normalized model key, so cost can be re-priced from
    -- the cache after a catalog update without re-reading the transcript.
    model_breakdown_json TEXT NOT NULL,
    active_secs        INTEGER NOT NULL,
    duration_secs      INTEGER NOT NULL,
    pattern_score      INTEGER NOT NULL,
    -- Transcript fingerprint (`mtime:size`) the analysis was computed from.
    source_fingerprint TEXT NOT NULL,
    pricing_generation INTEGER NOT NULL,
    analyzed_at        TEXT NOT NULL,
    PRIMARY KEY (environment_key, agent, session_id)
) STRICT;

-- Local relationships between sessions: orchestration (a session's sub-agents)
-- and lineage (the session a fork branched from). Labels are the vendor's own
-- short display label, not transcript content.
CREATE TABLE session_relation (
    environment_key TEXT NOT NULL,
    agent           TEXT NOT NULL,
    session_id      TEXT NOT NULL,
    kind            TEXT NOT NULL,           -- 'subagent' | 'forkParent'
    related_id      TEXT NOT NULL,
    label           TEXT,
    PRIMARY KEY (environment_key, agent, session_id, kind, related_id)
) STRICT;

-- Per-agent scan bookkeeping: the cursor a future incremental scan resumes
-- from, and what the last pass saw.
CREATE TABLE scan_state (
    agent             TEXT PRIMARY KEY,
    last_completed_at TEXT,
    cursor_epoch      INTEGER,
    sessions_seen     INTEGER NOT NULL DEFAULT 0
) STRICT;

-- Repositories located on this machine, plus the user's include/ignore choice.
-- Inclusion is opt-out, matching the engine's own selection default.
CREATE TABLE repository (
    key            TEXT PRIMARY KEY,
    repo_name      TEXT NOT NULL,
    full_name      TEXT NOT NULL,
    status         TEXT NOT NULL,
    repo_root      TEXT,
    suspected_path TEXT,
    worktree_count INTEGER NOT NULL DEFAULT 0,
    session_count  INTEGER NOT NULL DEFAULT 0,
    wsl_distro     TEXT,
    enabled        INTEGER NOT NULL DEFAULT 1,
    last_seen_at   TEXT NOT NULL
) STRICT;
"#;
