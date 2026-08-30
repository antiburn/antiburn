# Plan: session evidence harness parity

**Status:** Revised draft for review (rev 2, 2026-08-30)
**Scope:** The six session hygiene checks across Claude, Codex, OpenCode, and Pi
**Primary risk:** A false clean result caused by incomplete source evidence

## Revision history

- rev 1 (2026-08-28): original draft. Accumulator-centric; six seams.
- rev 2 (2026-08-30): rebased onto the turn-row store decision (see
  "Turn rows are the source of truth"). Phases 2, 3, and 5 rewritten as queries
  over rows. Policy items reopened. Overdepth environment override dropped.
  Standing-default fast-mode rule deferred.
- rev 3 (2026-08-30): Phase 4 gains a shared-parser cleanup item. Open
  decisions gain the generic fallback capability claim.
- rev 4 (2026-08-30): Phase 3 rewritten with the row-query design and its
  scope rules, split into 3a (query, no behaviour change) and 3b (switch).
- rev 5 (2026-08-30): decisions 10–12 settle the replacement registry (with
  verified dates), the per-family tier policies, and the crate boundary.
  Phase 5 ships as 5a–5d.

## Summary

Antiburn's high-level pipeline is right:

```text
raw transcript or provider database
    -> source adapter
    -> NormalizedRecord / NormalizedEvent
    -> CompositeSink
    -> SessionMetricsAccumulator + SessionEvidenceAccumulator
    -> persisted metrics and evidence
    -> session badges and the Insights report
```

The problems are in the middle: evidence is a bounded single-pass accumulator
per source file, source capability is a static per-vendor table, and the
canonical event has no provider-neutral thread or scope contract. Together they
produce false clean results and block child-aware checks.

This revision keeps the adapters, the durable worker, the evidence table, the
report reducer, and the six stable badge identifiers. It changes one thing
underneath them: parsed turns are persisted as SQLite rows, and evidence is a
query over those rows rather than streaming state.

The plan has two goals:

1. Stop current false-clean and false-finding outcomes immediately.
2. Support every check that each source can prove, and return `NotAssessed`
   when a source or an individual session cannot prove a verdict.

## Problem statement

All four gaps below are confirmed against `main` at `99b5e7e`.

### Static source capabilities decide too much

`apps/desktop/src-tauri/src/analysis.rs` (`capabilities_for_vendor`) selects a
static `SourceCapabilities` by vendor before the adapter reads anything, and
`insights/badges.rs` gates each check on those booleans plus group coverage. A
capability therefore describes a harness family, not the facts present in one
session. An optional signal that a harness supports but a given session never
recorded reads as clean.

### Metrics merge child streams, but evidence keeps only the parent

`stream_vendor_with_hooks` builds one accumulator pair per parent or child
source, merges metrics with `merge_metrics(parent, children)`, then publishes
`evidence: evidence.first().map(SessionEvidenceAccumulator::evidence)`. Child
evidence accumulators are constructed and discarded. This blocks or weakens
parent-versus-child model comparison, delegated fast-mode detection, and
per-thread cache analysis.

### The canonical event lacks a logical-thread contract

`NormalizedEvent` carries `uuid` / `parent_uuid` (Claude-only) and an
`EventSource` parent/subagent tag. It does not express provider-neutral thread
identity, per-thread turn order, or main-loop versus delegated scope. Adapters
already know some of these facts and use them only to filter records.

### Detector prerequisites conflate finding proof with clean proof

Some detectors refuse to evaluate when a conservative prerequisite is absent,
hiding a directly observed finding that needs fewer facts than a clean claim.

### Why not more accumulator state

Rev 1 fixed these by adding thread, scope, and per-signal coverage state to the
accumulators, each with an explicit cap that degrades to
`Partial(CapExceeded)`. That approach has three costs:

- Correctness is bought with caps. Every new collection is another place a
  large session becomes `Partial`.
- Every `ANALYZER_REVISION` or `EVIDENCE_SCHEMA_REVISION` bump reparses every
  transcript in the cohort. JSON parsing is ~92% of ingestion cost, and Claude
  Code prunes transcripts after roughly 30 days, so old sessions cannot be
  reprocessed at all.
