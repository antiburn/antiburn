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
