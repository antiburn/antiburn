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
pub const MIGRATIONS: &[&str] = &[
    V1, V2, V3, V4, V5, V6, V7, V8, V9, V10, V11, V12, V13, V14, V15, V16, V17, V18, V19, V20, V21,
    V22, V23, V24, V25, V26, V27, V28, V29, V30, V31,
];

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
/// must still be deliberate, bounded, covered by the local-data
/// clear/delete/retention paths, and must not create a network or logging path.
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
    -- Most recent meaningful transcript activity (unix seconds), as the scan
    -- reported it. File mtime is only a fallback for sources without events.
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
    -- Billable tokens per normalized model key. The map merges the parent
    -- session and every sub-agent it launched. A later pass can re-price
    -- the session from this cache after a catalog update, with no need to
    -- read any transcript again.
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

/// v3 — drop any stored `liveUsageEnabled` row, so every install picks up the
/// flipped default.
///
/// `liveUsageEnabled` used to default to `false`. It does not merely default
/// that way for keys nobody ever touched: [`super::write_settings`] writes
/// every settings key on every save, so any install that has ever saved
/// settings at all — finishing onboarding is enough — already has an
/// explicit `liveUsageEnabled|false` row from that old default, indistinguishable
/// from a reader who deliberately switched it off. There is no way to tell
/// the two apart from the row alone.
///
/// antiburn has not shipped to a public audience yet, so there is no real
/// installed base whose deliberate opt-out this could be silently
/// overturning. Dropping the row is what lets every existing install fall
/// through to the new default (`true`) the same way a fresh install does,
/// rather than being permanently stuck on the old one. A later default
/// change, after a real public release, would need an actual migration
/// strategy instead of this one.
const V3: &str = r#"
DELETE FROM setting WHERE key = 'liveUsageEnabled';
"#;

/// v4 — activity cursor and timestamp provenance.
///
/// Existing rows are marked as mtime-derived so the next scan gets one chance
/// to replace a stale mtime-derived timestamp with one from transcript content.
const V4: &str = r#"
ALTER TABLE session ADD COLUMN activity_source TEXT NOT NULL DEFAULT 'mtime';
ALTER TABLE session ADD COLUMN activity_cursor TEXT NOT NULL DEFAULT '';
"#;

/// v5 — the anonymised application-event queue.
///
/// Numbered 4 on its own branch until this merge, where the activity migration
/// above had already taken that index on `main`. Renumbering is only safe
/// because no release ever carried the old number; a database built from the
/// branch beforehand is stamped 4 and will abort here rather than diverge,
/// which is the intended outcome and is repaired by rewinding it to 3.
///
/// # Data policy (schema-level contract)
///
/// These two tables are the one exception to the rule that nothing derived
/// from a reader's work leaves this machine. `payload` holds exactly what
/// `analytics::event::Event` serializes and nothing else; a migration
/// that let a path, a repository name, a title, a credential, or an unbucketed
/// count in here would break the governance record that permits the table to
/// exist at all.
///
/// `attempts` bounds the queue by age of failure, as `store::mod` bounds it by
/// row count. Opting out deletes every row in both tables.
const V5: &str = r#"
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

/// v6: drop every cached session analysis.
///
/// The old `session_analysis.model_breakdown_json` held only the parent
/// session's own billable tokens. The engine analyzed each sub-agent too.
/// The cached cost never included the sub-agent tokens. The activity list,
/// the provider-usage totals, and an export all showed the wrong number.
///
/// The fingerprint check alone marks an old row fresh. It cannot detect
/// this kind of change. Dropping every row forces the next scan to
/// recompute each one under the new merge rule.
const V6: &str = r#"
DELETE FROM session_analysis;
"#;

/// v7 — ordered model runs used by a session and its sub-agents.
///
/// The cost breakdown has no thinking modes or display order. The new list
/// puts parent runs before runs used only by sub-agents.
const V7: &str = r#"
ALTER TABLE session_analysis ADD COLUMN inclusive_models_json TEXT NOT NULL DEFAULT '[]';
DELETE FROM session_analysis;
"#;