- Every detector must be expressible as single-pass streaming state. Per-thread
  adjacent-pair cache accounting, "child discovered but unreadable", dominant
  parent model versus child model, and sidechain/child-file deduplication are
  all awkward in that form and trivial as queries.

Rows have one real cost: disk, O(n) in turns. A slim row is ~100–200 bytes; a
20,000-turn session is a few megabytes. Message content is larger and lives in
a separate blob table. Disk is the lowest-priority resource for this app.

## Immediate correctness defects

These land first, in one pull request, independent of the row store.

### Empty obsolete-model registry produces a clean result

`ReportCatalogs::default()` has `model_replacements: BTreeMap::new()`, and
`Report::new()` uses it. `old_model_usage.rs` finds no rule, returns
`Observation::NoFinding`, and `badges.rs` maps that to `Clean`. Every session
with complete evidence currently reports "Obsolete model: clean".

An empty registry must produce `NotAssessed(EvidenceContractIncomplete)` or an
equivalent structured reason. It must never produce clean.

### Fast-mode detection counts non-fast labels

`overuse_of_fast_mode.rs` sums `fast_modes.values().map(|t| t.delegated)`
across every tier key. A delegated `standard` turn is a fast-mode finding.

Only the normalized `fast` key may enter the numerator. Recognized `standard`
values belong only in the assessed denominator.

### Missing optional signals can read clean

`ModelEvidence` has `effort_tiers` and `fast_modes` counts but no
eligible-turn count. A session whose turns carry no effort value iterates an
empty map and falls through to `NoFinding`.

Until rows land, the accumulator must retain, per signal: eligible turns,
signal-bearing turns, and missing turns. A missing signal blocks clean. An
observed finding still wins when partial evidence proves presence.

### Session-list not-assessed copy uses finding wording

`sessionHygiene.ts` sets `name: definition.findingTitle` on not-assessed rows,
and `SessionStatusBar.tsx` renders `check.name` for those rows, so the tooltip
shows "Obsolete model detected" beside a not-assessed mark. The clean branch
has the same assignment; it is currently unrendered.

The presentation definition must carry a verdict-free `name` separately from
the clean, finding, and not-assessed titles.

## Design principles

### Turn rows are the source of truth

Decided 2026-08-27 and confirmed 2026-08-30. This supersedes the "out of scope"
ruling in `issue-245-bounded-session-metrics.md`, which rejected turn rows for
metrics because no consumer needed them. Evidence is that consumer.

Each parsed turn becomes one row in a `turn` table in the existing app
database. Message text goes in a separate `turn_content` table in the same
file, keyed by `turn` rowid, blob column last, so the hot table stays narrow.
Content is captured from day one; a disable or retention setting can come
later.

Per-session `SessionMetrics` and `SessionEvidence` JSON remain in
`session_analysis` and `session_evidence` exactly as now. They become
rebuildable caches derived from rows rather than the only representation. The
report reducer, badges, IPC, and UI keep reading them unchanged.

Consequences:

- A `PARSER_REVISION` bump reparses transcripts. An `ANALYZER_REVISION` or
  `EVIDENCE_SCHEMA_REVISION` bump requeries rows. The second is an order of
  magnitude cheaper and works after the transcript is pruned.
- Incremental ingestion of an append-only source becomes byte-offset plus
  fingerprint, using the existing `PinnedSource` / `SourceClaim` machinery.
- `MAX_*` caps on evidence collections stop being a correctness concern for
  anything derived from rows. Caps remain on the small summary structs that are
  serialized to JSON.

### Where the row logic lives

`local-insights-architecture.md` decision 11 keeps `antiburn-local`
storage-neutral. Rows-based evidence needs the crate to read rows. Proposed
resolution, to be confirmed in review:

- `antiburn-local` owns the `turn` and `turn_content` DDL as exported constants
  and the read/write functions over a borrowed `rusqlite::Connection`. It
  already depends on `rusqlite` for vendor database discovery.
- The desktop app's migration ladder (`store/schema.rs`, currently V14)
  appends a migration that applies that DDL. The crate never opens the app
  database itself and never learns app-level tables.
- The crate stays free of Tauri.

### Evolve the existing canonical seam

Do not build a parallel ingestion architecture. Extend `NormalizedEvent` with
the provider-neutral fields the row needs; do not add a second turn type.

The row shape:

