# Changelog — antiburn-local

Changes to the local engine crate, released under `antiburn-local-v*` tags. The
desktop application has its own changelog at the repository root.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and
versions follow [semantic versioning](https://semver.org/spec/v2.0.0.html).

The audience here is different from the application's: this file is read by
somebody who depends on the crate from their own code, so it states API and
behaviour changes — including anything that moves the local boundary, the
discovery roots, or the persistence and export contracts, each of which is
a compatibility fact rather than a feature note.

`.github/workflows/release-engine.yml` reads the section matching the tagged
version and refuses the release if there is none.

## [Unreleased]

## [0.3.0] - 2026-09-01

### Added

- The row pipeline exposes `TurnRow`, `TurnRowSink`, `TurnRowStore`, turn and
  content schema migrations, bounded batch writes, deletion helpers, and
  row-derived metrics and evidence queries.
- `TurnContent`, `ContentPart`, and `ContentKind` carry bounded message text,
  thinking, tool inputs, and tool results to a separate content table.
- `TurnFacts`, `metrics_from_rows`, and `metrics_by_source` rebuild projections
  from a fenced persisted snapshot.
- `ModelRegistry` and its policy contracts expose model-family, replacement,
  effort, and speed rules used by Insights.

### Changed

- **Breaking:** `NormalizedRecord` gains `TurnContent`; `NormalizedEvent` gains
  logical parent and thread identity; and the report requirement contract now
  uses `Fact` and `FactState` instead of capability and evidence groups.
- Vendor adapters now derive stable thread relationships for Claude, Codex,
  OpenCode, and Pi, including delegated and sidechain turns. Codex also reads
  service tiers and cache-write tokens.
- Metrics and evidence reducers retain bounded derived state, account for
  repeated context per thread, and expose row-backed chart and drilldown data.
- Insights evaluates explicit finding and clean fact sets, preserves the last
  published verdict during recomputation, and declines clean results when a
  required fact is incomplete.
- macOS repository discovery treats `~/Developer` as a common unprotected code
  directory.

## [0.3.0-rc.1] - 2026-09-01

### Added

- The row pipeline exposes `TurnRow`, `TurnRowSink`, `TurnRowStore`, turn and
  content schema migrations, bounded batch writes, deletion helpers, and
  row-derived metrics and evidence queries.
- `TurnContent`, `ContentPart`, and `ContentKind` carry bounded message text,
  thinking, tool inputs, and tool results to a separate content table.
- `TurnFacts`, `metrics_from_rows`, and `metrics_by_source` rebuild projections
  from a fenced persisted snapshot.
- `ModelRegistry` and its policy contracts expose model-family, replacement,
  effort, and speed rules used by Insights.

### Changed

- **Breaking:** `NormalizedRecord` gains `TurnContent`; `NormalizedEvent` gains
  logical parent and thread identity; and the report requirement contract now
  uses `Fact` and `FactState` instead of capability and evidence groups.
- Vendor adapters now derive stable thread relationships for Claude, Codex,
  OpenCode, and Pi, including delegated and sidechain turns. Codex also reads
  service tiers and cache-write tokens.
- Metrics and evidence reducers retain bounded derived state, account for
  repeated context per thread, and expose row-backed chart and drilldown data.
- Insights evaluates explicit finding and clean fact sets, preserves the last
  published verdict during recomputation, and declines clean results when a
  required fact is incomplete.
- macOS repository discovery now includes `~/Developer`.

## [0.2.0] - 2026-08-28

### Added

- `analysis::OpenCodeAdapter` and `SourceCapabilities::opencode()` provide
  bounded metrics and evidence from OpenCode JSONL exports and SQLite sessions.
  Claimed SQLite reads validate the session fingerprint inside one read snapshot.
- `insights::UnrecognizedRecords` and `insights::MAX_REPORT_UNRECOGNIZED_TYPES` expose a bounded discriminator set and non-exclusive cohort counts for inert, evidence-bearing, set-capped, and string-truncated unknown records.
- `analysis::PiAdapter` and `SourceCapabilities::pi()` provide bounded,
  source-validated metrics and evidence for Pi JSONL sessions.
- `NormalizedEvent::may_resolve_late_tool` identifies events whose tool call is
  available only in the final session summary. The hidden public field
  `NormalizedEvent::late_tool_candidate_is_builtin` identifies provisional
  built-in command candidates for bounded late-tool resolution.

### Changed

- `adapter_for("opencode")` now streams OpenCode SQLite sessions directly and
  no longer depends on a schema-agnostic SQLite fallback.
- Codex fork ownership lookahead keeps at most 256 records or 1 MiB. A later
  ownership marker reports partial attribution instead of retaining more rows.
- **Breaking:** `EvidenceObservation::UnrecognizedType` gains an `inert` field, `ParseDiagnostics` gains `records_unrecognized_inert`, and `EfficiencyReport` gains `unrecognized_records`. Its `UnrecognizedRecords` summary separates set-capped and string-truncated session counts. `PARSER_REVISION` is now 4 and `EVIDENCE_SCHEMA_REVISION` is now 3, so older stored evidence is stale and reprocessed lazily.
- Structurally inert unknown Claude records retain complete coverage and can produce report and badge results. Evidence-bearing unknowns still fail closed, including allowlisted eventless names that begin carrying shallow evidence. Known eventless records tolerate command echoes and unread scalar evidence-key names in nested configuration. Unknown discriminator truncation or collection overflow still produces `CapExceeded` and blocks clean results.
- `adapter_for("pi")` now selects the dedicated Pi adapter instead of the
  generic JSONL fallback.
- `SessionMetricsAccumulator` now retains bounded derived state instead of one
  entry per metric event. Large sessions merge facts on an active-position
  quantum, so continuous values can move between progress buckets. Additive
  totals remain exact outside documented collection caps.
- `merge_metrics` uses one shared active-time axis, adds efficiency per thread,
  honors parent source tags, and projects retained cache facts on the shared axis.
- `skill_uses` is capped at 256 entries, `tool_calls_by_name` at 256,
  `mcp_tool_calls` at 128, and model breakdowns and runs at 32. The export format
  remains version 2 because no field shape changed. Efficiency keeps 1,440
  ordered cost contributions. Beyond that cap, priced contributions can change
  floating-point accumulation order. The aggregate fallback fresh-token split
  remains per-turn exact. Efficiency also keeps 64 open
  fragmented messages and a 32-turn timestamp reorder window.
- `retained_turns()` is replaced by `observed_turns()`, and `retained_bytes()`
  reports reducer-owned derived state. `RETAINED_METRICS_BYTES_BOUND` publishes
  a 640 KiB derived-state contract. Exact caller-provided identity strings are
  additional.
- Summary models, skill descriptions, and initial-context details now use
  explicit bounds inside the metrics accumulator. Initial context keeps the 61
  largest named rows and up to three named source-total rows. Descriptions for
  invoked skills keep 300 characters and end with an ellipsis when shortened.
  Session identity strings remain exact.
- Tool, MCP, model, thinking-mode, speed, last-tool, and skill names use
  separate bounded stores. The limits are 256 tools, 128 MCP servers, 32
  normalized models, 32 bucket-display models, 64 thinking modes, 64 speeds,
  256 last-tool names, and 64 distinct skill names. Skill names keep 192 bytes;
  other names keep 64 bytes. Every shortened name uses a hash suffix.
- More than 1,024 active-time intervals merge a new interval with its nearer
  neighbour. This makes active duration and positions approximate inside the
  compacted span.
- `PARSER_REVISION` is 4 because unknown-record structural inertness changes
  parser behavior. `ANALYZER_REVISION` is 6, so cached analyses recompute once.

## [0.1.9] - 2026-08-27

This release starts the public engine release line under the MIT License. It has
the same API and behavior as `0.1.8`. Git consumers must update their pinned
commit SHA.

## [0.1.8] - 2026-08-27

### Added

- `analysis::SessionEvidence`, `SessionEvidenceAccumulator`, and
  `CompositeSink` collect bounded, versioned evidence about context depth,
  tools, loaded skills and MCP servers, models, delegation, cache behavior,
  compactions, source coverage, and parse diagnostics in the same streaming
  pass that produces session metrics.
- `insights::EfficiencyReportAccumulator` reduces ready session evidence into
  a bounded report with explicit cohort, coverage, capability-gap, and
  per-detector counts. The API includes the nine detector identifiers and the
  evidence requirements for each detector; it does not yet implement detector
  policy.
- `analysis::tool_catalog` resolves the built-in tools and definition-token
  costs for a recorded harness version and model. Initial-context rows now
  include built-in tools, use counts, deferred-tool state, and known skill
  origins.

### Changed

- **Breaking:** `InitialContextTokenSource` now represents `Skill`, `Mcp`, and
  `BuiltinTool`; it no longer exposes agent-instruction, system-instruction, or
  unattributed variants. `InitialContextBreakdown` no longer has
  `tracking_status` or `total_tokens`, and each `InitialContextSourceCount` now
  includes `use_count`, `origin`, and `deferred`.
- **Breaking:** `SessionMetrics` no longer exposes the categorical `tool_mix`
  totals. `NormalizedRecord` no longer carries `grep_count`, and
  `SessionSummary` no longer carries `grep_total`. Callers can use the new
  per-tool evidence and `tool_calls_by_name` data instead.
- Claude metrics and evidence now share one record-by-record pass. The engine
  records cache-routing misses, per-tool calls, MCP calls, model and effort
  changes, delegation, compaction boundaries, and bounded context evidence
  without rereading the transcript.

## [0.1.7] - 2026-08-25

### Changed

- **Breaking:** `VendorAdapter::visit` returns `VisitOutcome` rather than `()`.
  The default implementation returns `VisitOutcome::Unvalidated`, so an adapter
  that does not check source validity only needs its signature updated. Callers
  that discarded the unit result must now handle the outcome, because a
  successful return no longer means the records describe a single coherent
  source.
- **rusqlite moves back to the 0.32 line** from 0.40. `libsqlite3-sys` sets
  `links = "sqlite3"`, so a dependency graph may contain exactly one version of
  it — a constraint on resolution, not on the build, which therefore binds even
  when the conflicting dependency's features are off. An embedder that also uses
  SQLx 0.8 needs `libsqlite3-sys ^0.30.1`, which only rusqlite 0.31 and 0.32
  satisfy; against 0.40 such a graph simply fails to resolve, and the failure
  lands downstream rather than here. The engine used no API newer than 0.32, so
  the newer line bought nothing. `.github/dependabot.yml` now ignores `rusqlite`
  and `libsqlite3-sys` so an automated bump cannot silently reintroduce this.
- Per-agent discovery completion now logs at `debug` rather than `info`. It
  reported once per agent per scan, which is scan bookkeeping rather than
  something an embedder's default log level should carry.

### Added

- `analysis::EfficiencyTotals` and `analysis::thread_efficiency` split the cost
  of priced assistant turns into new work, cached carry, and rewritten input.
  The calculation merges records for one message, orders turns by timestamp,
  and reports unpriced turns separately. Callers calculate each parent or
  sub-agent event stream on its own, then combine totals with
  `EfficiencyTotals::add`.
- `analysis::source_validity` decides whether a transcript that was read still
  describes the source it claimed to: `SourceClaim`, `PinnedSource`,
  `PinnedOpen`, `PinnedReader`, `AppendOnlyGuarantee`, and
  `append_only_guarantee`. `PinnedSource::open` pins a claimed source,
  `recheck_prefix` and `recheck_full` re-verify it after reading, and each
  returns the specific way it diverged rather than a bare failure.
- `analysis::VisitOutcome` and `analysis::SourceChangedReason` report that
  verdict to a caller. `AcceptedFull`, `AcceptedPrefix { boundary }`, and
  `Unvalidated` distinguish a fully verified read from a verified prefix and
  from no check at all, so a partial result is usable instead of merely
  suspect. `SourceChangedReason` names the divergence — identity mismatch, a
  short file at open, a head-region mismatch, a short read, truncation after
  reading, or a fingerprint mismatch.
- `ClaudeAdapter` is exported, and `ClaudeAdapter::visit_claimed` streams a
  Claude transcript against a `SourceClaim`, validating the read rather than
  trusting it.
- `discovery::SourceStat::from_open_std_file` stats an already-open
  `std::fs::File`, which is what the pinned-read path holds.

### Added

- `analysis::framing` frames a JSONL transcript one record at a time.
  `BoundedJsonlReader` and `FramedRecord` hold each record under
  `MAX_RECORD_BYTES`, so a single oversized or malformed line cannot make a scan
  allocate without bound, and the caller can cancel between records.
- `analysis::interface` adds a streaming seam for transcript records:
  `RecordSink`, `NormalizedRecord`, `RecordSkip`, `RecordCoverage`,
  `PartialReason`, `SessionSummary`, and `SessionCollector`. An adapter reports
  one record at a time and finishes with a `SessionSummary`. `SessionCollector`
  accumulates the same `NormalizedSession` the whole-document path produces and
  reports the coverage and the partial reasons for it.
- `discovery::source_version` gives a session source a storage-neutral identity
  and version: `SourceDescriptor`, `SourceVersion`, `SourceStat`,
  `FingerprintInputs`, `Streamability`, `head_hash_of`, and
  `FINGERPRINT_HEAD_BYTES`, with `SourceRead` in `discovery`.
  `Explorers::source_version` builds the value, and a scan keeps the
  fingerprint, so a caller can tell an unchanged source from a grown one without
  reading the transcript again.
- `analysis::merge::merge_subagent_events` folds a sub-agent transcript into its
  parent session, and `EventSource` records which transcript an event came from.
- `SessionMetrics` carries `model_runs: Vec<ModelRun>`, `compaction_count`, and
  `cache_rehydration_count`. A `Bucket` carries `cache_read_tokens`,
  `cache_write_tokens`, `is_cache_rehydration`, `subagent_tokens`,
  `secs_since_prior_turn`, `subagent_launches`, `user_prompts`, `last_tool`,
  `model`, `thinking_mode`, `speed`, `has_thinking`, `compaction_trigger`
  (`CompactionTrigger`), `compaction_pre_tokens`, and `compaction_post_tokens`.

### Changed

- Analysis and discovery now emit structured local diagnostic events at silent
  recovery seams. This change does not alter analysis results or public APIs.
- The bundled TOML integration now uses `toml` 1.1.4.

### Removed

- The session pattern analytics surface: `Phase`, `PhaseSegment`,
  `PhaseDistribution`, `MIN_PHASE_WEIGHT`, and `active_time_fraction`. What they
  reported did not describe the sessions they claimed to describe.
- The local skill detail surface: `SkillDetail`, `LocalSkillDetails`, and
  `SkillScope`.

### Fixed

- Codex title discovery now distinguishes user-set names and generated titles
  from raw first-message fallbacks in the current state database. Generated
  session-index names can replace raw prompts, while legacy title-only state
  databases keep their existing rename behavior.
- Codex cache rehydration is now inferred when the cached prefix stays cached,
  and the `token_count` row Codex repeats on resume no longer counts twice.
- A Codex compaction is now detected from a top-level compacted record.
- A session keeps its own context window in its summary rather than the
  reference window.
- A multi-model session now reports stable cost totals across repeated analysis.
- Codex task titles are restored.
- The spawn-edges sidecar is flushed before its rename.

## [0.1.4] - 2026-08-21

### Changed

- The bundled SQLite integration now uses `rusqlite` 0.40.2.

## [0.1.3] - 2026-08-20

### Added

- `AgentExplorer::indexed_session_titles` and
  `Explorers::indexed_session_titles_for` batch title lookups from durable
  vendor indexes without falling through to transcript content. Shared indexes
  are opened once per batch, so background discovery can reuse its bounded
  transcript metadata on misses.

### Fixed

- Claude and Codex discovery now derives session recency from meaningful
  transcript events rather than filesystem modification times. Agent
  housekeeping such as title, mode, permission, and token-count updates no
  longer makes an idle session look active, while subagent transcript activity
  still advances its parent session. Incremental aggregate cursors keep this
  provider-aware scan bounded across unchanged parent and child transcripts.

## [0.1.2] - 2026-08-17

### Added

- `repositories::partition_cwds_by_grants` lets an embedding application split
  working directories into immediately safe and consent-deferred paths without
  touching the filesystem. `repositories::verify_dir_access` exposes the same
  directory-read check discovery uses, including stale-grant revocation and
  probe diagnostics.

### Fixed

- Repository resolution no longer starts `git` inside an ungranted macOS
  protected directory. Working directories under Documents, Desktop, and
  Downloads are deferred before any filesystem or child-process access, so a
  background scan cannot raise the operating system's consent dialog.
- A grant revoked outside the application is removed when a directory read is
  denied, and its diagnostic probe now uses the same `denied` outcome vocabulary
  as an application-requested consent check.

## [0.1.1] - 2026-08-14

### Fixed

- `pricing::normalize_model_key` no longer panics on model IDs containing
  multi-byte UTF-8 characters. Model IDs come from external transcript files;
  the date-suffix check now runs on bytes and only slices at a confirmed ASCII
  hyphen boundary.
- The scan-down arm of `repositories::resolve_granted_repos` reports the
  canonical repository path in `repo_root` and `suspected_path` instead of the
  folded identity key (which lowercases and slash-normalizes on Windows). The
  key now serves only deduplication, matching the session-resolved arm and the
  documented field contract.

## [0.1.0] - 2026-08-13

### Added

- Initial public surface of the local engine, extracted as a self-contained
  crate:
  - `discovery` — local discovery of AI coding-agent sessions from documented
    files, read-only databases, and bounded WSL paths.
  - `analysis` — transcript and session analysis.
  - `repositories` — repository identity.
  - `pricing` — API-equivalent pricing.
  - `model`, `paths`, `platform` — shared local data model, filesystem roots,
    and platform handling.
  - Versioned local persistence and export contracts.
- The crate's local boundary as a compatibility contract: no dependency on any
  service of ours, no private dependencies, and a public API that carries no
  authentication, organization, remote-sharing, enrichment, or telemetry
  concepts. Enforced mechanically by the crate's boundary test suite.
