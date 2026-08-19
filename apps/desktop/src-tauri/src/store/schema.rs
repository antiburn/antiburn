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
pub const MIGRATIONS: &[&str] = &[V1, V2, V3];

/// v1 — sessions, derived analysis, relations, settings, sources.
///
/// # Data policy (schema-level contract)
///
/// This is app-controlled, on-device storage. The schema may retain session
/// content as well as derived values when a visibility or analysis feature
/// needs it. Keeping a local copy never transfers ownership of the source:
/// provider files are not modified or deleted.
///
/// The v1 schema below stores normalized identity, provider-file locations,
/// derived metrics, a session title, and capped skill descriptions. That is its
/// current shape, not a prohibition on future migrations storing messages,
/// tool activity, or file content recorded in a transcript. Any such migration
/// must still be deliberate, bounded, covered by the local-data clear/delete
/// paths, and must not create a network or logging path for the content.
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
    -- Where the provider's source transcript lives.
    source_kind      TEXT NOT NULL,
    source_label     TEXT NOT NULL,
    wsl_distro       TEXT,
    -- The session's own title. A short derived excerpt when `title_source` is
    -- 'firstMessage' (the engine caps it at 200 characters); the vendor's own
    -- string otherwise. See the data policy above.
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
    -- counts and distributions, plus each skill's capped one-line description
    -- (see the data policy above).
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
-- short display label.
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

/// v2 — the record of which operating-system-protected directories the user
/// granted access to.
///
/// This is a record of the user's *decisions*, not of the operating system's
/// state: the system is authoritative and can revoke a grant at any time
/// without telling the application. A row here means "the user allowed this and
/// a read succeeded at the time"; a read that later comes back denied drops the
/// row again. The application never probes to fill this table, because probing
/// without a recorded decision is precisely what raises the consent dialog.
///
/// One row per protected directory name (`Documents`, `Desktop`, `Downloads`),
/// not per path: the operating system grants access at that granularity, so
/// storing paths would imply a precision the grant does not have.
const V2: &str = r#"
CREATE TABLE consent_grant (
    dir_name   TEXT PRIMARY KEY,
    granted_at TEXT NOT NULL
) STRICT;
"#;

/// v3 — the anonymised application-event queue (D-026, deviations D-28).
///
/// # Data policy (schema-level contract)
///
/// Both tables below are the one exception to the rule that nothing derived
/// from a reader's work leaves this machine, and they are shaped so the
/// exception cannot quietly widen. `usage_analytics_event.payload` carries only the
/// fields `usage_analytics::event` names — app version, operating system,
/// architecture, a rotating installation identifier, an event name, and
/// bucketed counts. A migration that lets a path, a repository name, a title,
/// a credential, or an unbucketed count into this table would break the
/// governance record that permits the table to exist at all.
///
/// `attempts` is what bounds the queue: a row that cannot be delivered is
/// dropped rather than retried forever, because an unbounded queue on a
/// machine that is offline for a week is a disk-space bug wearing a feature's
/// clothes. Opting out deletes every row in both tables.
const V3: &str = r#"
CREATE TABLE usage_analytics_event (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    name      TEXT NOT NULL,
    payload   TEXT NOT NULL,
    queued_at TEXT NOT NULL,
    attempts  INTEGER NOT NULL DEFAULT 0
) STRICT;

-- One row, ever. The identifier is random, is not derived from any machine
-- fact, and is rotated on a schedule so events cannot be joined into a
-- longitudinal profile.
CREATE TABLE usage_analytics_identity (
    id         INTEGER PRIMARY KEY CHECK (id = 1),
    install_id TEXT NOT NULL,
    minted_at  TEXT NOT NULL
) STRICT;
"#;