```text
turn
  environment_key, agent, session_id   -- FK to session, ON DELETE CASCADE
  source_key           TEXT   -- parent transcript or child file identity
  thread_id            TEXT   -- provider-neutral logical thread
  turn_index           INTEGER -- stable order within thread
  scope                TEXT   -- 'main' | 'delegated'
  child_id             TEXT   -- when scope = 'delegated'
  role                 TEXT
  ts_ms                INTEGER NULL
  model                TEXT NULL
  effort               TEXT NULL
  speed                TEXT NULL
  input_tokens, cache_read_tokens, cache_write_tokens, output_tokens
  is_compaction_boundary INTEGER
  message_id           TEXT NULL
  uuid, parent_uuid    TEXT NULL   -- Claude only; kept for thread derivation

turn_content
  turn_rowid  INTEGER PRIMARY KEY REFERENCES turn(rowid)
  kind        TEXT   -- 'user' | 'assistant' | 'reasoning' | 'tool_input' | 'tool_result'
  content     BLOB
```

Tool calls and context sources keep their existing evidence representation
for now; a `turn_tool` table is a follow-up if a check needs it.

Compactions, context sources, and subagent relationships remain explicit
observations when they are not model turns.

### Keep source-specific logic at the adapter edge

Each adapter remains responsible for parsing raw shapes, filtering inherited or
replayed records, normalizing model, effort, speed, and token classes,
assigning thread and scope facts it can prove, declaring structural support,
and reporting per-session completeness at end of stream.

Shared code must not branch on harness names to interpret raw fields.
Detector policy can still differ by harness when the economic meaning differs
(cache-write tokens versus uncached input for repeated context).

### Structural support and observed coverage are separate

The adapter states what the source format can express. The rows state what
this session actually expressed. The final evidence state is their
intersection:

- `Unsupported`: the source cannot express the fact;
- `Partial`: the source can express it, but this session has gaps;
- `Complete`: the source expressed every fact needed to prove absence.

`SourceCapabilities` may remain as serialized diagnostics, but static booleans
must not be the sole eligibility authority. Add
`NotAssessedReason::SignalMissing` for a source-supported setting that this
session did not record. Keep it distinct from `CapabilityMissing`,
`IncompleteEvidence`, and `EvidenceContractIncomplete`; serialize it as
`signalMissing` with reader wording "Not assessed — this session did not
record the setting this check needs." Add it to the shared vocabulary in
`lib/presentation/sessionHygiene.ts`, not a parallel one.

### One logical session

Parent and child sources write rows under the same `(environment_key, agent,
session_id)` with distinct `source_key`, `thread_id`, and `scope`. Evidence is
computed over the union. Per-child metrics remain available for the cost split
and roster UI.

If discovery proves a child exists but the child cannot be read, the session
records a `child_unreadable` diagnostic and child-dependent evidence groups
become `Partial`. The child must not silently disappear and permit clean.

### Keep evidence rule-neutral

Persist observed facts and coverage, not hygiene conclusions. Thresholds and
reviewed registries remain report-time policy in versioned catalogs.

### Privacy with content stored

This is the first time transcript text is persisted. The rule changes from
"we hold none" to "it stays put":

- Content lives only in `turn_content`. Evidence JSON, metrics JSON, DTOs, the
  report reducer, logs, and analytics events never join to it. A test feeds a
  fixture with sentinel strings and asserts no serialized output contains them.
- `Store::clear_local_session_data` and `delete_session` delete `turn` and
  `turn_content` rows. The FK cascade is a backstop, not the mechanism.
- Content is excluded from any diagnostics export.
- Evidence and rows persist only bounded technical identifiers where a
  detector needs stable grouping: no private paths, no raw provider IDs beyond
  `session_id`, `message_id`, and Claude `uuid` links already stored.
- Sentry and third-party telemetry are already banned in `deny.toml`; keep
  that.

## Target support matrix

Legend:

- **Supported:** the current source provides the needed facts after
  canonicalization.
- **Conditional:** supported only when the individual session records the
  optional signal or complete relationship.
- **Unsupported:** no trustworthy current source contract exists.

