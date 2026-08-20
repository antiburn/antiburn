---
artifact: master_plan
issue: GH-70
title: "Local Insights first-provider implementation (Phases 0-11)"
created_by: master_planner
created_at: "2026-08-20"
---

# Local Insights: architecture and master plan

- **Issue:** [#70](https://github.com/antiburn/antiburn/issues/70) — *Implement Local Insights from the reviewed architecture plan*
- **Date:** 2026-08-20
- **Verified against:** `feat/gh70` @ `c03ad1e` (merge base of PR #54, PR #60, PR #74) — line numbers may drift; **prefer symbol/function names** over line numbers.
- **Immutable source revision:** this file replaces `docs/plans/local-insights-architecture.md` at commit `23979b103624cd8e6dafe9d148e652a64d8053d0` (blob `27ae19b3c389c7d8dbee5c09c59df12d437c449a`), merged in PR #54. That revision is the last one that touched the file before this rewrite, and it stays recoverable with `git show 23979b1:docs/plans/local-insights-architecture.md`. Its researched architecture is preserved below under **Architecture reference**; its sequential Phase 0-13 checklist is replaced by **Scope Areas**.

## Overview / Problem

antiburn discovers local coding-agent sessions and computes per-session metrics, but it cannot answer cross-session questions: which sessions ran too deep, which MCP servers and skills load into context and are never used, which subagents inherit a premium model, where cache tokens are resent, and where a provider quota blocked work. The private reference implementation answers those questions server-side. antiburn must answer them on the device, from the reader's own transcripts, without sending anything anywhere.

Two obstacles block that today. First, analysis reads each transcript whole (`crates/antiburn-local/src/analysis/vendors/mod.rs:read_source` calls `std::fs::read_to_string`) and materializes a complete `Vec<NormalizedEvent>`, so memory grows with transcript size — unacceptable for an always-running background utility. Second, nothing durable exists between the transcript and a report: `session_analysis` stores display metrics, not the rule-neutral facts a detector needs, and it carries no source generation or parser revision, so no consumer can tell whether a cached row is current.

The reviewed architecture in PR #54 solves both with two streaming pipelines separated by compact, versioned per-session evidence in antiburn's SQLite database. This issue executes that architecture for the first provider (Claude Code) through the source plan's Phase 11.

## Goals

- Process Claude JSONL one record at a time, with bounded retained record memory, and remove the whole-file `String` and the complete canonical-event vector from the shipped Claude metrics path.
- Keep every currently displayed `SessionMetrics` value equivalent through that conversion.
- Produce truthful, compact, rule-neutral `SessionEvidence` from the same source pass and persist it atomically with `session_analysis`.
- Version every transcript-derived projection (source generation, parser, analyzer, metrics, evidence) so staleness is decidable without reparsing.
- Move transcript processing out of the scan pass into a restart-safe durable worker that never holds the application database lock across I/O.
- Compute a bounded thirty-day Hygiene and Efficiency report over nine categories plus a separate quota-pressure section, with one truthful status per category.
- Expose the report, its coverage, and its processing backlog through desktop IPC and a Settings → Insights pane, with no transcript content in any payload.

## Current State (evidence)

Verified against `c03ad1e`. `crates/antiburn-local` is the only crate under `crates/`.

**Discovery.** `crates/antiburn-local/src/discovery/mod.rs` provides `Explorers::discover_recent_sessions`, `Explorers::discover_recent_sessions_with_progress`, `Explorers::provider_db_fingerprint` (delegating to `AgentExplorer::provider_db_fingerprint`), `SessionSource::{File, Inline, ProviderDb}`, and `SessionLog`. `SessionLog` carries `agent_type`, `source`, `updated_at`, and `environment` — it has **no** `session_id` field. `SessionLog::dedupe_key`, `SessionLog::cursor_key`, and `SessionLog::source_label` identify the *discovered source* for dedupe and incremental cursors. They are not the persisted session identity, which comes from parsed metadata, `scan.rs:recovered_id`, and `SessionKey` (see correction 7). `ACTIVE_SESSION_WINDOW_SECS` is `180` and lives in the same module. **It is a UI liveness predicate today, not a processing gate, and this plan does not make it one** (see the scan paragraph below). `SOURCE_PREVIEW_BYTES` caps bounded metadata previews at 16 MiB. `discovery/mod.rs:session_source_preview` already reads the head of every file source: `file.take(SOURCE_PREVIEW_BYTES).read_to_end(...)`.

**Analysis.** `crates/antiburn-local/src/analysis/interface.rs` defines `RawSource` as an **enum** (`Jsonl(String) | File(PathBuf) | Sqlite(PathBuf)`), `SessionInput`, and `VendorAdapter`. `VendorAdapter` has exactly two methods today: `agent()` and `normalize()`. `crates/antiburn-local/src/analysis/vendors/mod.rs:adapter_for` dispatches to six adapter statics: `claude`, `codex`, `cursor`, `opencode`, `antigravity`, and the `generic_jsonl` fallback (`jsonl.rs` and `sqlite.rs` are shared helpers, not adapters). `crates/antiburn-local/src/analysis/mod.rs` exposes `normalize_source` and `analyze_sources_with`; `crates/antiburn-local/src/analysis/engine.rs` defines `SessionMetrics` (including `first_ts_ms`) and `analyze_session`. `crates/antiburn-local/src/analysis/initial_context.rs` is a second independent pass over the same payload (`parse_initial_context`, `parse_skill_descriptions`), called from `analyze_sources_with`. `crates/antiburn-local/src/analysis/vendors/claude.rs` is the Claude adapter.

**Whole-file reads.** `vendors/mod.rs:read_source` returns `Cow::Owned(std::fs::read_to_string(path)?)` for `RawSource::File`. `vendors/sqlite.rs` also builds a whole `String` before `parse_jsonl`. `vendors/jsonl.rs:parse_jsonl(&str) -> Vec<NormalizedEvent>` materializes every event.

**No general size cap.** There is no 512 MiB transcript cap and no general analysis cap. Only provider-specific limits exist: `discovery/agents/opencode.rs::{CLI_METADATA_MAX_BYTES, CLI_EXPORT_MAX_BYTES, CLI_EXPORT_CACHE_MAX_BYTES}` (8/32/64 MiB), `discovery/agents/antigravity.rs::HISTORY_MAX_BYTES` (16 MiB), and `discovery/mod.rs::SOURCE_PREVIEW_BYTES` (16 MiB). None of these bound transcript analysis.

**Desktop scan and analysis.** `apps/desktop/src-tauri/src/scan.rs:pass` discovers sessions, calls `Store::upsert_sessions`, records scan state, and then calls `top_up_analysis` in the same pass. `scan.rs:spawn_scheduler` decides when a pass runs, and it is **not** an unconditional timer. A pass runs at launch when onboarding is finished, on the `scan.rs:TICK` of `Duration::from_secs(60)` **only while the popover is visible** — the tick branch calls `continue` when `ScanController::popover_visible()` is false — when the popover opens, and on demand from the rescan control or a source-selection change. The module contract states the same thing: scanning is "paused entirely while the popover is hidden". `AppSettings::discovery_paused` additionally stops every scheduled pass. Each pass caps analysis at `MAX_ANALYSES_PER_PASS = 60` candidates, and `top_up_analysis` re-reads the whole transcript of every candidate whose fingerprint moved; its own comment states the cost: "Analysis is the long tail of a pass — one whole transcript read per session." **So the whole-transcript re-read is already paid by shipped code whenever a pass runs, and background work is already bounded by the visibility gate.** There is no unconditional once-per-minute re-read.

`apps/desktop/src-tauri/src/analytics.rs` provides `fingerprint_of` (a second-resolution `mtime:size` string), `MISSING_FINGERPRINT = "-"`, `cache_is_fresh`, `analyze`, `analyze_subagent`, `analytics_supported`, and `is_active`. **`is_active` gates nothing.** Its only consumers are `commands.rs:398` and `commands.rs:632`, and both only fill a DTO field for the UI.

**No post-read recheck exists today.** `analytics.rs` computes `fingerprint_of` and then calls `read_to_string`, and nothing rechecks the source after the read. A transcript that grows during that read can therefore be cached under a fingerprint that no longer describes what was parsed. The inconsistency is real, though benign at today's stakes. This work fixes it rather than preserving it. `SourceChanged` appears in **zero** `.rs` files in the tree: it is a new result variant this plan introduces (CH-004), not an existing one.

**Store.** `apps/desktop/src-tauri/src/store/mod.rs` holds one `Mutex<Connection>`, sets `journal_mode = WAL` in `from_connection`, and serializes every access through `Store::lock`. Relevant methods: `upsert_sessions`, `session`, `recent_sessions`, `save_analysis`, `analysis`, `usage_evidence`, `delete_session`, `clear_local_session_data`.

**Schema.** `apps/desktop/src-tauri/src/store/schema.rs` declares `MIGRATIONS: &[&str] = &[V1, V2, V3, V4, V5]` and states the append-only rule in its module comment. Tables: `setting`, `scan_root`, `session`, `session_analysis`, `session_relation`, `scan_state`, `repository`, `consent_grant`, `usage_analytics_event`, `usage_analytics_identity`. `session` has no `source_fingerprint`, no `source_generation`, and no `started_at_epoch`; V4 added `activity_source` and `activity_cursor`. `session_analysis` already has `source_fingerprint TEXT NOT NULL` (the `mtime:size` string) and `pricing_generation`, but no analyzed generation and no parser/analyzer/metrics revisions.

**Desktop IPC and UI.** `apps/desktop/src-tauri/src/commands.rs` holds the `#[tauri::command]` surface, registered in `apps/desktop/src-tauri/src/lib.rs:invoke_handler` (for example `get_session_analytics`, `list_recent_sessions`, `clear_local_index`, `delete_session_data`). The frontend calls them through `apps/desktop/src/lib/ipc.ts`. `apps/desktop/src/lib/settingsPanes.ts:SETTINGS_PANE_IDS` is `general, appearance, sources, privacy, notifications, usage, about` — there is **no** `insights` pane. Panes live in `apps/desktop/src/views/settings/*Pane.tsx` under `apps/desktop/src/views/SettingsView.tsx`. Existing session UI is `apps/desktop/src/views/popover/SessionPane.tsx` and `apps/desktop/src/components/session/SessionAnalyticsPresentation.tsx`.

**Provider usage — two existing contracts, neither one an evidence source here.** `apps/desktop/src-tauri/src/provider_usage/mod.rs` (`summarize`, `window_bounds`, `lookback_start`, `spend_between`) derives *spend* from `Store::usage_evidence(since_epoch)`; its module comment states that an allowance, a percentage, a remaining balance, and a reset time are not things a transcript states. Account limits live on the separate cached `dto::LiveUsageSummary` held by `apps/desktop/src-tauri/src/usage_alerts.rs:LiveUsage`, read through `LiveUsage::snapshot()`. Session evidence in this issue comes from session transcripts only, so neither contract feeds it (see **Out of Scope**).

**Fixtures and tests.** The only committed transcript fixtures are `crates/antiburn-local/tests/fixtures/initial_context/{claude_realistic.jsonl, codex_realistic.jsonl, cursor_unsupported.jsonl}`. Parser and metric tests are inline in `crates/antiburn-local/src/analysis/tests.rs` and `crates/antiburn-local/src/analysis/vendors/jsonl.rs`. `crates/antiburn-local/tests/boundary.rs` mechanically enforces the engine's source boundary — no exfiltration endpoints or telemetry SDKs, no proprietary provenance — against the manifests in `docs/oss/`.

### Corrections to the pinned revision's current-code claims

The source plan was written against an earlier tree. These claims are now wrong or incomplete; the plan's **decisions** are unaffected.

1. **`RawSource` is an enum**, not a struct, and it carries no source-version concept. The plan listed it as a reusable type without that shape.
2. **`VendorAdapter` has only `agent()` and `normalize()`.** Adding `visit_source` needs either a default method or an update to all six adapter statics in `vendors/mod.rs:adapter_for`.
3. **The migration head is V5, not the count the plan assumed.** PR #60 added V5 (`usage_analytics_event`, `usage_analytics_identity`). The source-generation migration is therefore V6 and `session_evidence` is V7 or later.
4. **`session` gained `activity_source` and `activity_cursor` in V4.** Every `SessionRecord` mapping change must carry them.
5. **Provider-DB and inline sources do not merely lack an "equivalent" fingerprint — they map to `MISSING_FINGERPRINT` (`"-"`), and `analytics.rs:cache_is_fresh` rejects that value outright.** Their analysis cache can therefore never be fresh today.
6. **Two usage contracts already exist.** `Store::usage_evidence` with `provider_usage::{summarize, window_bounds, spend_between}` is transcript-derived local *spend*. The cached `dto::LiveUsageSummary`, read through `usage_alerts::LiveUsage::snapshot()`, is account-level limit state. Neither is an evidence source for this issue. This is a statement of fact and carries no obligation on any seam.
7. **`SessionLog` has no `session_id`, and the persisted session id is not derived from the dedupe/source-label scheme.** `SessionLog::{dedupe_key, cursor_key, source_label}` identify a *discovered source* for dedupe and cursor purposes. The canonical persisted id comes from `apps/desktop/src-tauri/src/scan.rs:describe_one_with_activity`, which takes `SessionMetadata::session_id` from the parsed bounded metadata, falls back to `scan.rs:recovered_id` (provider path recovery via `Explorers::recover_session_id_from_path`, then the file stem, then `SessionSource::ProviderDb::session_id`, then the `SessionSource::Inline` label), rejects an empty id, and builds the `SessionKey`. A `SourceDescriptor` must preserve that exact resolution order, not invent a second identity scheme.
8. **`crates/antiburn-local/tests/boundary.rs` enforces exfiltration and provenance boundaries, not a Tauri dependency ban.** The storage-neutrality rule for `antiburn-local` is a design boundary held by review and by the crate's dependency list, not by that test.

Claims that were re-verified and hold: the whole-file `read_to_string`, the complete `Vec<NormalizedEvent>`, the second initial-context pass, second-resolution `mtime:size` file fingerprints, the absence of generation/revision columns on `session_analysis`, `top_up_analysis` running inside the scan pass, the single mutex-guarded store connection, and the absence of any 512 MiB analysis cap.

## Desired End State

For Claude Code sessions on this machine:

- Discovery records a provider-aware `source_fingerprint` under the one fingerprint contract of Locked Decision 16, and a monotonic `source_generation` on `session`, plus a nullable `started_at_epoch`.
- A durable worker claims one pending generation at a time, streams the transcript record by record with bounded retained memory, and updates a metrics accumulator and an evidence accumulator from the same pass.
- Completion writes `session.started_at_epoch`, `session_analysis` (with analyzed generation and parser/analyzer/metrics revisions), and `session_evidence` (status, generation, revisions, versioned `evidence_json`) in one short transaction, guarded by the claimed generation and the claim fence, or writes nothing.
- A read-only connection on a pinned snapshot streams the trailing thirty-day assessed cohort into a bounded report accumulator. The accumulator produces nine category statuses plus a separate quota-pressure section. The report also carries the coverage denominator defined by FR-12. The denominator contains the assessed cohort. Every non-ready or non-current denominator row stays visibly outside the assessed cohort.
- The quota-pressure section reports transcript-attributable quota incidents only: limit kind, hit count, affected sessions and models, and observed times. It is not assessed when the transcript carries no quota evidence. There is one condition, not a matrix.
- Settings → Insights renders the report, its coverage, and the evidence backlog. No transcript content leaves the engine and no Insights path calls a provider.
- No `Unimplemented` evidence placeholder exists in any build, every cardinality-bearing evidence field has an enforced cap, and the Claude capability and coverage matrix is published at `crates/antiburn-local/tests/fixtures/claude_characterization/README.md`.

Non-Claude providers keep their current behavior throughout.

## Locked Decisions

Rationale lives here once; scope areas reference these by number.

1. **First provider is Claude Code JSONL.** It has a dedicated adapter (`vendors/claude.rs`) and the widest fixture surface, so it proves the JSONL streaming path before any provider-database row streaming.
2. **Evidence is rule-neutral fact, persisted as a versioned JSON payload in a new `session_evidence` table.** Thresholds, pricing, and catalogs then change without reparsing transcripts, and the payload matches the existing `metrics_json` convention rather than adding a column per map-valued fact.
3. **Evidence values are three-state — `Complete` / `Partial` / `Unsupported` — with a temporary debug-only `Unimplemented` variant.** Absence must never be inferred from incomplete coverage, and the debug-only variant makes an unfinished field a release-build failure.
4. **SQLite is the durable queue; an in-process signal only wakes the worker.** A restart then cannot lose pending work, and no second store is needed.
5. **Completion is guarded by both the claimed source generation and a `claim_fence` token.** Generation alone cannot fence two workers after a lease expires, because both hold the same generation.
6. **The report cohort is sessions whose trustworthy `started_at_epoch` falls in the trailing thirty days.** A start-time cohort keeps each selected session's whole history inside the window; `first_seen_at` is discovery time and is never a start fallback.
7. **The report reads through a dedicated read-only connection on a pinned read transaction inside `spawn_blocking`.** The store's single `Mutex<Connection>` would otherwise serialize a full report scan against the user's own writes.
8. **A changed source is reprocessed from the beginning; append-tail resume checkpoints are out of scope. A still-growing source may additionally be read as a pinned prefix, but a prefix may publish only where the provider carries an evidence-backed append-only guarantee.** Full reprocess stays the rule. A prefix read is stamped with its boundary and superseded by the next full pass, so no resume state persists across generations. Where the guarantee is absent, no prefix publishes and the source takes the full-reprocess path with its `SourceChanged` behavior unchanged. **Architecture reference → Source-validity outcomes** is the one home of those rules. Correctness first — byte-offset resume checkpointing belongs to the source plan's Phase 13 and needs measurement to justify.
9. **There is no total transcript size cap; retention is bounded per record instead.** Streaming removes the memory reason for a file cap, and no such general cap exists today.
10. **New schema arrives as appended migrations after V5 (generations and revisions first, `session_evidence` second); every shipped migration constant stays immutable.** This is the stated rule in `store/schema.rs`.
11. **`antiburn-local` stays storage-neutral: no Tauri dependency and no knowledge of the application schema.** Durable state, claiming, and IPC belong to the desktop application, per the ownership boundary below.
12. **Every other provider stays on the existing `analyze_sources_with` path for the whole of this issue.** Provider-by-provider rollout avoids an endlessly retrying pending backlog for uncharacterized providers.
13. **Existing `SessionMetrics` output stays equivalent field by field; any intended difference is stated and approved before a golden result changes.** The conversion is a rewrite of how metrics are produced, not of what they say.
14. **Rich metrics state may still grow with metric-bearing events in this issue.** The bounded-memory guarantee covers raw framing, normalized-record lifetime, evidence, and report reduction; making exact metrics finalization bounded would change the metrics output contract and is deferred.
15. **Evidence rides the metrics pass. There is no separate evidence schedule and no quiet-period debounce.** One pass, one mechanism — CH-006's composite sink already does this. Whenever a pass runs at its existing shipped triggers — launch, a `TICK` while the popover is visible, popover open, on demand — `top_up_analysis` has already paid the whole-transcript read and the JSON parse for every candidate whose fingerprint moved, so attaching evidence accumulators costs only counters over records that were already decoded (FR-5). A debounce would save the durable row write but not that read, and it would stack a second scheduling policy on a scheduler that already bounds background work: scheduled passes do not run while the popover is hidden, and `AppSettings::discovery_paused` stops them outright. Consequence: nothing defers an active session, `ACTIVE_SESSION_WINDOW_SECS` stays the UI liveness predicate it is today, and the Claude metrics path keeps refreshing on exactly the triggers it uses now. **This plan claims no global minute schedule anywhere.**
16. **One fingerprint contract: stable file identity where available, byte size, high-resolution modification/change time, and a hash of a small fixed head region.** This decision is the single home of that contract; the Architecture reference's fingerprint policy and CH-002 reference it rather than restating it, and it supersedes the second-resolution `mtime:size` string `analytics.rs:fingerprint_of` computes today. The head hash costs a hash over bytes discovery already reads (`discovery/mod.rs:session_source_preview`), not a new read. **Detection envelope, stated exactly: the head hash detects changes within the hashed head region only.** It closes a same-size in-place rewrite *of that region*. It does **not** prove that any byte outside that region is unchanged, and it does not prove a prefix or a whole file is append-only. Hashing more would be the unbounded work this plan exists to avoid. Two constraints bind every seam: the hashed region is small and fixed and is explicitly **not** `SOURCE_PREVIEW_BYTES` (16 MiB); and the hash is fingerprint input only — never surfaced, logged, exported, or retained as evidence. Its size is set in CH-002.

## Invariants & Constraints

Every seam must respect these however it is sliced.

**Repository rules.**

- No React `useEffect` without explicit prior agreement from the dev (`AGENTS.md`, "React useEffect"). The Insights pane derives during render or acts in the event that caused the work.
- No Rust lint suppressions for dead or deprecated code without explicit prior agreement (`AGENTS.md`, "Rust dead and deprecated code"). Evidence scaffolding must be reachable from real code or tests, not silenced.
- All code comments in ASD-STE100 (`AGENTS.md`, "Comments"): active voice, present tense, short sentences, approved words, identifiers unchanged.
- Every commit carries a DCO sign-off — `git commit -s` (`AGENTS.md`, "Commits"). One missing sign-off fails the whole PR.
- Test fixtures must be synthetic: no real transcripts, usernames, home paths, repository names, or captured machine output; redaction is not sufficient (`CONTRIBUTING.md`, "Test fixtures must be synthetic").
- Performance and memory are product constraints for an always-running background utility: bound reads, allocations, concurrency, and retained data (`CONTRIBUTING.md`, "Performance and memory use are product constraints").
- A capability that could cost the reader something is named and bounded in the PR, and a decision of that shape is recorded in `docs/deviations.md` (`CONTRIBUTING.md`).
- Migrations are append-only for any constant that has reached an installed database. Pre-release the ladder may still be edited; the constraint binds at first ship (`apps/desktop/src-tauri/src/store/schema.rs`, module comment: "Never edit an entry that **has shipped**"). V6 and V7 in this plan are appended, not squashed into an existing entry.
- The engine's source boundary stays clean: no exfiltration endpoint, no telemetry SDK, no proprietary provenance (`crates/antiburn-local/tests/boundary.rs`, `docs/oss/`).

**Correctness and privacy boundaries** (issue GH-70, "Required boundaries"; expanded in **Architecture reference → Non-negotiable correctness semantics** and **→ Privacy and local-data policy**).

- Persist derived, rule-neutral facts only. Never persist raw transcripts, complete canonical sessions or events, or detector conclusions.
- Distinguish `Complete`, `Partial`, and `Unsupported`. Incomplete evidence must never produce a false clean result.
- Keep source/parser capabilities separate from per-session evidence coverage.
- Reject stale projections when a source changes during processing; leave the newer generation pending and preserve the last completed payload.
- Update `session_analysis` and `session_evidence` atomically for the claimed generation, or update neither.
- Keep transcript processing bounded, restart-safe, and outside long-held application-database locks. Never hold the store guard across `await`, `spawn_blocking`, transcript I/O, parsing, analysis, or detector work.
- Preserve existing session UI behavior until the Insights UI scope area (CH-012).
- Implement provider by provider. Do not add provider-specific logic to the report reducer where a normalized evidence fact can express the difference.
- No transcript content in logs, errors, Tauri events, or IPC DTOs. Never modify or delete a provider's source transcript.
- Include `session_evidence` in `Store::clear_local_session_data` and `Store::delete_session`, and in the schema data-policy comments and local-data documentation.

## Definition of Done (applies to every seam)

- The scope area's acceptance criteria are met for the slice.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` are clean (`CONTRIBUTING.md`, "Development"). Frontend slices additionally run `pnpm --filter @antiburn/desktop lint`, `type-check`, and `test`.
- `aislop ci --changes --base <seam base>` is clean. **The base is the parent seam's branch, not `main`**: these PRs stack, so gating against `main` makes each seam inherit its predecessors' findings. This gate depends on antiburn#89, which adopts aislop and must land before the first seam in this stack verifies; antiburn#90 ratchets thresholds separately and is not a dependency. `complexity/file-too-large` is enforced, so a new module must stay within the configured threshold.
- New non-trivial branches have at least one test. Untested defensive branches are untested behavior.
- Golden `SessionMetrics` comparisons still pass, or a stated and approved intentional difference is recorded (Locked Decision 13).
- No new transcript content reaches any persisted row, log, error, or DTO.
- Schema slices verify a fresh migration and a migration from every prior schema version.
- Every commit is signed off (`git commit -s`).

## Patterns & Utilities to Reuse

- Discovery fan-out, WSL/native dedupe, and bounded previews: `crates/antiburn-local/src/discovery/mod.rs` (`Explorers`, `SessionLog`, `SessionSource`, `ACTIVE_SESSION_WINDOW_SECS`, `SOURCE_PREVIEW_BYTES`).
- Provider-database fingerprints: `Explorers::provider_db_fingerprint` and `AgentExplorer::provider_db_fingerprint`.
- Vendor dispatch and adapter registry: `crates/antiburn-local/src/analysis/vendors/mod.rs` (`adapter_for`, `has_dedicated_adapter`).
- Token, model, and tool normalization plus metric semantics: `crates/antiburn-local/src/analysis/engine.rs`.
- Initial-context attribution semantics, including tracked/partial/unavailable states: `crates/antiburn-local/src/analysis/initial_context.rs`.
- Pricing: `crates/antiburn-local/src/pricing/` and `analysis/pricing.rs` — reused at report time, never baked into evidence.
- Cache-freshness and fingerprint conventions: `apps/desktop/src-tauri/src/analytics.rs` (`fingerprint_of`, `cache_is_fresh`, `MISSING_FINGERPRINT`).
- Migration and store conventions: `apps/desktop/src-tauri/src/store/schema.rs`, `store/model.rs`, `store/mod.rs` (`Store::lock`, `save_analysis`, `usage_evidence`).
- Settings pane registration and UI shape: `apps/desktop/src/lib/settingsPanes.ts`, `apps/desktop/src/views/SettingsView.tsx`, existing `*Pane.tsx` components.
- Imperative-boundary session classes read through `useSyncExternalStore`: `apps/desktop/src/views/settings/SourcesSession.ts` and `apps/desktop/src/views/settings/SettingsWindowSession.ts`. `SettingsWindowSession`'s doc comment states the principle and cites `OnboardingSession` as the precedent for the same shape in a different window. CH-012 follows it.

## Functional Requirements

- **FR-1:** Discovery must record a provider-aware source fingerprint and a monotonic `source_generation` per session; re-observing the same fingerprint must not increment the generation, and no complete transcript may be read to decide it.
- **FR-2:** JSONL framing must bound retained bytes per record, drain an oversized record through its newline, resume at the next record, and process a file larger than the retained-memory budget.
- **FR-3:** The shipped Claude metrics path must contain no whole-file `String` and no complete `Vec<NormalizedEvent>`.
- **FR-4:** Every currently displayed `SessionMetrics` value must stay equivalent after the streaming conversion (Locked Decision 13).
- **FR-5:** One source pass must feed both the metrics accumulator and the evidence accumulator; adding evidence must not add a second pass, and the source line and parsed value must be dropped before the next record is retained.
- **FR-6:** Every detector-critical evidence group must report `Complete`, `Partial`, or `Unsupported`, and a complete empty collection must be distinguishable from an unsupported one.
- **FR-7:** Completion must write `session.started_at_epoch`, `session_analysis`, and `session_evidence` in one short transaction, guarded by the claimed generation and claim fence; a failed guard must commit nothing.
- **FR-8:** Marking a newer generation pending must preserve the previously completed generation's evidence payload.
- **FR-9:** Transcript processing must run in a durable worker outside the scan pass, and a process restart must not lose pending work.
- **FR-10:** Evidence must be produced on the metrics pass, with no separate evidence schedule and no quiet-period gate on claiming (Locked Decision 15). A `SourceChanged` result must use bounded backoff through `next_attempt_at` rather than a tight retry loop, and one repeatedly changing source must not prevent stable sessions from progressing. An actively updated session's metrics must keep refreshing on exactly the shipped trigger set — launch, a `TICK` while the popover is visible, popover open, and on demand — and this work must add no trigger and remove none.
- **FR-11:** A projection-revision change (parser, analyzer, metrics, evidence) must requeue affected sessions lazily without incrementing `source_generation`; detector, remediation, pricing, and model-replacement catalog changes must not requeue anything.
- **FR-12:** The report must define two nested populations over one thirty-day window and the current machine/environment scope. **The coverage denominator is the report population; the assessed cohort is its ready-and-current subset — denominator ⊇ cohort.** This is the one home of that rule; every other passage references FR-12 rather than restating it. (a) The **coverage denominator**: every session in scope whose trustworthy `started_at_epoch` is in `[window_start, window_end)`, **plus** every session whose `started_at_epoch` is unknown and whose `updated_at_epoch` is in the same window — **regardless of evidence lifecycle or currentness**, so a pending, processing, failed, unsupported, or stale row with a trustworthy in-window start belongs to it. (b) The **assessed cohort**: the denominator's subset with ready, generation- and revision-current evidence. A session with an unknown start and no activity in the window is outside the report entirely and is not counted. Every denominator row outside the cohort must be counted and reported by its own reason, must never be dated by discovery time, and must never enter any detector's eligible or assessed denominator.
- **FR-13:** The report must read only compact database rows through a read-only connection on one pinned snapshot, must not read transcripts, and must not block the writer connection for the duration of the scan.
- **FR-14:** Each of the nine categories must report exactly one status — findings, clean, or not assessed with a structured reason — and incomplete coverage must never produce clean.
- **FR-15:** The quota-pressure section must stay separate from the nine categories and must be sourced only from transcript-attributable quota incidents (the `quota_incidents` evidence group). It must deduplicate incidents and must report limit kind, hit count, affected sessions and models, and observed times. It must be not assessed when the transcript carries no quota evidence — one condition, not a matrix. It must call no provider endpoint and must read no account-level limit state.
- **FR-16:** Desktop IPC must expose the report, its coverage counts, and the evidence backlog without any transcript content, and closing the pane or shutting down must cancel report work without corrupting durable evidence state.
- **FR-17:** No `Unimplemented` placeholder and no fake provider value may exist in a release build; a remaining use must fail release compilation.
- **FR-18:** `session_evidence` must be removed by session deletion and by clear-local-session-data, and must be described in the schema data-policy comments and local-data documentation.

## Architecture reference (preserved from the pinned revision)

The sections below are the researched architecture from `23979b1`, demoted one heading level and otherwise preserved. They remain the design reference every seam works against. Where a passage described current code, this plan's **Current State (evidence)** section supersedes it.

### Pipeline shape and delivery order

Local Insights should be built as two streaming pipelines separated by compact, versioned per-session evidence in Antiburn's SQLite database.

```text
Provider transcript processing

session discovery
  → source versioning and durable work state
  → bounded source-record stream
  → provider-specific normalization
  → multiple analysis accumulators
      ├── existing SessionMetrics
      └── new SessionEvidence
  → atomic session_analysis + session_evidence update

Cross-session report processing

read-only SQLite snapshot
  → stream session + ready evidence rows
  → composite report accumulators and detector checks
  → in-memory Insights/Hygiene report
  → desktop IPC and UI
```

The design intentionally does **not** persist raw transcripts or complete canonical sessions. A normalized record exists only long enough to update the relevant accumulators and is then discarded.

The bounded-memory guarantee applies to raw source framing, normalized-record lifetime, `SessionEvidence`, and report reduction. It does not initially apply to every retained collection needed for exact existing `SessionMetrics` output. The current analysis already grows in memory with transcript size; the first streaming implementation removes the worse whole-file `String` and complete canonical-event materialization while accepting that compact metrics state may still grow with metric-bearing events.

The implementation should proceed provider by provider:

1. add streaming transcript processing to the provider's existing session-metrics generation;
2. attach session-evidence processing to that same stream;
3. complete and persist truthful evidence for the provider;
4. make the provider's evidence available to the shared streaming report reducer;
5. repeat for each additional provider.

The first provider should be one JSONL provider, preferably Claude Code. This proves the shared architecture before adding more provider formats or generality.

### Product scope

#### Canonical Hygiene and Efficiency categories

The local report should preserve the complete set of finding categories represented by the private reference implementation under My Work → Insights → Hygiene and Efficiency.

| Category | What it finds | Current remediation concept |
|---|---|---|
| **Sessions Over Depth** | Individual requests whose context exceeds the evidence-driven autocompact cap. This is request depth, not merely a long-running session. | Lower the autocompact window, compact at task boundaries, or move open-ended exploration into a subagent. |
| **Model Overthinking** | Claude Code or Codex sessions using reasoning/thinking tiers above the reviewed recommended cap. | Lower the tier with the provider's effort/model controls. Prompt keywords are not inferred as explicit reasoning-tier evidence. |
| **Overpowered Subagents** | Premium main-loop models silently spawning subagents on the same premium tier. | Configure a cheaper default or per-agent subagent model while retaining premium subagents when deliberately justified. |
| **Unused MCP Servers** | MCP definitions loaded into eligible sessions but never directly invoked. | Remove the server, deny the connector, or scope it to projects that use it. |
| **Unused Built-In Tools** | Native harness tools whose definitions consume context but are not used. | Disable only where the tool, lost capability, and safe disable mechanism are known. Otherwise keep the result audit-only. |
| **Unused Skills** | Skills repeatedly loaded but never invoked, grouped by installed, project, plugin, or bundled origin. | Remove, narrow, disable, or uninstall the relevant skill/plugin where a safe mechanism exists. |
| **Old Model Usage** | Use of a curated deprecated model after its reviewed replacement became available. | Select the replacement model or update the provider's default model. |
| **Overuse of Fast Mode** | Fast-tier usage in delegated work or left enabled as a standing default. | Use fast mode deliberately for latency-sensitive interactive work and disable it for delegation-heavy or non-urgent work. |
| **Cache Churn** | Tokens paid or resent because of idle expiry, compaction, model switching, or provider cache eviction. | Finish or hand off before expiry, compact at appropriate boundaries, start fresh when context is unnecessary, and avoid needless model changes. |

Each category always has one detector status:

- findings;
- clean;
- not assessed with a structured reason.

An empty findings list does not prove a detector ran cleanly.

#### Additional local category: subscription/quota limit pressure

Antiburn should also support an explicitly local extension for subscription or quota-limit pressure. This remains separate from the nine-category private compatibility contract because provider limits may represent:

- rolling five-hour usage;
- weekly usage;
- model-specific allocation;
- weighted usage rather than raw tokens;
- rate-limit errors without an exposed numeric quota.

The section is sourced **only** from transcript-observed limit errors attributable to a session, including reset times and utilization where the transcript itself exposes them. It reports limit kind, hit count, affected sessions and models, and observed times. When the transcript carries no quota evidence the section is not assessed.

Account-level limit state is a different subject, from a different subsystem, on a different schedule. Looking it up is out of scope for this issue (see **Out of Scope**).

### Non-negotiable correctness semantics

#### No false absence

Detector-critical evidence must distinguish:

```text
Complete(value)       the capability was supported and completely observed
Partial(value, reason) some evidence was observed, but absence cannot be concluded
Unsupported           this provider/source/schema cannot expose the evidence
```

A complete zero or empty collection means an event did not happen. `Unsupported` and `Partial` never mean zero.

During development of the first provider only, a temporary debug-only state may be used:

```rust
pub enum EvidenceValue<T> {
    #[cfg(debug_assertions)]
    Unimplemented,
    Unsupported,
    Partial {
        observed: T,
        reason: CoverageReason,
    },
    Complete(T),
}
```

`Unimplemented` is implementation scaffolding. It must be removed entirely before the first provider implementation is complete. Any remaining use should make a release build fail while the variant is debug-only; after removal, remaining debug references should also fail.

#### Capabilities and coverage are separate

The system records two distinct concepts:

1. **Source/parser capabilities:** what a provider, source format, and observed schema/version can reliably expose.
2. **Per-session evidence coverage:** whether this particular source was parsed completely enough to conclude presence or absence.

Discovery carries cheap provider/source/version hints. The parser returns the definitive capabilities, provenance, coverage, and diagnostics after inspecting source records.

#### Findings are policy; evidence is fact

Persist facts such as token quantities, tool counts, model usage, context depths, and observed transitions. Do not persist conclusions such as `overthinking = true` or final savings.

This allows thresholds, detector rules, remediation text, and pricing to change without reparsing transcripts.

### Ownership boundaries

#### `antiburn-local` owns

- provider discovery;
- source identity and provider-aware source version calculation;
- stable bounded source opening;
- provider-specific record parsing;
- normalized evidence-bearing records;
- shared record-level classification;
- `SessionMetrics` accumulation;
- `SessionEvidence` accumulation;
- source capabilities, provenance, coverage, and diagnostics;
- pure detector logic;
- pure cross-session report accumulation.

#### Desktop application owns

- Antiburn's application SQLite schema and migrations;
- durable pending/processing/ready state;
- work claiming, leases, retries, and wake-up events;
- CPU, source, provider-DB, and memory admission limits;
- report-population queries and read-only database connection;
- Tauri commands/events;
- settings-pane UI;
- clear/delete integration.

`antiburn-local` must remain storage-neutral and must not depend on Tauri or Antiburn's application database schema.

### Target source and processing contracts

#### Source descriptor and version

Candidate storage-neutral types in `antiburn-local`:

```rust
pub struct SourceDescriptor {
    pub agent: AgentKind,
    pub session_id: String,
    pub environment: DiscoveryEnvironment,
    pub source: SessionSource,
    pub updated_at_epoch: Option<i64>,
}

pub struct SourceVersion {
    pub fingerprint: String,
    pub estimated_bytes: Option<u64>,
    pub streamability: Streamability,
}

pub enum Streamability {
    RecordStream,
    DatabaseRows,
    WholeDocumentFallback,
    InlineMaterialized,
}
```

Fingerprint policy:

- file: the fingerprint contract of Locked Decision 16, which is its one home;
- provider DB: reuse and strengthen provider-specific fingerprints;
- inline: hash the already-materialized content or mark always-refresh where hashing is unavailable;
- format is internal and versioned.

#### Bounded normalized-record visitor

The existing compatibility API remains available:

```rust
normalize_source(&SessionInput) -> Result<NormalizedSession>
```

The normal metrics/evidence path should use a lower-level visitor:

```rust
pub trait NormalizedRecordSink {
    fn observe(&mut self, record: NormalizedRecord) -> ControlFlow<()>;
}

pub trait VendorAdapter: Sync {
    fn visit_source(
        &self,
        input: &SessionInput,
        sink: &mut dyn NormalizedRecordSink,
        diagnostics: &mut ParseDiagnostics,
        control: &ProcessingControl,
    ) -> anyhow::Result<SessionProvenance>;
}
```

`normalize_source` can later be implemented as a collector over this visitor for tests, tools, and views that truly need a complete normalized session.

#### Normalized record model

A provider-neutral record enum may be more honest than forcing every detector fact into message-shaped `NormalizedEvent`:

```rust
pub enum NormalizedRecord {
    MetricsEvent(NormalizedEvent),
    ModelTurn(NormalizedModelTurn),
    ContextSource(ContextSourceObservation),
    ToolDefinition(ToolDefinitionObservation),
    Subagent(SubagentObservation),
    Compaction(CompactionObservation),
    UsageLimit(UsageLimitObservation),
}
```

The variant is named `MetricsEvent`, not `Event`, so the record that feeds the existing metrics path is named for what it is. **The rename is on the variant only.** The `NormalizedEvent` type at `analysis/model.rs:234` keeps its name: it is existing code with serde derives that appear in `NormalizedSession`, which the CH-001 characterization goldens serialize, and renaming it would churn fixtures inside the seam whose job is proving nothing changed. `Turn` was rejected because it collides with the existing `ModelTurn` variant.

Only add variants or fields required by an implemented metric, detector, or established cross-session view.

#### Composite per-session processing

One parsed record feeds multiple bounded accumulators:

```text
bounded source record
  → provider normalization
  → normalized record
      ├── SessionMetricsAccumulator
      └── SessionEvidenceAccumulator
```

At end of stream:

```rust
pub struct ProcessedSession {
    pub metrics: SessionMetrics,
    pub evidence: SessionEvidence,
    pub provenance: SessionProvenance,
    pub diagnostics: ParseDiagnostics,
    pub processed_source_version: SourceVersion,
}
```

The source line, parsed JSON value, and normalized record are discarded before the next source record is retained.

#### Rich metrics and retained state

Some existing `SessionMetrics` fields require finalization after the complete event sequence:

- active-time normalization;
- timeline segments;
- phase buckets;
- timestamp ordering;
- per-invocation skill data.

Exact parity with the current metrics contract can require retained state proportional to the number of metric-bearing events. Some output collections, including phase segments and skill uses, can themselves grow with session size. The first implementation accepts this existing unbounded metrics behavior while making it substantially less expensive: retain compact timestamp/phase/skill facts only where exact finalization needs them, and do not retain raw text, whole JSON values, tool-output payloads, the complete transcript `String`, or complete normalized events.

A later measured improvement may move growing metric details into generation-scoped child/staging tables or another disk-backed spool and retain only bounded summaries in memory. That optimization may require changing the rich metrics output contract or finalization path and is not required for the first streaming provider.

### JSONL source policy

#### Full forward processing

Antiburn should process the complete JSONL stream by default, even when the file exceeds 512 MiB. Streaming removes the memory reason for a total-file cap, though CPU/I/O budgets and cancellation still matter.

#### Bounded newline framing

Plain `BufRead::read_until` is insufficient by itself because its destination buffer can grow without bound. Implement a bounded newline-framed reader using `fill_buf` or equivalent chunk scanning:

1. scan buffered bytes for `\n`;
2. retain chunks only until the configured record limit;
3. if the record exceeds the limit, stop retaining and drain through the next newline;
4. record an oversized-record diagnostic;
5. continue at the next record;
6. mark affected capabilities/session coverage partial.

The provider controls row size; Antiburn controls how much it retains and parses.

**The maximum retained record size is 8 MiB.** It is a safety valve against one pathological record, not a transcript-size limit — there is no total file cap (Locked Decision 9). Required failure behavior: drain the oversized record through its newline, mark the affected coverage `Partial` with a reason, and resume at the next record. Never truncate a record into evidence and never abort the source. Evidence for the number: measured local Claude transcripts top out near 79 KB per record, and repository precedent for bounded reads sits at 8-64 MiB (`SOURCE_PREVIEW_BYTES` 16 MiB, `antigravity.rs:HISTORY_MAX_BYTES` 16 MiB, opencode's 8/32/64 MiB caps).

#### Malformed and trailing records

**The newline marks a record complete for the current pass.** A raw `\n` in JSONL can only mean end-of-record, because JSON escapes newlines inside strings. Two properties follow, both scoped to one read. A partially written record has no newline yet, so it is the incomplete tail and is never committed. A line that has its newline is complete for this pass, which is what lets the framing primitive commit that record and what makes prefix membership decidable.

**The newline says nothing about whether those bytes persist across reads.** A later rewrite of an already-complete record is a source mutation, handled by the **Source-validity outcomes** table like any other mutation. Newline framing decides record boundaries within one read; the append-only guarantee decides whether a prefix may publish. They are separate properties, and framing does not supply the guarantee.

**This is why there is no in-loop retry on a malformed line.** Within a pass, a newline-terminated record is already complete, so re-reading it in that same pass cannot yield more of it. Retry belongs at generation granularity instead: a changed source produces a new generation, and that generation is reprocessed whole.

- Parse each complete line independently.
- A malformed line does not discard valid surrounding records.
- An incomplete final line is not treated as committed evidence.
- Skipped records update diagnostics and evidence coverage.
- Cancellation is checked between records or bounded byte intervals.

#### Source mutation and actively growing transcripts

- Capture source version before reading.
- Recheck source identity/version after processing.
- If the source changed, return `SourceChanged` and do not publish stale projections.
- The next generation remains pending.

`SourceChanged` is a **new** result variant introduced by CH-004 on the visit entry point's result type in `analysis/interface.rs`. Today's path has no post-read recheck at all (see **Current State**).

##### Source-validity outcomes

This is the **one home** of what publishes and when. One source pass produces one source-validity result, and **both projections use that same result**: metrics and evidence publish together, or neither publishes. FR-5 requires one pass and FR-7 requires one atomic completion, so an outcome that publishes metrics while rejecting evidence is not available to any seam. CH-004, CH-005, CH-007, and CH-008 reference this table rather than restating it.

| Outcome | Condition | What publishes |
|---|---|---|
| **Accepted full read** | The post-read recheck confirms the same source version over the whole source | Metrics and evidence, atomically, for the claimed generation |
| **Accepted pinned prefix** | The provider carries an evidence-backed append-only guarantee **and** the prefix recheck passes | Metrics and evidence, atomically, for that generation, stamped with its boundary `L` |
| **`SourceChanged`** | Any source mutation the recheck classifies as changed. For a **full read** that is the whole-source recheck failing. For a **pinned-prefix read** it is narrower: the pinned prefix could not be obtained intact — a truncation, or a head-region rewrite detected by re-hashing from the pinned handle. **Truncation has two timings and both count:** it surfaces inline as a short read before reaching `L`, or, when it happens after `[0, L)` was already read, only as a post-read pinned-handle size below `L`. Appends past `L` are invisible to a prefix read and **never** cause it; that acceptance is deliberate and is not a defect to fix. An in-place rewrite below `L` and outside the head region is undetectable here; the provider append-only guarantee covers that case, not the recheck | **Neither projection.** The newer generation stays pending and the last completed payload is preserved |
| **Handle is not the claimed generation** | The pre-read validation from the opened handle fails: stable identity does not match the claimed generation, size is short of the claimed prefix boundary `L` where one applies, or the head-hash envelope the generation recorded does not match. This is a **replacement before pinning** — a rename between version capture and `open` — and it is checked from the opened handle before any record streams. It applies to a full read and a prefix read alike | **Neither projection**, reported as `SourceChanged`. The generation stays pending, the last completed payload is preserved, and the work retries under the bounded backoff below |
| **No append-only guarantee** | The provider has no evidence-backed guarantee | **No prefix publication at all.** The source takes the normal full-reprocess path, with the `SourceChanged` row above applying unchanged |

A transcript that is still receiving appends must not enter an immediate discard/retry loop or monopolize worker capacity. **Nothing defers an active session** (Locked Decision 15). Instead:

- set `next_attempt_at` instead of immediately retrying `SourceChanged` work;
- apply bounded backoff when a source repeatedly changes during processing;
- claim eligible rows fairly so one hot session cannot prevent stable sessions from progressing;
- retain the last completed `analyzed_generation` and evidence payload while a newer generation is pending;
- exclude stale/pending evidence from a clean current report and count the active session as not assessed.

Marking a new generation pending must not erase its last completed evidence.

**Pinned-prefix reads.** A still-growing source is read as a pinned prefix (Locked Decision 8). The boundary needs no new state: the fingerprint carries `size`, so a generation already encodes its own read length `L = size`. Stream records normally but **stop at byte `L`** rather than at EOF.

- *Membership rule:* a record belongs to the prefix when its **terminating newline is at or before `L`**. That is the same rule as the incomplete tail, with no special case. It is chosen for reproducibility: **given the append-only guarantee below**, `(file identity, L)` determines the evidence exactly, so reprocessing a generation is byte-identical and testable. That reproducibility is a consequence of the guarantee. It is not a property the recheck can prove on its own.
- *Nothing is lost:* a record straddling `L` is picked up by the next generation, whose `L'` includes it.
- *Pre-read validation:* the lifecycle has three points — version capture at discovery, `open`, then the read and its recheck. Pinning the handle covers only what happens **after** `open`. A rename between capture and `open` silently substitutes a different file, so the worker validates the opened handle against the claimed generation **before it streams any record**: stable identity, size at least `L`, and the head-hash envelope the generation recorded, every input read from that handle. The outcome of a mismatch is stated once in the **Source-validity outcomes** table above. **Replacement before pinning and rename after pinning are opposite cases:** the first is rejected and retried, the second is accepted on the original inode.
- *Pinned handle:* the read opens the source **once** and reads `[0, L)` from that one handle. Every recheck input comes from that handle, never from the path. **Replacement after the handle is pinned is not a failure.** If the path is replaced by a rename over it mid-read, POSIX keeps the descriptor on the original inode, so the read completes on exactly the bytes the claimed generation described. That result is correct, not an error, and the replacement arrives as a new generation at the next discovery through the normal path. **Implementation trap:** an implementer who re-stats *the path* after the read turns a harmless replacement into a spurious rejection. The recheck must use the pinned handle.
- *Recheck:* taken from the pinned handle — size still `>= L`, and the head region unchanged when re-hashed (Locked Decision 16). An append passes and is invisible. What counts as a failed prefix recheck is stated once in the **Source-validity outcomes** table above; this bullet does not restate it.
- **Detection envelope, stated plainly.** That recheck does **not** verify the append-only property of the whole prefix. The head hash covers the hashed head region only (Locked Decision 16). A writer can rewrite a complete record after the head region and before `L` while preserving file identity, a non-shrinking size, and the head bytes, and the recheck passes. Hashing the whole prefix would be exactly the unbounded work this plan exists to avoid, so this plan does not add it.
- **Publication is therefore conditional on an evidence-backed provider append-only guarantee**, established per provider by test in the seam that builds the mechanism (CH-004). Where the guarantee holds, a pinned prefix may publish. Where it does not, no prefix publishes and the source takes the full-reprocess path — see the **Source-validity outcomes** table above, which is the one home of both rules.
- *Header rewrites:* where a provider rewrites a header, the seam may define the prefix as `[header_end, L)` and re-read the header fresh each pass. Claude's header shape is not specified here; CH-004 carries the constraint and the per-provider note.

#### Selective deserialization

Provider-specific deserializers should avoid retaining huge prompt/tool-output fields when only metadata, usage, tool names, or mode fields are needed. Use ignored-field/visitor techniques where justified by fixtures. Temporary-file spooling remains a fallback only if a legitimate evidence-bearing record exceeds the chosen memory bound.

### Provider SQLite source policy

For supported provider databases:

1. open a dedicated read-only connection;
2. begin a read transaction for one consistent source snapshot;
3. query only the target session cluster and required columns;
4. order rows deterministically;
5. normalize one row at a time into the same record sink;
6. discard each provider row before stepping to the next;
7. close the statement, transaction, and connection immediately after the session fold.

Do not render the complete provider session into a synthetic JSONL `String`.

Provider database connections are distinct from the Antiburn application database connection. Provider journal mode is not controlled by Antiburn, so concurrent readers should be conservatively limited, initially to one unless provider-specific tests justify more. Do not use SQLite immutable mode for a database the provider may still be writing.

### Source capabilities, coverage, and provenance

The parser returns a `SessionProvenance` containing:

- provider;
- source kind and format;
- observed harness/schema version;
- parser revision;
- supported capabilities;
- ordering guarantee;
- malformed/skipped/oversized record counts;
- unknown schema variants;
- source mutation/truncation observations.

An **unknown schema variant degrades the affected evidence group to `Partial` with a reason**, exactly as a malformed record does. A diagnostic count alone is not enough: an unmodelled record must never let a group report `Complete`. The diagnostic also records a **bounded, capped set of unrecognized `type` values** — discriminator strings only, never payloads. A `type` discriminator is schema vocabulary and is safe under the same rules as tool names, but it is capped like every other cardinality-bearing field (CH-009's cap audit).

Capabilities should cover at least:

- model identity;
- input/output/cache token classes;
- request context depth;
- timestamps and canonical ordering;
- reasoning/effort tier;
- fast/service tier;
- tool definitions and invocations;
- skill/MCP source attribution;
- compaction boundaries;
- subagent relationships/models;
- transcript-observed quota incidents.

Pricing is not a source capability. The source provides model identity and billable token quantities; a versioned report-time catalog determines whether those quantities are priceable.

### `SessionEvidence` contract

Candidate shape:

```rust
pub struct SessionEvidence {
    pub schema_revision: u32,
    pub identity: SessionEvidenceIdentity,
    pub time_range: EvidenceValue<SessionTimeRange>,
    pub eligibility: EvidenceValue<EligibilityEvidence>,
    pub context: EvidenceValue<ContextEvidence>,
    pub models: EvidenceValue<ModelEvidence>,
    pub tools: EvidenceValue<ToolEvidence>,
    pub context_sources: EvidenceValue<ContextSourceEvidence>,
    pub subagents: EvidenceValue<SubagentEvidence>,
    pub cache: EvidenceValue<CacheEvidence>,
    pub compactions: EvidenceValue<CompactionEvidence>,
    pub quota_incidents: EvidenceValue<SessionQuotaEvidence>,
    pub capabilities: SourceCapabilities,
    pub coverage: EvidenceCoverage,
    pub diagnostics: ParseDiagnostics,
}
```

The exact grouping may evolve while implementing the first provider. The durable requirements are:

- facts are rule-neutral;
- evidence state is not ambiguous;
- maps and examples are bounded;
- strings and diagnostics are capped;
- provider/source provenance is retained;
- schema revision is explicit.

Persist model-attributed token quantities, not only current prices. Pricing changes should not require transcript reparsing.

### Database changes

#### Shared source version on `session`

Add source truth and a queryable analyzed start time to the existing `session` row:

```text
source_fingerprint TEXT
source_generation  INTEGER NOT NULL DEFAULT 0
started_at_epoch   INTEGER
```

When discovery first obtains a reusable fingerprint, generation becomes 1. When the fingerprint changes, generation increments. Re-observing the same fingerprint is idempotent.

Antiburn does not currently store a queryable session start on `session`; the earliest parsed timestamp exists only as `SessionMetrics.first_ts_ms` inside `metrics_json`. Streaming analysis should populate `started_at_epoch` from trustworthy provider start metadata or the earliest normalized timestamp. `first_seen_at` is discovery time and must not be used as a session-start fallback.

#### Version existing `session_analysis`

Add:

```text
analyzed_generation      INTEGER NOT NULL DEFAULT 0
parser_revision          INTEGER NOT NULL DEFAULT 1
analyzer_revision        INTEGER NOT NULL DEFAULT 1
metrics_schema_revision  INTEGER NOT NULL DEFAULT 1
```

Existing `source_fingerprint` remains useful diagnostic evidence. Freshness requires generation and revisions to match current values.

#### New `session_evidence` table

The table stores stable relational lifecycle/version columns plus a versioned JSON evidence payload. It does not create one SQL column for every map-valued fact.

```sql
CREATE TABLE session_evidence (
    environment_key          TEXT NOT NULL,
    agent                   TEXT NOT NULL,
    session_id              TEXT NOT NULL,

    status                  TEXT NOT NULL,
    analyzed_generation     INTEGER,
    processed_fingerprint   TEXT,

    parser_revision         INTEGER,
    analyzer_revision       INTEGER,
    evidence_schema_revision INTEGER,

    evidence_json           TEXT,
    diagnostics_json        TEXT,

    retry_count             INTEGER NOT NULL DEFAULT 0,
    claim_fence             INTEGER NOT NULL DEFAULT 0,
    claimed_at              TEXT,
    lease_expires_at        TEXT,
    next_attempt_at         TEXT,
    analyzed_at             TEXT,
    last_error              TEXT,

    PRIMARY KEY (environment_key, agent, session_id),
    FOREIGN KEY (environment_key, agent, session_id)
      REFERENCES session (environment_key, agent, session_id)
      ON DELETE CASCADE,

    CHECK (status IN (
      'pending', 'processing', 'ready', 'unsupported', 'failed'
    ))
) STRICT;

CREATE INDEX session_evidence_status
    ON session_evidence (status, next_attempt_at, lease_expires_at);
```

Work lifecycle and evidence quality remain separate:

- lifecycle: pending, processing, ready, unsupported, failed;
- evidence quality: complete or detector-specific partial/unsupported fields inside `evidence_json`.

#### Dirty marking

When discovery observes a new source generation:

```text
session.source_generation += 1
session.source_fingerprint = observed fingerprint
session_evidence.status = pending
session_evidence.last_error = null
session_evidence.evidence_json remains the last completed generation until replacement
```

The session upsert and evidence transition happen in one short transaction. Worker notification occurs only after commit.

#### Revision-driven requeue

A source can require reprocessing even when its fingerprint and generation did not change. On startup and before normal worker claiming, reconcile enabled-provider rows against current projection revisions. Mark or treat a row as pending when any transcript-derived projection is stale:

```text
session_analysis.analyzed_generation != session.source_generation
OR session_analysis.parser_revision != CURRENT_PARSER_REVISION
OR session_analysis.analyzer_revision != CURRENT_ANALYZER_REVISION
OR session_analysis.metrics_schema_revision != CURRENT_METRICS_SCHEMA_REVISION
OR session_evidence.analyzed_generation != session.source_generation
OR session_evidence.parser_revision != CURRENT_PARSER_REVISION
OR session_evidence.analyzer_revision != CURRENT_ANALYZER_REVISION
OR session_evidence.evidence_schema_revision != CURRENT_EVIDENCE_SCHEMA_REVISION
```

Do not increment `session.source_generation` for a code/schema revision; the source did not change. Requeue stale projections lazily through the normal bounded worker so an application update cannot create an uncontrolled processing spike.

Invalidation policy:

| Change | Reprocess transcript |
|---|---:|
| Parser revision | Yes |
| Analyzer revision | Yes |
| Metrics schema revision | Yes |
| Evidence schema revision | Yes unless an explicit data-only migration can upgrade it safely |
| Detector thresholds/rules | No |
| Remediation/catalog wording | No |
| Pricing catalog | No for evidence/report facts |
| Deprecated-model replacement catalog | No |

#### Claim fencing and atomic completion

Source generation is not a worker-claim token: two workers may process the same generation after a lease expires. Every initial claim or reclaim must atomically increment `claim_fence` and return the new value. The worker carries both its claimed source generation and fence token.

Lease renewal, success, failure, and retry transitions must match:

```text
status = processing
source generation = claimed generation
claim_fence = claimed fence
```

A reclaimed worker receives a newer fence, making every late transition from the older worker a no-op.

At completion, write `session.started_at_epoch`, `session_analysis`, and `session_evidence` in one short transaction only if the claimed generation still matches `session.source_generation` and the current processing row still carries the worker's claim fence. Conceptually:

```sql
UPDATE session_analysis ... analyzed_generation = ?claimed_generation ...;
UPDATE session_evidence ... status = 'ready', analyzed_generation = ?claimed_generation ...;
```

If either guard fails, commit neither projection. A source change leaves the newer generation pending; a fence mismatch means another claim owns the work.

#### Persistence policy

- Persist compact derived facts only.
- Do not persist complete canonical events or raw transcript text.
- Start with serde JSON consistent with existing `metrics_json` conventions.
- Include evidence schema revision.
- Use normalized child tables only if measured query/update requirements justify them later.
- Include `session_evidence` in clear/delete and local-data documentation.

### Durable worker model

SQLite is the durable queue. An in-process event only wakes the worker.

#### Worker lifecycle

```text
scan transaction marks pending
  → commit
  → wake worker
  → worker claims one generation and increments/receives its claim fence
  → release Antiburn DB lock/transaction
  → acquire resource permits
  → process source in blocking job
  → reacquire Antiburn DB briefly
  → conditional atomic completion
```

On startup, reconcile revision-stale projections and reclaim abandoned `processing` rows whose leases expired. Every reclaim increments the fence before work starts. Duplicate wake-ups are harmless.

#### Database connection rule

For the Antiburn application database:

- hold the store mutex/transaction only for short claims, state transitions, reads, and writes;
- never hold it across `await`, `spawn_blocking`, transcript I/O, parsing, analysis, or detector work;
- release statements and guards before doing unrelated work.

The current `Store` may retain its physical writer connection; releasing the guard/transaction is the relevant concurrency boundary.

#### Resource limits

Use separate controls rather than one dynamically resized thread pool:

- CPU/job concurrency;
- open source/file concurrency;
- provider SQLite reader concurrency;
- memory-weight permits for materialized fallbacks;
- maximum retained record bytes;
- maximum evidence cardinality;
- cancellation interval.

Acquire permits before scheduling a blocking job. Streaming JSONL receives a small fixed memory weight. Whole-document fallbacks receive weight based on estimated size and run alone when they consume the budget.

**Starting values: one CPU permit, one source permit, one provider-database permit, and a five-minute lease renewed on progress.** They are retunable in CH-013 from measurement. The bias is deliberate: a background utility competing with the reader's own agent sessions is a worse failure than a slow backlog.

### Cross-session report reduction

#### Dedicated read-only Antiburn connection

The current `Store` serializes one persistent connection behind a mutex. Report generation should open a separate read-only connection to the same WAL database.

In `spawn_blocking`:

1. open the read-only/query-only connection with a sensible busy timeout;
2. begin a read transaction to pin one consistent snapshot;
3. execute the thirty-day evidence query;
4. deserialize one compact row;
5. update the report accumulator;
6. discard the row before stepping to the next;
7. finalize the report;
8. drop the statement, transaction, and connection promptly.

WAL permits the normal writer connection to continue committing while the report sees its original snapshot. The report must remain bounded and must not await, read raw transcripts, or perform unrelated long-running work while the snapshot is open.

#### Report population

The first report is a **session-start cohort**, not an event-time slice. Select sessions whose trustworthy `started_at_epoch` falls within the trailing thirty days, then consume each selected session's complete evidence. Because every selected session began inside the window, its normal event history is also inside the cohort window.

This deliberately excludes a session that began before the window even if it remained active or was updated recently. The UI/report wording must say “sessions started in the last 30 days,” not imply that it includes every event from every session active during the period.

Select:

- `session.started_at_epoch >= window_start` and `< window_end`;
- current machine/environment scope;
- ready evidence;
- `session_analysis`/`session_evidence` generations matching `session.source_generation` where needed;
- current parser/analyzer/evidence revisions.

A session without a trustworthy start time is not eligible for the cohort and is counted as not assessed rather than assigned discovery time. Quota incidents carry their own transcript-observed times inside the selected sessions, so they need no separate window rule.

Count pending, actively growing, processing, failed, unsupported, stale, unknown-start, and ready rows separately.

> **Plan amendment (GH-70).** The passage above says a session without a trustworthy start is "counted as not assessed" but never says *which* such sessions are counted, and a pending session usually cannot have `started_at_epoch` yet because that value is written at completion. **FR-12** defines the population and owns the rule. In short: the report runs two bounded queries over the same pinned snapshot, window, and environment scope — a coverage-denominator count over every in-window session regardless of evidence lifecycle, and the assessed-cohort query above over its ready-and-current subset. The `Select:` list above therefore describes the **cohort** query only, not the denominator.

#### Candidate query shape

```sql
SELECT
    s.environment_key,
    s.agent,
    s.session_id,
    s.started_at_epoch,
    s.updated_at_epoch,
    s.cwd,
    s.surface,
    e.evidence_json,
    e.diagnostics_json
FROM session s
JOIN session_evidence e
  USING (environment_key, agent, session_id)
WHERE s.started_at_epoch >= ?window_start
  AND s.started_at_epoch < ?window_end
  AND e.status = 'ready'
  AND e.analyzed_generation = s.source_generation
  AND e.parser_revision = ?parser_revision
  AND e.analyzer_revision = ?analyzer_revision
  AND e.evidence_schema_revision = ?evidence_schema_revision
ORDER BY s.started_at_epoch, s.session_id;
```

The final query may join compact `session_analysis` fields where a report requires them, but it must not load canonical transcripts.

#### Provider-neutral accumulator

Candidate module:

```text
crates/antiburn-local/src/insights/
  mod.rs
  report.rs
  status.rs
  quota.rs
  detectors/
```

Candidate API:

```rust
pub struct EfficiencyReportAccumulator {
    // bounded shared cross-session state
}

impl EfficiencyReportAccumulator {
    pub fn observe_session(&mut self, session: SessionReportEvidence);
    pub fn observe_usage_limit(&mut self, evidence: UsageLimitEvidence);
    pub fn finish(self, context: ReportContext) -> EfficiencyReport;
}
```

The report engine is implemented once. Each provider maps its source into the shared `SessionEvidence` contract rather than receiving provider-specific report code.

#### Report contract

Include:

- computed time;
- window start/end and explicit session-start cohort semantics;
- source/evidence snapshot information;
- discovered count;
- ready/usable count;
- pending/processing/failed/unsupported/stale counts;
- eligible count per detector;
- findings;
- one status per detector;
- quota-limit pressure section/status;
- parser/analyzer/evidence/detector/pricing/catalog revisions;
- coverage and diagnostics summary.

Partial coverage may still support observed findings. A detector may report clean only when its required capabilities and coverage are sufficient.

### Detector evidence requirements

#### Sessions Over Depth

- request/turn context depth;
- model/harness context semantics;
- thread/sidechain identity;
- canonical ordering;
- compaction boundaries where relevant.

#### Model Overthinking

- explicit reasoning/effort tier;
- provider and model;
- turn/session counts;
- confidence that the setting was directly observed.

#### Overpowered Subagents

- parent/main-loop model;
- child identity/model;
- relationship confidence;
- observed override information where available.

#### Unused MCP Servers

- loaded server/tool definitions;
- normalized direct invocation names;
- source scope and eligible sessions;
- attribution coverage.

#### Unused Built-In Tools

- built-in definitions for the observed provider/version;
- normalized invocations;
- curated disable/remediation support;
- fleet/local validation status.

#### Unused Skills

- loaded names;
- installed/project/plugin/bundled origin;
- normalized invocations;
- eligible-session and attribution coverage.

#### Old Model Usage

- model identity per turn/session;
- timestamp;
- token/turn quantities;
- replacement catalog revision.

#### Overuse of Fast Mode

- explicit fast/service-tier signal;
- main-loop versus delegated work;
- persistent/default signal where available;
- provider-specific impact semantics.

#### Cache Churn

- canonical turn order;
- thread/sidechain identity;
- timestamps and idle gaps;
- model changes;
- cache read/write/fresh-input quantities;
- compaction boundaries;
- user-controlled versus provider-eviction confidence.

#### Subscription/quota limit pressure

Session-attributable evidence:

- provider;
- observed time;
- limit kind;
- hard hit versus warning;
- reset time/utilization where available;
- model/session attribution;
- confidence.

Incidents are deduplicated. There is no account-level input: the section is sourced from session transcripts only.

### Desktop IPC and UI

Candidate commands:

- `get_local_insights_report`;
- `get_local_insights_processing_status`;
- an explicit retry/refresh command only if the product needs it.

The first UI remains Settings → Insights.

Show:

- loading/report calculation state;
- evidence backlog status;
- coverage summary;
- findings;
- clean/not-assessed states;
- quota-limit pressure;
- local-only/freshness wording.

Do not emit raw prompts, tool inputs, transcript content, or unnecessary local paths through IPC or logs.

### Privacy and local-data policy

- Persist derived facts, not complete prompts or canonical events.
- Bound and sanitize examples, names, strings, and diagnostics.
- Avoid tool inputs unless a narrowly defined detector requires a redacted field.
- Never place transcript content in logs, Tauri events, or errors.
- Include evidence in clear/delete behavior.
- Never modify or delete provider source transcripts.
- Update the schema data-policy comment for new retained evidence.

### Implementation map (from the pinned revision)

These are the file paths the pinned revision expected to touch. They are hints for seam carving, not a contract; verify against live code.

#### `crates/antiburn-local`

Likely additions:

```text
src/discovery/source_version.rs
src/analysis/jsonl_reader.rs
src/analysis/stream.rs
src/analysis/evidence.rs
src/analysis/process.rs
src/insights/mod.rs
src/insights/report.rs
src/insights/status.rs
src/insights/quota.rs
src/insights/detectors/*.rs
```

Likely modifications:

```text
src/discovery/mod.rs
src/analysis/interface.rs
src/analysis/model.rs
src/analysis/vendors/*.rs
src/analysis/initial_context.rs
src/analysis/engine.rs
src/analysis/mod.rs
src/lib.rs
```

#### Desktop Rust application

Likely additions:

```text
apps/desktop/src-tauri/src/insights_worker.rs
```

Likely modifications:

```text
apps/desktop/src-tauri/src/store/schema.rs
apps/desktop/src-tauri/src/store/model.rs
apps/desktop/src-tauri/src/store/mod.rs
apps/desktop/src-tauri/src/scan.rs
apps/desktop/src-tauri/src/analytics.rs
apps/desktop/src-tauri/src/commands.rs
Tauri application setup/state registration
clear/delete/export/privacy paths
```

#### Desktop frontend

Likely modifications/additions:

```text
apps/desktop/src/lib/settingsPanes.ts
apps/desktop/src/views/SettingsView.tsx
Insights pane components
Tauri command types/hooks
coverage/finding/status views
```

## Scope Areas (backlog — NOT seams)

Dependency-ordered outcomes derived from the pinned revision's Phase 0-11 checklist. **The default carve is one seam per source phase**, with the phase's detailed checklist grouped into the fewest coherent commit slots the seam planner needs. Exactly one phase is split below, with its reason stated inline. A scope area is an outcome, not a seam: one area may still take more than one seam, and the seam planner decides the boundary against live code.

Tiers are **provisional** per **Approval Tiers**. The seam planner sets the real tier and the reviewer critiques it. Any area that changes persisted data, runtime coverage semantics, or a user-visible conclusion carries a Tier 3 floor; raise a tier rather than keeping the hint, never lower one.

- [ ] **CH-001 — Claude scope frozen and characterization baseline captured.** (Source Phase 0. Provisional Tier 1.) Acceptance: the exact Claude source variants in the first slice are written down; synthetic fixtures cover user, assistant, usage, model, tool, skill, compaction, error, and thinking records, repeated and out-of-order timestamps, malformed JSON between valid lines, an incomplete final record, **a well-formed record of an unrecognized `type`**, a generated many-record source, and a generated single-oversized-line source; the malformed-between-valid-lines fixture states what it proves — coverage degrades to `Partial` and neighbouring records are not discarded; the incomplete-final-record fixture states that the record is never committed and is picked up by the next generation; the unrecognized-`type` fixture states what it proves and asserts the current behavior for its exact record shape, because today's analysis API returns no coverage value to assert against; asserting `Partial` and **not** `Complete` against this same fixture is CH-004's acceptance; golden expectations exist for `NormalizedSession`, every currently displayed `SessionMetrics` field, initial-context and skill-description behavior, and Claude parent/subagent behavior; the pre-change `cargo fmt`/`clippy`/`test` results are recorded. Fixtures live in a new directory beside the existing ones — `crates/antiburn-local/tests/fixtures/claude_characterization/` — with a `README.md` created here, following the `tests/fixtures/initial_context/README.md` precedent. That README is the designated home of the Claude capability and coverage matrix, filled in by CH-009. **Fixtures are authored by hand from format knowledge, never captured** (`CONTRIBUTING.md`: fixtures must be synthetic and "redaction is not sufficient"). Format knowledge and Claude version history come from the private reference repository at `crates/harness-kb/facts/claude.json` — a versioned harness knowledge base with per-CLI-version entries and capture provenance, not a transcript. That repository's `crates/secret-redaction/tests/fixtures/session-logs/claude-code-session.jsonl` and its `.pii.redacted` goldens are **explicitly off limits**: no captured session file enters this repository. This scope area also creates `docs/plans/local-insights-followups.md` and seeds it (see **Deferred work and followups**). (Refs: FR-4; Locked Decisions 1, 13)
- [ ] **CH-002 — Source generations and projection revisions exist end to end.** (Source Phase 1. Provisional Tier 3 — schema migration plus changed discovery and persistence behavior.) Acceptance: migration V6 adds `source_fingerprint`, `source_generation`, and nullable `started_at_epoch` to `session` and adds `analyzed_generation`, `parser_revision`, `analyzer_revision`, and `metrics_schema_revision` to `session_analysis`; storage-neutral `SourceDescriptor`/`SourceVersion` types exist in `antiburn-local`; **`SourceVersion`'s fingerprint implements the one fingerprint contract of Locked Decision 16** — stable file identity where available, byte size, high-resolution modification/change time, and a hash of a small fixed head region — with the region size chosen here, explicitly not `SOURCE_PREVIEW_BYTES`, and the hash used as fingerprint input only, never surfaced, logged, exported, or retained as evidence; **two pure fingerprint-input tests isolate the head-hash component**, both holding stable identity, size, and modification/change time constant — through a fingerprint-input fixture rather than a real filesystem rewrite — so each asserts only that component's behavior: one proves a same-size rewrite _within the hashed head region_ changes the head-hash component, and the other proves a same-size rewrite _below_ that region does **not** change it; the claim is about the component and not the whole fingerprint, because an ordinary filesystem rewrite changes modification/change time and therefore changes the full fingerprint, which is correct and desirable, and the tests isolate the inputs precisely because that real behavior would otherwise mask the envelope being demonstrated; discovery computes a source version without reading a complete transcript, reusing the head bytes `discovery/mod.rs:session_source_preview` already reads; **the descriptor resolves the session id through the existing canonical order and does not introduce a second identity scheme** — `SessionMetadata::session_id` from the bounded metadata first, then `scan.rs:recovered_id` (`Explorers::recover_session_id_from_path`, file stem, `ProviderDb` session id, `Inline` label), with the empty-id rejection preserved, and a test proves a session keeps its existing `SessionKey` across the migration; the upsert transaction increments the generation only on a fingerprint change and is idempotent otherwise; provider-DB sources reuse `provider_db_fingerprint` and inline sources have defined hashing or always-refresh behavior; `started_at_epoch` is never backfilled from `first_seen_at`; `activity_source` and `activity_cursor` mappings are preserved; fresh migration and migration from every prior schema version pass; existing scan behavior is unchanged. Likely touches: `store/schema.rs`, `store/model.rs`, `store/mod.rs`, `scan.rs`, `analytics.rs`, `crates/antiburn-local/src/discovery/`. (Refs: FR-1, FR-11; Locked Decisions 10, 11)
- [ ] **CH-003 — Bounded JSONL framing primitive.** (Source Phase 2. Provisional Tier 2 — new module, no shipped behavior change and nothing persisted.) Acceptance: a synchronous bounded newline-framed reader exists in `antiburn-local` with results for complete, malformed, oversized, incomplete-tail, cancelled, and I/O-error records; **the retained record bound is 8 MiB**, and an oversized record is drained through its newline, marks coverage `Partial` with a reason, and never truncates into evidence or aborts the source; it uses chunk scanning rather than unbounded `read_until`; it drains an oversized record and resumes at the next one; it checks cancellation between records or bounded byte intervals; tests assert the retained buffer never exceeds its bound on a source larger than that bound, that diagnostics contain no transcript content, and that an incomplete final record is not committed. Likely touches: `crates/antiburn-local/src/analysis/` (new reader module). (Refs: FR-2; Locked Decision 9)
- [ ] **CH-004 — Claude normalization runs record by record.** (Source Phase 3. Provisional Tier 2 — the shipped `normalize`/`analyze` output must not change and the new path is not yet wired to the desktop; raise to Tier 3 if the seam wires it in.) Acceptance: a record sink interface and a `VendorAdapter` visit entry point exist without breaking the six existing adapters; Claude file input flows through the bounded reader, parsing one JSON object per retained line and dropping the `serde_json::Value` before the next record; malformed-record tolerance, model and context-window semantics, tool categorization and naming, compaction markers, and sidechain identity are preserved; the record enum names the metrics-bearing variant `NormalizedRecord::MetricsEvent`, leaving the `NormalizedEvent` type name unchanged; a collector sink rebuilds `NormalizedSession` and matches the legacy normalizer fixture by fixture; **an unrecognized record `type` degrades the affected evidence group to `Partial` with a reason and never leaves it `Complete`**, proven against CH-001's unrecognized-`type` fixture; source version is captured before opening and rechecked after the final record, returning **`SourceChanged` — a new result variant introduced here on the visit entry point's result type in `analysis/interface.rs`** — instead of publishing; the seam implements the **Architecture reference → Source-validity outcomes** table exactly and does not restate or vary it; a still-growing source may be read as a **pinned prefix** bounded by the generation's own `L = size`, admitting a record only when its terminating newline is at or before `L`, with the source opened once and every recheck input taken from that pinned handle — size still `>= L` and the head region unchanged; **the opened handle is validated against the claimed generation before any record streams**, so a replacement *before* pinning is rejected and retried while a rename *after* pinning completes on the original inode and is **not** `SourceChanged` (**Architecture reference → Source-validity outcomes**, which is the one home of both conditions and of what a failed prefix recheck means); **tests distinguish those two timings** — one replaces the path between version capture and `open` and asserts neither projection publishes and the generation retries, the other renames over the path after the handle is pinned and asserts the read is accepted on the original inode; **truncation is tested at both its timings** — an inline short read before reaching `L`, and a truncation after `[0, L)` was read that only the post-read pinned-handle size below `L` catches — while an append past `L` stays accepted; and a test proving two reads of the same `(identity, L)` produce byte-identical evidence **when the append-only guarantee holds**; **the seam states the head hash's detection envelope in code comments and tests rather than implying whole-prefix integrity** — a rewritten complete record after the head region and before `L` passes that recheck; **this seam determines the provider's append-only guarantee by test, cites the evidence its tests rest on, and records the guarantee as absent when it cannot be evidenced** — the guarantee is a property of how the provider writes, so proving it is a test rather than a reading exercise, and the test belongs in the same seam as the mechanism it protects; **a pinned prefix publishes only where the guarantee is evidenced (Locked Decision 8), and absent it the source takes the full-reprocess path with `SourceChanged` behavior unchanged**, with a test for the no-guarantee path; if a header is rewritten the prefix is defined as `[header_end, L)` with the header re-read fresh each pass. Likely touches: `analysis/interface.rs`, `analysis/vendors/mod.rs`, `analysis/vendors/claude.rs`, `analysis/mod.rs`, `analysis/model.rs`. (Refs: FR-2, FR-3; Locked Decisions 1, 8, 13, 16)
- [ ] **CH-005 — Claude `SessionMetrics` are produced by the streaming pass.** (Source Phase 4. Provisional Tier 3 — the desktop's metrics generation path changes and new persisted columns are written.) Acceptance: a `SessionMetricsAccumulator` updates counts, tokens, billable classes, model breakdown, peak context, tool mix, errors, skills, and compactions online and finalizes duration, active time, buckets, phase segments, and pattern signals at end of stream; initial-context and skill-description attribution come from the same pass with no transcript reread; parent and subagent sources stream independently while preserving roster and relationship output; the desktop Claude path routes through the streaming processor inside `spawn_blocking`, writes `analyzed_generation` and the parser/analyzer/metrics revisions, and derives `started_at_epoch` from trustworthy start metadata or `first_ts_ms`, leaving it unknown otherwise; equivalence tests compare every metric, cost, tool, skill, context, bucket, phase, pattern, and subagent output; **metrics and evidence use the same source-validity result — this is locked, not a seam decision** (**Architecture reference → Source-validity outcomes**; FR-5 and FR-7 leave no room for metrics to publish while evidence is rejected); **acceptance: an accepted read refreshes metrics, including an accepted append-prefix where CH-004 evidenced the append-only guarantee, while any source mutation classified as `SourceChanged` publishes neither projection**, with a test for each of those two outcomes; **a further test asserts metrics keep refreshing on the shipped trigger set — launch, a `TICK` while the popover is visible, popover open, and on demand — with no quiet-period gate anywhere and no new or removed trigger** (FR-10, Locked Decision 15); measurements show no whole-file `String` and no complete `Vec<NormalizedEvent>` in this path, and document the remaining proportional growth of retained metrics state; other providers keep the existing path. Likely touches: `analysis/engine.rs`, `analysis/initial_context.rs`, `analysis/mod.rs`, `apps/desktop/src-tauri/src/analytics.rs`. (Refs: FR-3, FR-4, FR-5, FR-10; Locked Decisions 12, 13, 14, 15)
- [ ] **CH-006 — One truthful in-memory evidence value from the same pass.** (Source Phase 5. Provisional Tier 2 — no persistence and no IPC.) Acceptance: the anticipated `SessionEvidence` grouping, capabilities, coverage, provenance, and diagnostics types exist with the debug-only `Unimplemented` variant for unfinished groups; `max_request_context_tokens` is accumulated online as a real `Complete`/`Partial`/`Unsupported` value with parser and source coverage attached; a composite sink updates metrics and evidence from one record without a second pass; `SessionMetrics` output is unchanged; tests assert the fixture's maximum depth, partial coverage after a malformed or oversized relevant record, unsupported source semantics, complete-zero versus unsupported, and the serde shape. Likely touches: `crates/antiburn-local/src/analysis/` (evidence module, composite sink). (Refs: FR-5, FR-6; Locked Decision 3)
- [ ] **CH-007 — Durable `session_evidence` storage with atomic dual projection writes.** (Source Phase 6. Provisional Tier 3 — new table and new persisted data.) Acceptance: an appended migration creates `session_evidence` with lifecycle, generation, revision, payload, retry, fence, lease, and next-attempt columns, its status check, its lifecycle index, and a cascading composite foreign key; store methods cover pending marking that preserves the last completed payload, revision reconciliation that does not touch `source_generation`, an atomic claim that increments and returns `claim_fence`, and fence-guarded completion, failure, retry, lease renewal, and reclaim; session deletion and clear-local-session-data remove evidence; completion writes `session.started_at_epoch`, `session_analysis`, and `session_evidence` in one short transaction or nothing, which is the persistence half of the **Architecture reference → Source-validity outcomes** rule that both projections publish together or neither does; debug builds serialize the current evidence shell; tests cover first-claim exclusivity, reclaim fencing, late-fence rejection, stale-generation rejection, revision-driven requeue, catalog changes causing no requeue, payload round-trip, cascade, clear, and reclaim of abandoned processing. Likely touches: `store/schema.rs`, `store/model.rs`, `store/mod.rs`, `apps/desktop/src-tauri/src/analytics.rs`. (Refs: FR-6, FR-7, FR-8, FR-11, FR-18; Locked Decisions 2, 4, 5, 10)
- [ ] **CH-008 — Processing decoupled from scan by a restart-safe durable worker.** (Source Phase 7. Provisional Tier 3 — runtime lifecycle, concurrency, and failure behavior change.) Acceptance: a worker module claims one generation at a time from the SQLite queue, carries its fence through every transition, releases the store guard before source processing, runs parsing in `spawn_blocking`, and reacquires the store only for short transitions; the scan marks new generations pending for the enabled streaming cohort only, wakes the worker after commit, and does not wait for the backlog; the existing `top_up_analysis` entry point has a stated relationship to the worker and no generation is processed twice; one CPU permit, one source permit, and one provider-DB permit are acquired before scheduling, under a five-minute lease renewed on progress; **no quiet-period gate excludes an active session from claiming** (Locked Decision 15); claims are ordered fairly, `SourceChanged` publishes neither projection and uses bounded backoff through `next_attempt_at` rather than a tight loop, per **Architecture reference → Source-validity outcomes**, unsupported schemas are not retried forever, errors carry no transcript content, and shutdown releases cleanly; **the missing-source case is decided explicitly rather than falling out of `MISSING_FINGERPRINT`: a source that cannot be stat-ed is marked missing and stops being claimed instead of retrying forever**; tests prove a repeatedly changing transcript backs off without a tight loop, stable sessions keep processing while one source keeps changing, a newer pending generation preserves the last completed payload, an un-stat-able source stops being claimed, and a restart loses no pending work. Likely touches: new `apps/desktop/src-tauri/src/insights_worker.rs`, `scan.rs`, `analytics.rs`, `store/mod.rs`, `lib.rs`. (Refs: FR-8, FR-9, FR-10; Locked Decisions 4, 5, 12, 15)
- [ ] **CH-009 — Claude evidence complete, persisted, and placeholder-free.** (Source Phase 8, all ten evidence groups plus the completion gate. Provisional Tier 3 — every group changes the contents of persisted `session_evidence`.) Acceptance, in three parts.
  1. *Every group is truthful.* Context depths and eligibility counts with bounded top-depth examples carrying no prompt text; normalized named tool usage with built-in, MCP, and skill classification where evidence supports it; loaded skills with origin and loaded MCP servers and tool definitions, matched against invocations, taken from the one-pass observations; model identity and token quantities per normalized model, with the timestamps a replacement rule needs, unknown identity left unsupported or partial rather than invented, and replacement policy kept out of evidence; explicit reasoning and effort tiers counted with capability recorded by harness version and no inference from prompt keywords; parent and child models with relationship confidence and provenance and no sidechain double counting; explicit fast and service tier observations distinguishing main-loop from delegated work; canonical turn order, per-thread previous turn, cache read/write/fresh-input quantities, model transitions, idle gaps, and compaction boundaries, separating observed user-controlled churn from provider-eviction estimates; harness built-in definitions joined to invocation evidence; explicit rate and quota error shapes yielding provider, limit kind, observed time, model and session attribution, warning versus hard hit, and preserved confidence.
  2. *Every cardinality-bearing field is bounded.* Each group declares an explicit cap and overflows to `Partial` with a reason rather than growing: unique tool names, loaded skill and MCP source names and their descriptions, **the model-identity and per-model token map**, subagent child and example collections, thread and sidechain cardinality, **the quota-incident collection** (deduplication is not a bound), **the set of unrecognized record `type` discriminators** (discriminator strings only, never payloads), and every retained string, example, and diagnostic. A seam-level audit lists every field that can grow with transcript cardinality and names its cap. Tests drive each cap past its limit and assert the overflow-to-`Partial` result and the absence of unbounded growth. This satisfies **Architecture reference → `SessionEvidence` contract** ("maps and examples are bounded; strings and diagnostics are capped") and `CONTRIBUTING.md`'s memory constraint at implementation time, not at CH-013 measurement time.
  3. *No placeholder survives.* Every field behaves as `Complete`, `Partial`, or `Unsupported`; the debug-only `Unimplemented` variant and every temporary provider value are removed; debug tests, `cargo check --release`, and the release build and tests pass with no remaining placeholder reference; persisted evidence contains no placeholder; `SessionMetrics` output is unchanged throughout; the Claude capability and coverage matrix is written into `crates/antiburn-local/tests/fixtures/claude_characterization/README.md` (created in CH-001).

  Likely touches: `analysis/vendors/claude.rs`, the evidence accumulator and evidence types, `analysis/initial_context.rs`, evidence fixtures and their README. (Refs: FR-5, FR-6, FR-17; Locked Decisions 2, 3)

  > **Commit-slot guidance for the seam planner** (not seam boundaries): the source plan's ten groups batch naturally into four ordered slices — (a) 8.1-8.3 context and eligibility, named tool usage, loaded skills and MCP sources; (b) 8.4-8.7 model usage, reasoning effort, subagent relationships, fast and service tier; (c) 8.8-8.10 compaction and cache churn, built-in tool validation, transcript-attributable quota incidents; (d) the completion gate — remove `Unimplemented`, audit the caps, publish the matrix. Use them as commit slots inside one seam. They are not four scope areas: all four mutate the same Claude parser and evidence contract, the first three deliberately leave other groups `Unimplemented`, and the gate is a completion step rather than an independently shippable outcome.
- [ ] **CH-010 — Bounded thirty-day report reduction over database evidence.** (Source Phase 9, everything except detector rules. Provisional Tier 3 — it defines the report's cohort and coverage runtime semantics, which decide what a reader is later told was assessed.) Acceptance: a dedicated read-only, query-only connection with a busy timeout opens the antiburn database inside `spawn_blocking` and pins one read transaction; the **assessed-cohort** query selects only sessions with trustworthy `started_at_epoch` in the window, in the current environment scope, with ready evidence and matching generations and revisions, ordered deterministically; a **separate coverage-denominator** count over the same snapshot, window, and scope covers every in-window session regardless of evidence lifecycle or currentness, adds unknown-start sessions whose `updated_at_epoch` falls in the window, and excludes unknown-start sessions with no activity in the window, per FR-12 (denominator ⊇ cohort); rows are deserialized and folded one at a time and dropped before the next step, with no canonical session collection and no transcript read; coverage counts separate discovered, ready, pending, actively growing, processing, failed, unsupported, stale, and unknown-start sessions, and expose per-detector eligible and assessed counts; **acceptance tests cover a known-start pending row, a known-start processing row, a stale-generation row, and an unknown-start row with and without in-window activity**, asserting that each in-window row lands in the coverage denominator with its own reason, never in the assessed cohort, and never in any detector's eligible or assessed denominator, and that the unknown-start row with no in-window activity is absent from both; a provider-neutral accumulator holds bounded maps and examples, avoids cloning per detector, and finalizes after denominators are known; a concurrency test proves the report sees one consistent snapshot while a writer commits, that the writer is not blocked for the duration, and that the read transaction is dropped after finalization. Likely touches: new `crates/antiburn-local/src/insights/`, `store/mod.rs` (read-only opener). (Refs: FR-12, FR-13; Locked Decisions 6, 7)
- [ ] **CH-011 — Nine detector statuses plus the separate quota-pressure section.** (Source Phase 9, detector rules. Provisional Tier 3 — these are the conclusions the product asserts about the reader's own work, and the failure mode is a false clean.) Acceptance: Sessions Over Depth, Model Overthinking, Overpowered Subagents, Unused MCP Servers, Unused Built-In Tools, Unused Skills, Old Model Usage, Overuse of Fast Mode, and Cache Churn each produce exactly one status, with clean allowed only when required capabilities and coverage suffice; per-detector rules state which partial evidence permits a finding and which prevents clean; tests assert that incomplete absence never yields clean and that unknown-start and pending rows never enter a detector denominator; the quota-pressure section is sourced only from transcript-attributable incidents (the `quota_incidents` evidence group from CH-009), deduplicates them, and reports limit kind, hit count, affected sessions and models, and observed times; it is not assessed when the transcript carries no quota evidence — one condition, not a matrix — with a test for that case and a test for the findings case; it calls no provider endpoint, reads no account-level limit state, and stays outside the nine-category contract; pricing and remediation catalogs are applied at report time only. Likely touches: `crates/antiburn-local/src/insights/detectors/`, `insights/quota.rs`. (Refs: FR-14, FR-15; Locked Decision 2)
  > CH-010 and CH-011 split source Phase 9 because the read path with its concurrency and population proofs and the nine detector rule sets are independently reviewable and carry unrelated risk: one is a database, population, and memory-behavior change; the other is policy logic whose failure mode is a false clean.
- [ ] **CH-012 — Insights report exposed through desktop IPC and UI.** (Source Phase 10. Provisional Tier 3 — new IPC surface carrying derived personal data.) Acceptance: report and processing-status commands with DTOs that carry no transcript content are registered in the invoke handler; concurrent report requests are deduplicated where needed and request identity does not stand in for cancellation; report calculation state and evidence pending and processing counts are exposed; closing the pane or shutting down cancels report work without corrupting durable evidence state; a Settings → Insights pane is registered in `settingsPanes.ts` and renders loading, backlog and coverage, findings, per-category clean and not-assessed states, the quota-pressure section, and local-only freshness wording; **the pane presents the coverage denominator separately from the assessed cohort per FR-12, so no denominator row outside the cohort — pending, processing, failed, unsupported, stale, or unknown-start — can read as assessed or as clean**; **an `InsightsSession` class lives in `apps/desktop/src/views/settings/`, following `SourcesSession.ts` and `SettingsWindowSession.ts`, and owns the report IPC call, the in-flight state, the error state, and an immutable snapshot read through `useSyncExternalStore`**; **the pane itself is presentational** — it renders a snapshot and calls session methods, holds no fetch logic, and has **no dependency on being inside the settings window**; **portability is proven, not asserted: a test mounts the pane from a second entry point with no change to the pane itself**, while building the actual second window is out of scope; the pane derives during render or acts in the causing event, with no `useEffect` — the portability requirement and `AGENTS.md`'s no-`useEffect` rule select the same design, and `SettingsWindowSession`'s doc comment already states the principle and cites `OnboardingSession` as the precedent for a different window; **the first-open experience shows coverage and pending progress prominently, and an incomplete report never renders as an empty or clean state**; **the "nothing has been processed yet" case is named explicitly and carries its own wording in the pane**, rather than being left to fall out of that general rule — it is what a reader hits who rarely opens the popover and opens Insights cold; **opening the Insights pane fires the existing `ScanController::request()`** (`scan.rs`), the same kick `popover.rs:note_shown` already fires on popover show and the six `commands.rs` sites fire on demand, so a cold open asks for a scan pass instead of waiting out a `TICK`. That is a further call site of the shipped on-demand trigger, not a new trigger class and **not queue reprioritisation**: no new mechanism and no priority ordering, and the closed question about the pane accelerating pending work stays answered no (**Risks & Open Questions → Closed**). Dismissals, history, notifications, and session-level cards are excluded; existing session UI behavior is unchanged. Likely touches: `commands.rs`, `dto.rs`, `lib.rs`, `apps/desktop/src/lib/ipc.ts`, `apps/desktop/src/lib/settingsPanes.ts`, `apps/desktop/src/views/SettingsView.tsx`, new `apps/desktop/src/views/settings/InsightsSession.ts` and Insights pane components. (Refs: FR-12, FR-14, FR-16; Invariants: no `useEffect`, no transcript content)
- [ ] **CH-013 — First-provider operational, privacy, and release hardening.** (Source Phase 11. Provisional Tier 3 — privacy review and release gate.) Acceptance: discovery, queue wait, source reading, parsing, metrics accumulation, evidence accumulation, persistence, report query, reduction, and IPC are measured separately, together with peak retained line buffer, accumulator memory, peak report memory, and impact while a representative Claude session is actively writing, and worker and provider-DB concurrency are tuned from those measurements; every persisted evidence field is re-reviewed for content sensitivity against the caps CH-009 already enforces, errors and logs are verified free of transcript content, and clear and delete behavior is verified; **the reader-facing local-data contract is updated in `docs/support.md` under "What antiburn stores"** to name derived session evidence and to state the retention reality — evidence is removed by session deletion and by clear-local-session-data, and is **not** removed automatically when a transcript is deleted from disk, **`apps/desktop/src/views/settings/PrivacyPane.tsx:PrivacyPane` is reviewed and updated** so the clear-index wording covers evidence, and the `store/schema.rs` data-policy comments describe the new table (`docs/privacy-policy.md` and `docs/usage-analytics.md` are not substitutes for the local-store contract and are touched only if a claim in them becomes wrong); the `docs/deviations.md` rule is applied with **no deviation as the default**, and the seam asserts explicitly that none was required, or records the one that was; **every entry in `docs/plans/local-insights-followups.md` carries a disposition, and every `file-issue` entry carries a real issue number**; worker concurrency and lease values are retuned from the measurements above; formatting, linting, all Rust tests, and release compilation and tests pass; no placeholder or fake evidence remains; the capability matrix, migration behavior, and downgrade or recovery expectations are verified; report output is checked on synthetic sources and on manually inspected real sessions. Likely touches: worker constants, `store/schema.rs` comments, `docs/support.md`, `apps/desktop/src/views/settings/PrivacyPane.tsx`, `docs/plans/local-insights-followups.md`, `docs/deviations.md` if applicable. (Refs: FR-17, FR-18; Invariants: performance and memory are product constraints)

> Ordering is a dependency hint that follows the source plan's phase order, not a seam sequence.

## Deferred work and followups

CH-001 creates `docs/plans/local-insights-followups.md`. It is a normal repository document, deliberately **not** under `.seams/` so `seams cleanup` cannot remove it, and it is never digest-bound, so any seam may append to it without reopening a review.

It is separate from this plan for one reason: an approved master plan is immutable, so a future-work section inside it could only be written before approval and would freeze at exactly the moment it needs to accrete.

Entry shape: what was found, which seam found it, why it was deferred, **kind** (`enhancement` or `deferred`), and **disposition** (`file-issue` | `fold-into-later-seam` | `drop-with-reason`). CH-013 requires every entry to carry a disposition and every `file-issue` entry to carry a real issue number.

CH-001 seeds it with these entries.

1. *Enhancement.* Join the cached account limits onto the quota-pressure section as a report-time join, with its own consent review.
2. *Enhancement.* Session-level hygiene badges from existing evidence — "reasoning overkill", "excessive cache rehydration", "bloated initial context" — through a second reducer at session granularity. This needs no new parsing, evidence, or schema: rule-neutral evidence plus report-time rules is exactly Locked Decision 2's shape.
3. *Enhancement.* Model additional Claude JSONL row types, argued from the collected unknown-`type` diagnostic after a release. Do not parse or retain unused records speculatively; reparse under a new parser revision instead.
4. *Enhancement.* Squash the migration ladder before v1. This is deliberately not done during this stack: an appended migration bumps `user_version` so a checkout self-upgrades on a branch switch, while a migration edited in place bumps nothing and silently leaves a developer database on the old shape. Check RC distribution first — tags exist through `antiburn-v0.1.0-rc.6`. Pairs with antiburn#76.
5. **Slow background discovery tick while the popover is hidden.** *Kind: enhancement.* Today scheduled scanning pauses entirely while the popover is hidden, so nothing notices a changed transcript until a pass is triggered. A slow background discovery tick (order of 15-30 minutes) would keep evidence current without user action. The gap is **discovery only**: CH-008's worker is not it, because that worker already drains a discovered backlog from the durable queue after the popover closes. **Three costs, priced at the GH-70 gate and deliberately not paid here:** it refreshes metrics nobody is looking at, because passes produce both projections; it reverses the deliberate existing decision to pause scanning while hidden, which `CONTRIBUTING.md`'s performance constraint supports; and it would require **Locked Decision 15 to be restated**, because a tick existing only to keep evidence current *is* an evidence schedule, which that decision currently denies. *Disposition: revisit with CH-013 measurement. Natural prerequisite if session-level badges (entry 2) proceed.*
6. *Deferred real work.* The second provider after Claude (source Phase 12).
7. *Deferred real work.* Reconcile the session index when a transcript is deleted from disk. This is privacy-relevant: `session_evidence` cascades on session delete, but the session row is never deleted when a file vanishes, so the cascade never fires. `Store::upsert_sessions` only inserts and updates; deletion happens only for gate-rejected transcripts (`scan.rs:311`), ignored paths (`repositories.rs:295`), and explicit user deletion (`commands.rs:1004`).
8. *Enhancement.* Phase 13 optimizations — report caching, relational evidence projections, additional read pooling — already out of scope here.

## Out of Scope (Non-Goals)

- **Source Phase 12 — the additional-provider loop.** No second provider is characterized, streamed, or enabled in the durable worker by this issue. Every non-Claude provider stays on the existing `analyze_sources_with` path (Locked Decision 12). Create a provider-specific follow-up issue after selecting and characterizing the next provider.
- **Source Phase 13 — measured optimizations.** No append-tail or byte-offset resume checkpoints, no report caching, no relational evidence child tables, no additional read connections or pooling, and no offloading of growing metrics collections to staging tables or a disk spool. Each needs a measurement first and then its own scoped change (Locked Decisions 8, 14). The pinned-prefix read of a still-growing source **is** in scope where the provider's append-only guarantee is evidenced (Locked Decision 8); it carries no state across generations, so it is not a checkpoint.
- Bounded exact rich-metrics memory. Retained metrics state may still grow with metric-bearing events (Locked Decision 14).
- **Looking up account-level limits.** Session evidence comes from session transcripts. Account state is a different subject, from a different subsystem, on a different schedule. Looking up limits is separate from session processing and out of scope for this issue. No Insights path calls a provider endpoint, and none reads the cached `LiveUsageSummary`. Joining the cached account snapshot onto the quota-pressure section is followup entry 1, with its own consent review.
- Building a second window for the Insights pane. CH-012 proves the pane is portable by mounting it from a second entry point in a test; shipping that window is not part of this issue.
- Contributor tooling. There is no contributor-tooling scope area here. Adopting the `aislop` gate is antiburn#89, and ratcheting its thresholds is antiburn#90.
- Session-level Insights cards, finding dismissals, finding history, and notifications. The first UI is one Settings pane (CH-012).
- Any change to the existing session UI behavior before CH-012.
- Changes to the pricing model, the remediation catalog content, or the deprecated-model replacement catalog beyond applying them at report time.
- Provider-specific branches inside the report reducer.
- Any telemetry or export of insights data.

## Risks & Open Questions

**Risks.**

| Risk | Impact | Handling |
|---|---|---|
| Streaming Claude normalization silently changes a displayed metric | User-visible regression in the existing session UI | Golden fixtures captured first in CH-001; field-by-field equivalence in CH-005; any difference stated and approved (Locked Decision 13) |
| A `SourceDescriptor` resolves a different session id than `scan.rs` does | Duplicate or orphaned rows against existing `SessionKey` values | CH-002 preserves the canonical resolution order and tests id stability across the migration (Current State correction 7) |
| Adding `visit_source` to `VendorAdapter` churns all six adapters | Broad diff, unrelated provider risk | Prefer a default method so non-Claude adapters are untouched (Current State correction 2) |
| Fence and generation guards look correct but race under a lease expiry | Two workers publish conflicting projections | Explicit tests in CH-007 for first-claim exclusivity, reclaim fencing, and late-fence rejection |
| A source that changes during every read never publishes a generation | That session contributes no evidence and its work repeats | Bounded backoff through `next_attempt_at` and fair claim ordering (CH-008), plus the pinned-prefix read where the provider's append-only guarantee is evidenced, which publishes a bounded generation instead of discarding the pass (Locked Decision 8, CH-004) |
| A rename between version capture and `open` substitutes a different file | The pass streams bytes from the wrong source and publishes them as the claimed generation | The opened handle is validated against the claimed generation before any record streams, that mismatch has its own row in **Architecture reference → Source-validity outcomes**, and CH-004 tests replacement-before-pinning against rename-after-pinning |
| A pinned prefix publishes evidence from a rewritten record | Stale or wrong evidence passes the recheck, because the head hash covers the head region only | Publication is conditional on an evidence-backed provider append-only guarantee, the detection envelope is stated rather than overclaimed, and the no-guarantee path is tested (Locked Decisions 8, 16; CH-002, CH-004) |
| Evidence grows with adversarial transcript cardinality | Database growth on an always-running utility | Explicit per-field caps with overflow to `Partial`, and cap tests, inside CH-009 rather than at measurement time |
| A detector reports clean from partial coverage | False reassurance — the worst possible failure for this feature | Three-state evidence (Locked Decision 3) plus the no-clean-from-incomplete tests in CH-011 |
| Non-ready or non-current in-window sessions read as assessed | The report implies a clean result it never proved | Two explicit populations (FR-12), population tests in CH-010, and separate presentation in CH-012 |
| An evidence schema revision forces a full reprocess of every session on update | Processing spike after an app update | Lazy revision-driven requeue through the bounded worker (FR-11) |

**Still open** — each is deferred to the scope area that first holds the evidence to settle it, and to no earlier point.

1. Exact first-provider fixture cohort and supported Claude schema versions — CH-001, sourced per that area's fixture-authoring rule.
2. The size of the hashed head region in the fingerprint — CH-002. That the region is small, fixed, and not `SOURCE_PREVIEW_BYTES` is settled (Locked Decision 16); only the number is open.
3. The numeric value of each per-field evidence cardinality cap and each retained-example bound — CH-009. That caps exist and overflow to `Partial` is settled; only the numbers are open.
4. Which partial evidence permits a finding and which prevents a clean status, per detector — CH-011.

**Closed, and written into this plan rather than left open.**

- *Maximum retained record bytes:* **8 MiB**, with the drain-mark-`Partial`-resume failure behavior (**Architecture reference → Bounded newline framing**).
- *Concurrency and lease:* one CPU permit, one source permit, one provider-DB permit, and a five-minute lease renewed on progress, retunable in CH-013 (**Architecture reference → Resource limits**).
- *Revision constants:* parser, analyzer, metrics, and evidence revisions all start at **1**. Bump one when the *meaning* of a stored value changes. Never bump for a refactor. Never bump for a catalog or pricing change — those are applied at report time precisely so they do not requeue (FR-11).
- *`docs/deviations.md`:* the rule applies, the default is no deviation, and CH-013 asserts explicitly that none was required.
- *Insights pane and scheduling:* opening the pane does **not** reprioritise worker scheduling. It does fire the existing on-demand `ScanController::request()` kick from a second call site (CH-012), which asks for a scan pass. That adds no mechanism and no priority ordering, and it does not change the trigger set FR-10 pins.
- *First-open experience:* a requirement, not an open number — coverage and pending progress are shown prominently, and an incomplete report never renders as an empty or clean state (CH-012). Only the numeric backlog threshold stays with CH-012.
- *Account-level quota input:* out of scope entirely (**Out of Scope**), with the report-time join recorded as followup entry 1.
- *Second provider after Claude:* out of scope, recorded as followup entry 6.

## Verification Strategy & Success Metrics

**Per seam** (see **Definition of Done**): `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test` from `crates/antiburn-local` and for the desktop crate; `aislop ci --changes --base <seam base>`, where the base is the parent seam's branch and not `main`; frontend slices add `pnpm --filter @antiburn/desktop lint`, `type-check`, and `test`.

**End-to-end bar for the issue.**

- Golden equivalence: every currently displayed `SessionMetrics` field matches its pre-change value on every characterization fixture (FR-4).
- Memory: a generated transcript larger than the retained-record budget processes to completion, with an asserted upper bound on the retained line buffer, an asserted cap on every cardinality-bearing evidence field, and a measured, documented figure for accumulator and peak report memory (FR-2, FR-3, FR-6).
- Concurrency: an integration test proves the report reads one pinned snapshot while a writer commits and does not block that writer for the scan's duration (FR-13).
- Durability: kill and restart during processing loses no pending work, publishes no stale projection, and preserves the last completed evidence payload (FR-7, FR-8, FR-9).
- Truthfulness: no detector reports clean without sufficient capability and coverage; the coverage denominator contains the assessed cohort, and every denominator row outside that cohort — known-start pending or processing, failed, unsupported, stale, or unknown-start — is counted with its reason and never assessed (FR-12, FR-14).
- Locality: no Insights code path calls a provider endpoint and none reads account-level limit state; the quota-pressure section reports transcript-attributable incidents and is not assessed when the transcript carries no quota evidence (FR-15).
- Responsiveness: an actively updated session's metrics refresh on exactly the shipped trigger set — launch, a `TICK` while the popover is visible, popover open, and on demand — with no quiet-period gate anywhere in the processing path and no trigger added or removed (FR-10, Locked Decision 15).
- Publication atomicity: an accepted read, including an accepted append-prefix, refreshes metrics and evidence together, and any source mutation classified as `SourceChanged` publishes neither (FR-5, FR-7, **Architecture reference → Source-validity outcomes**).
- Privacy: an inspection of every persisted evidence field, log line, error, and DTO finds no transcript content; clear and delete remove evidence; `docs/support.md` and `PrivacyPane` describe what is now stored (FR-18).
- Release: `cargo check --release` plus release build and tests are clean with no placeholder variant anywhere (FR-17).
- Product: the report renders on manually inspected real Claude sessions with defensible findings and honest not-assessed states (CH-013).

## Rollback / Safety

- **Schema.** Two appended migrations (generations and revisions, then `session_evidence`). Both are additive: new nullable or defaulted columns and one new table. No existing column is dropped or rewritten, and no shipped migration constant is edited (Locked Decision 10). A build without the new code still reads every existing row.
- **Downgrade.** An older application build ignores the new columns and the new table; it recomputes `session_analysis` through the legacy path. Rows written by the newer build stay readable. CH-013 verifies this expectation explicitly.
- **Data.** `session_evidence` is derived and disposable: deleting every row costs only reprocessing. Clear-local-session-data and session deletion remove it (FR-18).
- **Behavior.** Every scope area up to CH-011 is invisible to the reader; the only user-visible surfaces are CH-005 (existing metrics, which must not change) and CH-012 (the new pane). Backing out CH-012 removes the pane and leaves the durable evidence in place.
- **Deploy ordering.** The migration in CH-002 must ship before any writer of the new columns, and CH-007's table must ship before any evidence write. Both are enforced by the scope-area order.
- **Provider transcripts are never modified or deleted**, so no rollback can damage the reader's own data.

## Progress Log

Append-only as seams land (seam ID → what shipped → commit). Derived status lives in the records ledger; read it with `seams progress`.

_No seams landed yet._