/// v8 — retain only analysis cache values that the app reads.
const V8: &str = r#"
CREATE TABLE session_analysis_v8 (
    environment_key      TEXT NOT NULL,
    agent                TEXT NOT NULL,
    session_id           TEXT NOT NULL,
    model_breakdown_json TEXT NOT NULL,
    inclusive_models_json TEXT NOT NULL,
    source_fingerprint   TEXT NOT NULL,
    pricing_generation   INTEGER NOT NULL,
    PRIMARY KEY (environment_key, agent, session_id)
) STRICT;

INSERT INTO session_analysis_v8 (
    environment_key, agent, session_id, model_breakdown_json,
    inclusive_models_json, source_fingerprint, pricing_generation
)
SELECT environment_key, agent, session_id, model_breakdown_json,
       inclusive_models_json, source_fingerprint, pricing_generation
FROM session_analysis;

DROP TABLE session_analysis;
ALTER TABLE session_analysis_v8 RENAME TO session_analysis;
"#;

/// v9 — source generations and analysis projection revisions.
///
/// The source fingerprint is a derived identity value. It contains no
/// transcript content. The head hash contributes to it and is never stored
/// separately.
const V9: &str = r#"
ALTER TABLE session ADD COLUMN source_fingerprint TEXT;
ALTER TABLE session ADD COLUMN source_generation INTEGER NOT NULL DEFAULT 0;
ALTER TABLE session ADD COLUMN started_at_epoch INTEGER;
ALTER TABLE session_analysis ADD COLUMN analyzed_generation INTEGER NOT NULL DEFAULT 0;
ALTER TABLE session_analysis ADD COLUMN parser_revision INTEGER NOT NULL DEFAULT 1;
ALTER TABLE session_analysis ADD COLUMN analyzer_revision INTEGER NOT NULL DEFAULT 1;
ALTER TABLE session_analysis ADD COLUMN metrics_schema_revision INTEGER NOT NULL DEFAULT 1;
"#;

/// v10 — rename the analytics-event tables for the analytics naming change.
///
/// The code dropped the "usage_" prefix from the Rust module and its
/// identifiers. This migration renames the two tables to match, so an
/// existing database still works with the renamed code.
const V10: &str = r#"
ALTER TABLE usage_analytics_event RENAME TO analytics_event;
ALTER TABLE usage_analytics_identity RENAME TO analytics_identity;
"#;

/// v11 — durable session evidence and its work lifecycle.
///
/// # Data policy (schema-level contract)
///
/// This table stores derived facts and never stores transcript text.
/// Session deletion and clear-local-session-data remove these facts.
/// Transcript file removal does not remove these facts.
const V11: &str = r#"
CREATE TABLE session_evidence (
    environment_key TEXT NOT NULL, agent TEXT NOT NULL, session_id TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    analyzed_generation INTEGER, processed_fingerprint TEXT,
    parser_revision INTEGER, analyzer_revision INTEGER, evidence_schema_revision INTEGER,
    evidence_json TEXT, diagnostics_json TEXT,
    retry_count INTEGER NOT NULL DEFAULT 0, claim_fence INTEGER NOT NULL DEFAULT 0,
    claimed_at_epoch INTEGER, lease_expires_at_epoch INTEGER,
    next_attempt_at_epoch INTEGER, analyzed_at_epoch INTEGER, last_error TEXT,
    PRIMARY KEY (environment_key, agent, session_id),
    FOREIGN KEY (environment_key, agent, session_id)
      REFERENCES session (environment_key, agent, session_id) ON DELETE CASCADE,
    CHECK (status IN ('pending','processing','ready','unsupported','failed'))
) STRICT;
CREATE INDEX session_evidence_status
    ON session_evidence (status, next_attempt_at_epoch, lease_expires_at_epoch);
"#;

/// v12 — add existing Codex sessions to the durable evidence queue.
///
/// A normal upsert queues future Codex source generations. This migration also
/// queues rows that scans stored before Codex joined the evidence cohort.
const V12: &str = r#"
INSERT INTO session_evidence (environment_key, agent, session_id)
SELECT environment_key, agent, session_id
  FROM session
 WHERE agent = 'codex'
ON CONFLICT(environment_key, agent, session_id) DO NOTHING;
"#;