| Check | Claude | Codex | OpenCode | Pi |
| --- | --- | --- | --- | --- |
| Session overdepth | Supported | Supported | Conditional on root/child tagging | Supported |
| Model overthinking | Conditional on explicit effort | Conditional on explicit effort | Conditional on mapped variant policy | Conditional on thinking-level evidence |
| Overpowered subagents | Supported | Conditional on child rollout attribution | Conditional on covered `parent_id` ancestry | Unsupported |
| Obsolete model | Conditional on reviewed registry | Conditional on reviewed registry | Conditional on reviewed registry | Conditional on reviewed registry |
| Fast-mode overuse | Conditional on speed coverage | Conditional on service-tier coverage | Unsupported | Unsupported |
| Excess context reprocessing | Supported | Conditional using uncached-input accounting | Conditional on session/thread identity | Conditional on cache-write support |

Expected maximum after this plan: Claude 6/6; Codex up to 6/6; OpenCode up to
5/6; Pi 4/6.

Unsupported checks still appear in the six-check UI with a settled
`NotAssessed` reason. The denominator is always six, per
`session-check-states.md`.

Cursor and Antigravity are outside this plan. They have adapters but no
`capabilities_for_vendor` entry and never stream evidence today; that stays
true until they have their own contracts and characterization suites.

## Detector semantics

### Session overdepth

Context occupancy from disjoint input classes on `scope = 'main'` turns only.

A finding requires one main-loop request above the cap. A clean result
requires complete main-loop ownership, request-context coverage, model
identity, and order where the adapter needs order to prove ownership. Do not
require Claude UUID links when an adapter has proven root ownership another
way.

Cap: 400,000 tokens, matching the context chart's heat kink. It lives in the
report-time catalog so a policy change does not reparse. **No environment
variable override** (rev 1 proposed `ANTIBURN_SESSION_OVERDEPTH_CAP_TOKENS`
with a debug-launch integration test; dropped as overengineering for a desktop
app). Unit tests inject catalogs directly.

### Model overthinking

Only explicit effort values count. Normalize case and whitespace before policy
lookup. Coverage: `count(*) where role='assistant'` versus
`count(*) where effort is not null`. Unknown values do not trigger but block
clean until policy classifies them. Use a reviewed per-family tier policy; do
not assume equal strings mean the same across harnesses.

### Overpowered subagents

Dominant `scope='main'` model tier versus each observed `scope='delegated'`
model tier in the same session. A finding requires a proven delegated turn or
child relationship, an observed child model, an observed main-loop model, and
reviewed premium-tier classification for both. Clean additionally requires
complete child enumeration (no `child_unreadable`) and child-model attribution.

### Obsolete model

Non-empty reviewed replacement registry with stable source IDs and aliases,
replacement ID, effective date, rationale, and registry revision. Normalize
observed model keys before matching. Usage before the effective date is not a
finding. Empty registry is not assessed.

### Fast-mode overuse

Recognize exact normalized `fast` and `standard`. Missing or unknown speed is
neither, enters neither numerator nor denominator, and blocks clean when
coverage is required.

Ship the delegated pattern: any recognized fast turn in `scope='delegated'`
work. The per-session badge and the 30-day report both use it. Claude findings
use dollar impact; Codex findings use plan-quota impact.

**Deferred:** rev 1's standing-default pattern (fast ≥ 30% of recognized turns
across ≥ 3 fast-containing and ≥ 5 eligible sessions). No real transcript has
yet shown `speed: fast`; the thresholds are unvalidated. Revisit after the
delegated pattern has run on real data.

For Codex, parse a trustworthy thread-settings service tier and normalize
reviewed values (priority/default) into fast/standard.

### Excess context reprocessing

Stable ID `excessCacheRehydration`; shared check name "Excess context
reprocessing"; detail copy "Reduce repeated cache writes" for cache-write
accounting and "Reduce full-price context re-reads" for uncached-input
accounting.

Over rows: `scope='main'` turns grouped by `thread_id`, ordered by
`turn_index`, timestamps as a secondary check; pairs with missing or
overlapping order are skipped. For adjacent turns, repeated paid context beyond
positive context growth, using cache-write accounting where the provider bills
cache creation and uncached-input accounting where it bills full-price
repeated input. Attribute causes (compaction, model switch, idle gap beyond the
reviewed cache TTL, other) as explanation, not as the calculation.

Never compare a parent turn with a child turn or two unrelated child threads.
Grouping by `thread_id` makes this structural.

## Implementation phases

### Phase 0: Freeze baselines and decisions

**Risk:** Low

1. Characterization tests for the current capability and badge status matrix.
2. Failing tests for the four immediate defects.
3. Lock the 400,000-token cap and the delegated fast-mode pattern.
4. Human review of the model replacement registry (see below).

**Acceptance:** Tests reproduce every current false-clean or false-finding path.

### Phase 1: Honesty fixes

**Risk:** Low. Independent of rows.

Files: `insights/detectors/{mod,old_model_usage,model_overthinking,overuse_of_fast_mode}.rs`,
`insights/badges.rs`, `analysis/evidence.rs` (eligible/present/missing
counts), `apps/desktop/src/lib/presentation/sessionHygiene.ts`.

- Compiled replacement registry; empty registry returns not assessed.
- Fast detection counts only the `fast` key.
- Per-signal coverage counts in `ModelEvidence`; missing coverage blocks clean
  with `signalMissing`.
- Verdict-free `name` on all presentation branches.
- Bump catalog revision.

**Acceptance:** No empty registry or absent signal produces clean. A delegated
standard turn cannot produce a fast finding.

### Phase 2: Turn-row store

**Risk:** Medium

Files: `crates/antiburn-local/src/analysis/{interface,model}.rs`, a new
`crates/antiburn-local/src/analysis/rows.rs`, `apps/desktop/src-tauri/src/store/{schema,mod}.rs`,
`apps/desktop/src-tauri/src/analysis.rs`.

- Add `thread_id`, `turn_index`, `scope`, `child_id` to `NormalizedEvent`
  (`skip_serializing_if` to protect the 15 goldens).
- Add `turn` and `turn_content` DDL (crate-owned) and migration V15.
- Add a `RowSink` to the `CompositeSink` that writes rows in one transaction
  per source file, inside the existing evidence-claim lease.
- Extend `clear_local_session_data` and `delete_session`.
- Parse once, write rows and existing accumulators in the same pass. Evidence
  JSON output is unchanged in this phase.

**Acceptance:** Claude, Codex, OpenCode, and Pi characterization fixtures
produce identical metrics and evidence JSON with rows enabled. Row counts and
content sentinels are asserted per fixture. Memory baselines hold.

### Phase 3: Logical-session evidence from rows

**Risk:** Medium. Two pull requests.

Files: `crates/antiburn-local/src/analysis/{evidence,evidence_sink,rows}.rs`, a new
`crates/antiburn-local/src/analysis/evidence_query.rs`,
`apps/desktop/src-tauri/src/{analysis,insights_worker}.rs`,
`apps/desktop/src-tauri/src/store/{mod,schema}.rs`.

**Measured before design (2026-08-30, 76 local Claude sessions):** no parent
transcript contains an inline `isSidechain` record; 61 sessions have child
files; every child-file record carries `isSidechain: true`. Child rows
therefore already land as `scope='delegated'`. The inline shape is an older
Claude Code layout and stays a Phase 4 reconciliation item.

**3a — the query (no evidence behaviour change).**

- `turn` gains `compaction_trigger`, `compaction_pre_tokens`,
  `compaction_post_tokens` (V16, `ALTER TABLE`; the crate exports
  `TURN_MIGRATIONS`). Compactions become row facts.
- `TurnRowWriter` becomes `TurnRowStore` with a read side:
  `query_turn_facts()` returns the facts below for the store's own session
  key and claim fence. The sink owns an `Arc<dyn TurnRowStore>`.
- Child inputs write rows with a forced `scope='delegated'`; the adapter's
  `EventSource` flag is no longer the only source of scope.
- `query_turn_facts` in `evidence_query.rs` runs SQL aggregates plus one
  ordered streaming scan. It never loads the session's rows into memory.
- `MemoryTurnRowStore` gives tests and tools an in-memory row store.
- A parity test streams every characterization fixture through both the
  accumulator and the query and compares the shared groups. Known
  divergences are recorded, not hidden.

**Scope rules for the facts.** These are the semantic decisions of this
phase:

| Fact | Rows used | Reason |
|---|---|---|
| eligibility, context depth, time range, token sums, models, effort and speed tiers, signal coverage | all scopes | The logical session is the union of parent and children. |
| delegated turns and models | `scope='delegated'` | Child facts reach the parent's hygiene checks. |
| model transitions, idle gaps, manual compactions, compaction boundaries | `scope='main'`, computed per `thread_id` | A child model switch or a child idle gap never reads as parent reprocessing. Two child threads never form an adjacent pair. |
| duplicate turn identities | all scopes | The same `uuid` under two `source_key`s marks parent-child overlap. It degrades the subagent and model groups instead of double counting. |

**3b — the switch.**

- `SessionEvidenceAccumulator::evidence` takes `&TurnFacts`. The accumulator
  keeps only what rows cannot express: tools, context sources, diagnostics
  and coverage, ordering, subagent spawn observations, and thread-link
  verification (a parent link can point at an eventless record, which has no
  row).
- `stream_vendor_with_hooks` queries the facts once after every input has
  streamed, then builds one `SessionEvidence`. Without a row store the pass
  publishes no evidence. `evidence.first()` is gone.
- A discovered child that cannot be read, or that streams with record loss,
  degrades every child-dependent group to `Partial`. `ParseDiagnostics`
  records the child counts.
- Bump `EVIDENCE_SCHEMA_REVISION` 4 → 5 and `ANALYZER_REVISION` 7 → 8.

**Deferred from this phase:** tool names and context sources from child
streams stay parent-only; a child's own idle gaps and compactions are not
folded into the parent's cache group.

**Acceptance:** A child-only model or speed signal reaches parent hygiene. A
child model switch never creates parent reprocessing. An unreadable known
child prevents false clean. Two child threads never form an adjacent pair.
The parity test explains every difference between the query and the old
accumulator.

### Phase 4: Adapter enrichment

**Risk:** Medium. One pull request per source.

**Claude:** derive `thread_id` from the existing UUID chain; reconcile inline
sidechains with separately discovered child files without double counting
(characterize identity overlap first, enable one authoritative path until
equivalence is proven); report effort and speed per eligible turn.

**Codex:** preserve root versus child rollout ownership; emit `delegated`
scope for discovered child rollouts; parse reviewed service-tier records to
fast/standard; report uncached-input support.

**OpenCode:** include source session ID and `parent_id` in the recursive
cluster query; stop flattening descendants anonymously; treat covered
`session.parent_id` as delegated ancestry and distinguish it from forks (null
`parent_id`, copied prefix, generated title) with hand-authored fixtures for
each shape; preserve per-message variant, model, token classes, and compaction
boundaries.

**Pi:** preserve root/fork filtering; preserve thinking-level changes as
explicit effort; retain per-session cache-write availability per selected API;
never infer subagents or speed.

**Shared parser cleanup:** `vendors/jsonl.rs` is not a vendor. It is the
shared record parser that every adapter imports, and it sniffs the Anthropic,
OpenAI, and Pi shapes from the keys present. Move it out of `vendors/` (for
example `analysis/records.rs`). Move the Claude-only skill-marker logic in
`parse_jsonl` into `claude.rs`. Make `parse_record` take an explicit shape
instead of guessing. `vendors/generic_jsonl.rs` stays as the fallback adapter.

`PARSER_REVISION` 5 → 6 once, when the first adapter emits the new fields.

**Acceptance:** Each adapter's characterization suite freezes its structural
support, per-session partial states, and six-check status matrix.

### Phase 5: Detector fact requirements

**Risk:** Medium

Files: `insights/{report,badges}.rs`, the six detector modules.

- Evaluate fact coverage rather than vendor capability masks.
- Separate finding requirements from clean requirements.
- Drop the universal `THREAD_IDENTITY` gate from overdepth when main-loop
  ownership is proven otherwise.
- Dominant-parent versus delegated-child model evaluation.
- Reviewed effort and speed policies; normalized replacement matching.
- Per-thread repeated-context accounting replacing event-presence churn.

**Acceptance:** Every target matrix cell has a fixture showing finding, clean,
and not-assessed behaviour where those states are valid.

### Phase 6: Rollout and hardening

**Risk:** Low to medium

- Verify `reconcile_evidence_revisions` requeues stale rows and recomputes
  from rows without reparsing where `PARSER_REVISION` is unchanged.
- Verify the UI never reads stale evidence as clean.
- Memory and disk baselines with parent plus many children and with content
  enabled.