/// v13 — add existing Pi sessions to the durable evidence queue.
///
/// A normal upsert queues future Pi source generations. This migration also
/// queues rows that scans stored before Pi joined the evidence cohort.
const V13: &str = r#"
INSERT INTO session_evidence (environment_key, agent, session_id)
SELECT environment_key, agent, session_id
  FROM session
 WHERE agent = 'pi'
ON CONFLICT(environment_key, agent, session_id) DO NOTHING;
"#;

/// v14 adds existing OpenCode sessions to the durable evidence queue.
const V14: &str = r#"
INSERT INTO session_evidence (environment_key, agent, session_id)
SELECT environment_key, agent, session_id
  FROM session
 WHERE agent = 'opencode'
ON CONFLICT(environment_key, agent, session_id) DO NOTHING;
"#;

/// v15 — persisted turn rows.
///
/// # Data policy (schema-level contract)
///
/// `turn` stores one row per parsed turn: identity, thread and scope facts,
/// and token accounting derived from a transcript. `turn_content` stores the
/// turn's text: `FencedTurnRowStore::write_turn_rows` writes it for every row
/// with captured content. No production code reads it yet; a later change
/// adds the drilldown's content read.
///
/// Session deletion and clear-local-session-data remove these rows
/// explicitly, the same way [`V11`]'s `session_evidence` does — the FK
/// cascade is a backstop, not the mechanism.
///
/// `antiburn_local::analysis::TURN_SCHEMA_SQL` owns the column list; this is
/// a re-export so [`MIGRATIONS`] stays a plain `&[&str]` of literals. See
/// "Where the row logic lives" in the session-evidence-harness-parity plan:
/// the crate owns the DDL and the read/write functions over a borrowed
/// connection, and never opens this database itself.
const V15: &str = antiburn_local::analysis::TURN_SCHEMA_SQL;

/// v16 adds the three compaction columns `query_turn_facts` reads:
/// `compaction_trigger`, `compaction_pre_tokens`, `compaction_post_tokens`.
///
/// `antiburn_local::analysis::TURN_SCHEMA_V2_SQL` owns the column list, for
/// the same reason [`V15`] re-exports `TURN_SCHEMA_SQL` instead of stating
/// its own DDL.
const V16: &str = antiburn_local::analysis::TURN_SCHEMA_V2_SQL;

/// v17 adds `initial_context_json` to `session_analysis`: the serialized
/// initial-context breakdown a later change (seam R3) reads back. Nullable,
/// with no default, because an existing row's initial context is unknown
/// until the next analysis pass fills it in — unlike [`V7`]'s
/// `inclusive_models_json`, there is no empty-but-valid value to backfill.
const V17: &str = r#"
ALTER TABLE session_analysis ADD COLUMN initial_context_json TEXT;
"#;

/// v18 adds the three chart-signal columns `query_turn_rows` reads:
/// `has_thinking`, `last_tool`, `subagent_launches`.
///
/// `antiburn_local::analysis::TURN_SCHEMA_V3_SQL` owns the column list, for
/// the same reason [`V16`] re-exports `TURN_SCHEMA_V2_SQL` instead of
/// stating its own DDL.
const V18: &str = antiburn_local::analysis::TURN_SCHEMA_V3_SQL;

/// v19 adds `source_summaries_json` to `session_analysis`: each source's own
/// serialized `SessionSummary`, keyed by `source_key`, that seam R3c's
/// drilldown replay reads back to rebuild per-source metrics without a
/// transcript. Nullable, with no default, for the same reason [`V17`]'s
/// `initial_context_json` is: an existing row's per-source summaries are
/// unknown until the next worker pass fills them in. Only a pass with a
/// `turn_row_store` (the durable evidence worker) writes this column; the
/// on-demand and scan-triggered passes leave it `NULL`.
const V19: &str = r#"
ALTER TABLE session_analysis ADD COLUMN source_summaries_json TEXT;
"#;

/// v20 — add existing sessions of the newly widened evidence cohort agents to
/// the durable evidence queue.
///
/// The evidence cohort now covers every [`AgentKind`](antiburn_local::model::AgentKind):
/// Cursor, Copilot, Cline, Kiro, Amp, Antigravity, and Windsurf join Claude,
/// Codex, OpenCode, and Pi. A normal upsert queues future source generations
/// for these agents; this migration also queues rows that scans stored before
/// they joined the cohort, following the same shape as [`V12`]/[`V13`]/[`V14`].
const V20: &str = r#"
INSERT INTO session_evidence (environment_key, agent, session_id)
SELECT environment_key, agent, session_id
  FROM session
 WHERE agent IN ('cursor', 'copilot', 'cline', 'kiro', 'amp-code', 'antigravity', 'windsurf')