- Privacy tests per "Privacy with content stored".
- Verify active-source and source-changed behaviour still publishes metrics
  and evidence atomically.

**Acceptance:** Upgrade tests prove old evidence is reprocessed without a
manual migration or a false clean transition.

## Test strategy

### Characterization

Extend `crates/antiburn-local/tests/{claude,codex,opencode,pi}_characterization.rs`.
Each source needs fixtures for: complete root session; missing optional
effort/speed signal; unknown signal value; child-only model usage where
supported; unreadable or incomplete child; compaction and model switching;
malformed and unknown records; active incomplete tail; obsolete model before
and after effective date.

### Row parity

Rows enabled versus disabled produce identical metrics and evidence JSON on
every fixture: token totals and classes, context occupancy, model runs, tools
and skills, compaction counts, cost split, parent and child roster.

### Logical-session attribution

A child premium model reaches the parent's evidence; a child fast turn reaches
the parent's evidence; a child context window never increases parent
overdepth; a child model switch never creates parent reprocessing; two child
threads never form an adjacent pair; a missing known child prevents clean.

### Detector honesty

For every check: observed findings survive partial evidence when presence is
conclusive; incomplete absence never becomes clean; unsupported facts return
`capabilityMissing`; source-supported but missing session facts return
`signalMissing`; an empty registry returns not assessed; unknown policy values
return not assessed.

### Privacy

Fixture sentinel strings appear in `turn_content` and nowhere else.
`delete_session` and `clear_local_session_data` remove them.

### UI and IPC

Preserve the six badge IDs and order: `sessionOverdepth`, `modelOverthinking`,
`overpoweredSubagents`, `obsoleteModel`, `fastModeOveruse`,
`excessCacheRehydration`. Verify all six appear in the session-row aggregate
and tooltip, session detail, and the Insights report. Verify unsupported
sources settle as not assessed rather than permanent pending.

### Validation commands

Engine:

```bash
cd crates/antiburn-local
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Desktop frontend:

```bash
pnpm --filter @antiburn/desktop lint
pnpm --filter @antiburn/desktop type-check
pnpm --filter @antiburn/desktop test
pnpm --filter @antiburn/desktop build
```

Desktop shell:

```bash
cd apps/desktop/src-tauri
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Repository gates:

```bash
pnpm run slop:all
pnpm run secrets
```

## Risks and mitigations

### Parent-child double counting

Claude exposes inline sidechain turns and separate child files.
**Mitigation:** characterize identity overlap first; one authoritative child
path per source until equivalence is proven; `source_key` on rows makes the
overlap visible.

### OpenCode descendant meaning

Covered schema uses `session.parent_id` for subagents; forks are new roots with
null `parent_id`. **Mitigation:** synthetic fixtures for each shape; enable the
subagent check only for the covered relationship; unknown shapes stay not
assessed.

### Optional signals produce false clean

**Mitigation:** per-signal eligible/present/missing counts; never interpret
`None` as a negative observation.

### Disk growth

Content is O(n). **Mitigation:** measure on representative corpora in Phase 6;
a retention or disable setting is a follow-up, with the schema already
supporting it (drop `turn_content` rows without touching `turn`).

### Storage-neutral crate boundary

Rows put a schema in `antiburn-local`. **Mitigation:** the crate owns only its
own tables' DDL and functions over a borrowed connection; it never opens the
app database or references app tables. Confirm in review.

### Policy changes alter old verdicts

**Mitigation:** policy in versioned report-time catalogs; catalog revision
increments independently of parser revision; with rows, a catalog change never
reparses.

## Non-goals

- Adding a seventh session hygiene check.
- Redesigning the session row or Insights pane.
- Enabling Cursor or Antigravity evidence.
- Inferring unsupported signals from prompts, model names, pricing, or generic
  ancestry.
- Replacing the durable worker, report reducer, or evidence table.
- Combining native and WSL report scopes.
- A `turn_tool` table or content retention settings (follow-ups).

## Delivery

One production-safe pull request per seam. No pull request may leave existing
evidence unreadable, stop the durable worker, or permit a false clean result.
Stack onto the prerequisite branch when needed; rebase after merge without
combining seams.

Seams:

1. honesty fixes;
2. turn-row store (rows written, evidence unchanged);
3. logical-session evidence from rows;
4. one adapter enrichment per source (four pull requests);
5. detector fact requirements and reprocessing accounting;
6. rollout and privacy verification.

Temporary loss of a clean badge is acceptable. A false clean result is not.

## Decisions

Settled:

1. Turn rows are the source of truth; content is stored from day one.
2. Overdepth cap is 400,000 tokens, catalog-only, no environment override.
3. Fast-mode ships the delegated pattern only; standing-default is deferred.
4. `signalMissing` is a structured not-assessed reason.
5. `excessCacheRehydration` keeps its ID; shared name "Excess context
   reprocessing"; accounting-specific detail copy.
6. Scope is all four harnesses.
7. The generic fallback adapter reports the minimum capability set, so the
   badges read not-assessed for an unknown vendor (decided 2026-08-31; ships
   with the Codex adapter change in Phase 4).
8. OpenCode fork semantics, verified 2026-08-31 against an anonymised real
   OpenCode 1.1.25 capture (cadence `session_forks/opencode` fixtures): a
   subagent has `parent_id`; a fork is a new root with null `parent_id`, a
   ` (fork #N)` title, and a copied prefix. A fork title without a copied
   prefix is not a fork. Copied messages keep the parent's `time_created`,
   which is older than the fork session itself; that is a structural
   fork-point signal that does not need the title.
9. Split the `thread_identity` capability (decided 2026-08-30). Today two
   consumers read one flag as two claims: `report.rs` reads "each row knows
   its thread" (the overdepth prerequisite), and `evidence_sink.rs` reads
   "each counted row carries a record identity" (`SUM(uuid IS NULL)` sets
   `thread_identity_missing`, which degrades the `cache` group and
   `previous_turn` to `AttributionIncomplete`). Phase 5 keeps
   `thread_identity` for thread membership and adds `record_identity` for
   the per-record linkage `previous_turn` verifies. Cache churn then
   requires `record_identity`; overdepth requires `thread_identity`. Codex
   (one rollout is one thread, no record ids) gets `thread_identity` only.

10. Model replacement registry (verified 2026-08-30 against vendor pages,
    ships in seam 5c as a compiled table with `REGISTRY_REVISION`): Claude
    Opus 4.5–4.8 → `claude-opus-5`, available 2026-07-24
    (anthropic.com/news/claude-opus-5); Claude Sonnet 4.5–4.6 →
    `claude-sonnet-5`, available 2026-06-30 (anthropic.com/news/claude-sonnet-5);
    GPT-5.4, GPT-5.5, GPT-5.5-fast → `gpt-5.6-sol` and GPT-5.4-mini →
    `gpt-5.6-luna`, available 2026-07-09 (openai.com/index/gpt-5-6). The
    mini → Luna size mapping is a judgment call; no Haiku entry exists because
    no Haiku 5 has been announced. Aliases are matched after
    `normalize_model_key` and lowercasing. Usage before the effective date is
    not a finding.
11. Tier policies (seam 5c) are keyed by model family from the normalized
    model-key prefix (`claude-` → Claude, `gpt-`/`o*` → OpenAI, else
    Unknown), not by harness, because OpenCode and Pi run any vendor's
    models. Effort: above cap = {xhigh, max, ultra, ultrathink} in both
    families (Cadence's fleet-derived policy, 2026-08-13); recognized floor
    Claude = {low, medium, high}, OpenAI = {none, minimal, low, medium, high}. Speed: exactly {fast, standard} after
    normalization. Premium: Claude = `claude-opus-*` and `claude-fable-*`;
    OpenAI = `gpt-5.6-sol*` (OpenAI states Sol, Terra, Luna are capability
    tiers with Sol at the top). A tier or model outside a reviewed policy
    never reads clean. Overdepth cap moves to 400,000 in the catalog
    (decision 2) in the same seam.
12. Crate boundary for rows: `antiburn-local` stays storage-neutral at the
    detector layer. Detectors and fact requirements read `SessionEvidence`
    only; row queries live behind `analysis/evidence_query.rs` and the
    `TurnRowStore` trait, never in `insights/`. Seam 5d's repeated-context
    accounting is a query in `evidence_query.rs` whose result is persisted as
    evidence, so a catalog change never reparses.

Open: none. Every remaining item is an implementation seam.