ON CONFLICT(environment_key, agent, session_id) DO NOTHING;
"#;

/// v21 — the fence a row set was actually published under, tracked apart
/// from `claim_fence`.
///
/// `claim_fence` moves the moment a session is reclaimed, even before the
/// new pass writes or publishes anything. `published_fence` moves only when
/// [`super::Store::publish_projections`] wins its race, so it always names a
/// complete, still-on-disk row set: a claim in flight, or a requeue back to
/// `pending`, leaves it exactly where the last winning publish put it. The
/// backfill gives every session `publish_projections` has already reached
/// its own last winning fence, the only fence whose rows are still on disk
/// for a `ready` or `unsupported` row.
///
/// Since [`V28`], `published_fence` names a complete `turn` row set and its
/// `session_coverage` record together: [`antiburn_local::analysis::delete_turn_rows_except_fence`]
/// and [`antiburn_local::analysis::delete_turn_rows_for_fence`] delete from
/// both tables under the same fence, so the two are never on disk one
/// without the other.
const V21: &str = r#"
ALTER TABLE session_evidence ADD COLUMN published_fence INTEGER;
UPDATE session_evidence SET published_fence = claim_fence WHERE status IN ('ready','unsupported');
"#;

/// v22 — drop `session_evidence.diagnostics_json`.
///
/// [`V11`] added this column to hold a serialized diagnostics snapshot from
/// each analyzed pass. Every pass wrote it and [`super::model::EvidenceRow`]
/// read it back, but no caller ever used the value: only round-trip test
/// assertions did. Nothing reads it, so this drops it.
const V22: &str = r#"
ALTER TABLE session_evidence DROP COLUMN diagnostics_json;
"#;

/// v23 — an expression index for [`super::Store::recent_sessions`].
///
/// That query wraps the indexed column in `COALESCE(updated_at_epoch, 0)`,
/// in the `WHERE` clause and the `ORDER BY`, so it cannot use [`V1`]'s plain
/// `session_recency` index — SQLite only matches an index to an expression
/// it names verbatim. This index names the same `COALESCE` expression, plus
/// `session_id DESC` as a second key, matching `recent_sessions`'s full
/// `ORDER BY` exactly.
///
/// No other query in this crate orders or filters on a bare
/// `updated_at_epoch`, so `session_recency` now serves no query and this
/// migration drops it too.
const V23: &str = r#"
DROP INDEX session_recency;
CREATE INDEX session_recency_coalesced
    ON session (COALESCE(updated_at_epoch, 0) DESC, session_id DESC);
"#;

/// v24 adds the serialized provider hints from the bounded parent
/// `SessionSummary`. Existing rows stay `NULL` because absence means the
/// provider observation is unknown, not an observed empty list.
const V24: &str = r#"
ALTER TABLE session_analysis ADD COLUMN provider_hints_json TEXT;
"#;

/// v25 removes cached account identifiers created before install-scoped keys.
const V25: &str = r#"
DELETE FROM setting
 WHERE key IN ('internal:liveUsageHistory', 'internal:liveUsageSnapshot');
"#;

/// v26 stores only opaque, append-only account observations for a session.
const V26: &str = r#"
CREATE TABLE session_provider_account (
    environment_key TEXT NOT NULL,
    agent           TEXT NOT NULL,
    session_id      TEXT NOT NULL,
    provider        TEXT NOT NULL,
    account_key     TEXT NOT NULL,
    provenance      TEXT NOT NULL CHECK(provenance IN ('provider_live', 'tool_oauth')),
    confidence      TEXT NOT NULL CHECK(confidence = 'direct'),
    first_seen_at   TEXT NOT NULL,
    PRIMARY KEY (environment_key, agent, session_id, provider, account_key),
    FOREIGN KEY (environment_key, agent, session_id)
      REFERENCES session(environment_key, agent, session_id) ON DELETE CASCADE
) STRICT;
CREATE INDEX session_provider_account_lookup
    ON session_provider_account(environment_key, agent, session_id, provider);
CREATE TABLE provider_account_seen (
    agent            TEXT NOT NULL,
    provider         TEXT NOT NULL,
    account_key      TEXT NOT NULL,
    first_seen_epoch INTEGER NOT NULL,
    PRIMARY KEY (agent, provider, account_key)
) STRICT;
INSERT OR IGNORE INTO setting (key, value)
VALUES ('internal:providerAccountRolloutV1', CAST(strftime('%s', 'now') AS TEXT));
"#;

/// v27 records the latest observation so account switches have a time boundary.
const V27: &str = r#"
ALTER TABLE provider_account_seen
ADD COLUMN last_seen_epoch INTEGER NOT NULL DEFAULT 0;
UPDATE provider_account_seen
   SET last_seen_epoch = first_seen_epoch;
"#;

/// v28 adds the `session_coverage` table. Each row holds one serialized
/// `SessionCoverageRecord`, keyed by `(environment_key, agent, session_id,
/// claim_fence)`. A pass writes its record under this key alongside its
/// `turn` rows, under the same fence. The record carries the accumulator
/// fields `evidence_from_facts` needs that never become a `TurnRow`: the
/// tools catalog, skills and MCP sources, subagent spawn observations,
/// diagnostics counters, and coverage and loss reasons.
///
/// `antiburn_local::analysis::SESSION_COVERAGE_SCHEMA_SQL` owns the column
/// list, for the same reason [`V15`] re-exports `TURN_SCHEMA_SQL` instead of
/// stating its own DDL.
const V28: &str = antiburn_local::analysis::SESSION_COVERAGE_SCHEMA_SQL;

/// v29 indexes the timestamp range used by session limit allocations.
const V29: &str = antiburn_local::analysis::TURN_SCHEMA_V4_SQL;

/// v30 adds the `source_resume` table (continuous ingest, phase 3b). Each
/// row holds one source's persisted resume snapshot, keyed by
/// `(environment_key, agent, session_id, source_key)` — the parent
/// transcript's own session id, or a discovered child's own id.
///
/// Unlike `turn` and `session_coverage`, this table carries no
/// `claim_fence`: it names one current snapshot per source, not a row set
/// per pass. [`super::Store::publish_projections`] writes and replaces a
/// row only inside a winning publish transaction, so a losing or
/// in-flight pass never clobbers the snapshot a prior winning pass left
/// behind.
///
/// `published_fence` (`v21`) itself changes meaning as of this migration:
/// a winning publish no longer always stamps it to the pass's own claim
/// fence. When a source resumes, its newly claimed rows are re-stamped
/// onto the session's *existing* `published_fence` instead — the append
/// joins the row set already there — so `published_fence` stays put across
/// a resumed pass and only becomes the claim fence the first time a
/// session ever publishes. A source that reads fully still has its old
/// published rows deleted and its new rows stamped onto that same
/// `published_fence`. Either way, every source's rows for a session share
/// one fence once a pass publishes: [`antiburn_local::analysis::delete_turn_rows_except_fence`]
/// keeps this true, and finds nothing left to delete on a resumed pass.
///
/// During the pass itself, before that publish transaction runs, a
/// resumed source's rows are genuinely split: its new rows sit under
/// `claim_fence` and its old rows stay under `published_fence` until
/// re-stamped. A mid-pass fact read must still see both, so it uses
/// [`antiburn_local::analysis::FenceScope`] to union the claim fence with
/// the published fence, restricted to the sources that resumed this
/// pass — never by re-stamping early, which would break claim-race
/// safety.
///
/// `antiburn_local::analysis::SOURCE_RESUME_SCHEMA_SQL` owns the column
/// list, for the same reason [`V15`] re-exports `TURN_SCHEMA_SQL` instead of
/// stating its own DDL.
const V30: &str = antiburn_local::analysis::SOURCE_RESUME_SCHEMA_SQL;

/// v31 supports the bounded Insights cohort range and recency order.
const V31: &str = r#"
CREATE INDEX session_insights_window
    ON session (environment_key, started_at_epoch DESC, session_id DESC);
"#;
