# Issue #244 — Promote Pi to a dedicated bounded full-evidence provider

Implementation plan. Authored against detached head
`679a5c9662e5a12e86319deb0a6a3c3be896b667` (`refs/heads/feat/227-codex-provider`,
PR #243).

Status: **implementation and final verification complete on the stacked draft
branch**. The branch rebases the four #243 commits onto merged #242, and #243
remains the merge dependency. Issue #229 remains open, but this change adopts
its recommended strict structural policy for Pi and leaves the broader shared
rollout to #229. Privacy-safe aggregate structural measurements resolved G1–G3.
Both application and engine changelogs include the promotion.

- Issue: <https://github.com/antiburn/antiburn/issues/244>
- Template PR: <https://github.com/antiburn/antiburn/pull/243> (Codex provider, issue #227)
- Policy dependency: <https://github.com/antiburn/antiburn/issues/229> (unrecognized record types)
- Coverage semantics dependency: <https://github.com/antiburn/antiburn/pull/242> (session hygiene badges, issue #221)

---

## 0. How to read this plan

The plan separates three kinds of statement, and every implementer must keep
them separate:

| Marker | Meaning |
| --- | --- |
| **VERIFIED** | Read directly from the worktree at `679a5c9`. File and line cited. Safe to build on. |
| **DECISION** | A choice this plan makes, with the reasoning and the evidence behind it. Implement as written unless a gate overturns it. |
| **GATE** | Not decided. Blocks a specific slice. Needs a maintainer answer or a fresh measurement before that slice starts. |

Anything not marked is background.

A statement that a research pass produced no coverage is **not** evidence of
absence. Where two local observations disagree, this plan records the
disagreement as a GATE rather than picking a winner.

**Corpus counts in this document are bucketed to an order of magnitude.** §4.1
prohibits recording a size, count, or hash that could fingerprint a private
local source; that prohibition binds this document too. See §17-R18.

---

## 1. Baseline and dependencies

### 1.1 Verified baseline

**VERIFIED** — repository state at the time of writing:

```
HEAD                     679a5c9  fix(codex): preserve fork usage ownership
HEAD is                  detached at refs/heads/feat/227-codex-provider
origin/main              8156a6e  Merge pull request #242 ... feat/221-session-hygiene-badges
git merge-base HEAD main e5fcf8a
```

**VERIFIED** — `gh pr view 243` returns `{"state":"OPEN","mergedAt":null}`.
PR #243 is **not merged**.

**VERIFIED** — `gh pr view 242` returns
`{"state":"MERGED","mergedAt":"2026-08-27T10:43:30Z"}`. #242 is a merged **pull
request**, not an issue. Its semantics are on `origin/main`.

**VERIFIED** — `crates/antiburn-local/src/insights/` on this worktree contains
`detectors/`, `mod.rs`, `quota.rs`, `report.rs`, `status.rs`. It does **not**
contain `badges.rs`. `git cat-file -e origin/main:crates/antiburn-local/src/insights/badges.rs`
succeeds. **#242 is absent from this baseline** because the branch forks at
`e5fcf8a`, before #242 landed.

**VERIFIED** — `gh issue view 229` returns `{"state":"OPEN"}`. Its comment
thread contains a fact-check, a final proposal, and a cross-reference to #242's
badge/report divergence. It contains **no recorded decision**.

### 1.2 Consequence for sequencing

Three of #244's own preconditions are unmet today:

1. #244 "Dependencies and sequencing" says *implement after #243 merges*. #243 is open.
2. #244 says *resolve or explicitly adopt the current outcome of #229*. #229 is undecided.
3. #244 says *apply the badge/report coverage behavior established by #242*. #242 is not in this branch's history at all, so its behaviour cannot be applied from here.

**DECISION D0 — the true implementation baseline is the merge commit of #243
into `main`, not `679a5c9`.** That commit contains #242 (already on `main`) and
#243 (the adapter template). Every anchor in §7 of this plan is derived from
`679a5c9` and **must be re-derived** against the merge commit before the first
line of Pi code is written, because review can still move Codex's adapter
shape, its capability set, its cohort array, and its migration index.

**GATE G0 — record the real baseline before slice 4.**

```sh
gh pr view 243 --json state,mergedAt,mergeCommit
git -C <worktree> rev-parse origin/main
```

Both values go into §16's decision log.

**G0 scope (revised).** Slices 1–3 add only new files (`vendors/pi.rs`,
`tests/pi_characterization.rs`, fixtures) plus a `pub mod` line in
`vendors/mod.rs` and a `pub use` line in `analysis/mod.rs`. They can begin
against `679a5c9` and be rebased. **Slice 4 onward requires `state == MERGED`**,
because slice 4 edits `evidence.rs` (whose `SourceCapabilities` shape #243 review
can still move) and slices 5–8 edit files #243 created or changed.

**Slices 2–3 are rework-exposed even though they add no shared file.**
*(Stated plainly in revision 2 — §17-R39.)* **VERIFIED**: at the merge base
`e5fcf8a`, `SessionSummary` has **six** fields; `started_at_ms` and
`coverage_gaps` are #243's additions. Slice 2's "all eight fields" requirement
and D7's `visit_claimed` / `AcceptedPrefix` shape are therefore written against
#243's **API surface**, not merely against files it touched. If review moves
`SessionSummary`, `VisitOutcome`, or `AppendOnlyGuarantee`, slices 2–3 need
rework, not just a rebase. Budget for that, or hold slice 2 until G0 clears.

Re-derive every line anchor in §7 at the G0 boundary regardless.

### 1.3 Dependency table

| Dependency | State at `679a5c9` | Blocks | Fallback if unresolved |
| --- | --- | --- | --- |
| PR #243 (Codex provider) | OPEN | Slices 4–8 (slices 1–3 may proceed and rebase) | Hard block on slice 4. |
| Issue #229 (unknown-type policy) | OPEN | Broader shared rollout | Pi adopts its recommended strict structural policy now. |
| PR #242 (hygiene badges) | MERGED on `main`, absent here | Final badge coverage after rebase | Do not copy its reducer into this stacking branch. |
| Local Pi format characterization | Aggregate structural measurements complete (§4.4) | None | G1–G3 resolved. |

---

## 2. Verified current state

Every claim in this section was read from the worktree.

### 2.1 Pi discovery already exists

`crates/antiburn-local/src/discovery/agents/pi.rs`

- `PiExplorer` scans `~/.pi/agent/sessions/**` for `.jsonl` files (`surface_paths`, lines 68–75).
- `cwd` is read from the first `{"type":"session","cwd":...}` row (module doc, lines 8–12).
- `recover_session_id_from_path` takes the UUID after the last `_` in the file stem (lines 84–93).
- Pi is a CLI-only source; `owns_path` matches `/.pi/agent/` (lines 57–59).
- **VERIFIED stale comment**: lines 76–78 state *"Pi transcripts carry no in-content session ID"*. The `session` header does carry an `id` field. See DECISION D8.

`crates/antiburn-local/src/model/agent.rs:75` — `AgentKind::Pi => "pi"`, inside
`AgentKind::slug()`. The vendor label is computed separately, at
`apps/desktop/src-tauri/src/agents.rs:23-30`, whose `other => other.slug()` arm
makes `vendor_label(AgentKind::Pi) == "pi"`. The two names coincide, so
`vendor_label` needs no new arm — and `analysis.rs:1113`'s
`agent: vendor_label(agent)` will therefore reach `capabilities_for_vendor("pi")`
(`analysis.rs:521`) once that arm exists. *(Anchor corrected in revision 2 — the
previous text implied line 75 was `vendor_label` itself. §17-R38.)*

### 2.2 Pi analysis is generic

`crates/antiburn-local/src/analysis/vendors/mod.rs:26-34` — `adapter_for` has
arms for `claude`, `codex`, `cursor`, `opencode`, `antigravity`. Everything
else, including `pi`, falls to `GENERIC`.

`vendors/mod.rs:47-49` — `has_dedicated_adapter` is derived from `adapter_for`,
exactly as #244 requires. No second list exists.

`vendors/mod.rs:78-91` — two registry tests:

```rust
fn dedicated_adapters_are_recognized_case_insensitively()  // line 78
    for agent in ["claude", "codex", "cursor", "opencode", "antigravity"]
fn generic_fallback_vendors_have_no_dedicated_adapter()     // line 85
    for agent in ["copilot", "cline", "windsurf", "pi", "", "totally-unknown"]
```

`"pi"` must move from the second list to the first.

**VERIFIED** — `generic_jsonl.rs:23-34`, the adapter Pi uses today, implements
only `normalize` and sets `cache_write_tokens_available: true`,
`context_window: None`, `model: None`. It has no `visit` override, so Pi
currently takes the `VendorAdapter` default streaming path.

### 2.3 Shared JSONL parsing already handles some Pi shapes

`vendors/jsonl.rs:652-689` — `parse_usage`. The `(None, None)` arm reads
`input`, and the getters read `output`, `cacheRead`, `cacheWrite`. Line 668
carries the comment *"Pi's disjoint camelCase shape; its buckets never overlap,
so no subtraction."*

**VERIFIED and load-bearing**: `parse_usage` reads exactly four keys and
constructs a four-field `Usage`. It has no path that reads `reasoning`,
`cacheWrite1h`, `totalTokens`, or `cost`. Any Pi adapter that adds those keys
introduces double counting. See DECISION D5.

**VERIFIED** — `analysis/model.rs:33-37`:

```rust
pub fn context_tokens(&self) -> u64 {
    self.input_tokens
        .saturating_add(self.cache_read_tokens)
        .saturating_add(self.cache_creation_tokens)
}
```

Request occupancy is **input + cacheRead + cacheWrite**, not `input + cacheWrite`.
`metrics_sink.rs:105` takes its running maximum into `peak_context_tokens`, and
only for `EventSource::Parent`.

### 2.4 Two Pi metrics tests are already pinned

`crates/antiburn-local/src/analysis/tests.rs`

- Line 333 `pi_jsonl_tool_calls_classify_into_tool_categories` — a five-line Pi fixture using `type:"message"`, roles `assistant` and `toolResult`, blocks `text`/`toolCall`/`thinking`, `toolCallId`, `toolName`, `isError`. Asserts tool categories `Read`, `Edit`, `Test`.
- Line 416 `pi_sessions_report_real_local_usage_downstream` pins the existing batch usage, model, cost, and context results. Repair fixtures do not copy its captured values. The deliberately loose `tokens_in > 0` assertion remains unchanged.

**VERIFIED** — request occupancy uses `input + cacheRead + cacheWrite`, not a
two-term approximation (§2.3). New Pi fixtures use independently chosen small
synthetic values.

**VERIFIED** — both fixtures are **bare `type:"message"` rows**. Neither has a
`session` header, an `id`, or a `parentId`. Both call `normalize_source`, i.e.
the **batch** path.

### 2.5 Capabilities

`crates/antiburn-local/src/analysis/evidence.rs:281-301` — `SourceCapabilities`
has 17 `bool` fields. Constructors: `claude()` at `303-324`, `codex()` at
`326-346`. There is no `pi()`. Adding one is new public API on the crate.

**VERIFIED and load-bearing** — `analysis/evidence_sink.rs:571-572`:

```rust
pub fn observe_summary(&mut self, summary: &SessionSummary) {
    self.capabilities.cache_write_tokens = summary.cache_write_tokens_available;
```

The declared `cache_write_tokens` is **overwritten per session** by the
adapter's `SessionSummary`. The constructor value is only a default. See
DECISION D12.

`crates/antiburn-local/src/insights/report.rs:140-190` — `requirements(detector)`
maps each of nine `DetectorId` values to required capability clauses and
evidence groups. Reproduced verbatim in §5.2.

`report.rs:94-116` — `EvidenceGroup::state()` reads the matching
`EvidenceValue` field on `SessionEvidence` and maps `Unsupported`/`Partial`/
`Complete`. `Self::Context => state(&evidence.context)`.

`evidence_sink.rs:672-676` — `context: self.supported_value(context,
self.capabilities.request_context_tokens, self.context_cap_exceeded)`.
`supported_value` is at `823-841`. **`EvidenceGroup::Context` is gated on
`request_context_tokens` alone.** `initial_context` never reaches an evidence
group; it flows to metrics (`metrics_sink.rs:151`). See §17-R6.

### 2.6 Desktop wiring gaps

| File:line | Current | Needs Pi |
| --- | --- | --- |
| `apps/desktop/src-tauri/src/analysis.rs:521-527` | `capabilities_for_vendor`: `"claude"`, `"codex"`, `_ => None` | yes — **this is the streaming gate** |
| `apps/desktop/src-tauri/src/analysis.rs:540-545` | `None` capabilities ⇒ `StreamOutcome::ParentUnsupported` | consequence of above |
| `apps/desktop/src-tauri/src/agents.rs:47-49` | `evidence_cohort() -> [&'static str; 2]` | yes, becomes `; 3` |
| `apps/desktop/src-tauri/src/agents.rs:60-66` | `dedicated` array in `every_kind_resolves_to_a_vendor_label_the_registry_recognizes` | yes |
| `apps/desktop/src-tauri/src/agents.rs:87-98` | `generic_fallback_agents_report_no_dedicated_adapter` asserts `!supports_analysis(AgentKind::Pi)` | **yes — fails at the registry flip** |
| `apps/desktop/src-tauri/src/agents.rs:101-107` | test pins the 2-element cohort | yes |
| `apps/desktop/src/lib/presentation/agents.ts:100-105` | `pi.supportsAnalysis: false` | yes |
| `docs/support.md:35` and `:39-42` | Pi row has no analysis column; the "Session analysis" paragraph enumerates no providers | see §7.3 |
| `crates/antiburn-local/src/discovery/source_version.rs:52-53` | `matches!(descriptor.agent, AgentKind::Claude \| AgentKind::Codex)` ⇒ `RecordStream` | optional, **no behavioural effect** — see below |

**`Streamability` is currently unread metadata.** A repository-wide grep for the
symbol returns exactly three sites: its definition and own tests in
`source_version.rs`, the re-export at `discovery/mod.rs:44`, and one assertion
at `discovery/tests.rs:86`. Nothing under `apps/desktop/src-tauri` reads it;
`store/model.rs`'s `SourceVersionState` does not carry it. **The evidence
streaming path is gated solely by `analysis.rs:521-527` returning `Some`, per
`analysis.rs:540-545`.**

Adding `AgentKind::Pi` to `source_version.rs:52-53` is therefore a **cosmetic
consistency change with no behavioural effect**, not a scope gap. It is optional.
If taken, it **must** be accompanied by an edit to `discovery/tests.rs:60-86`,
which builds a `SessionLog { agent_type: AgentKind::Pi, .. }` and asserts
`version.streamability == Streamability::WholeDocumentFallback` — that test fails
otherwise. See §17-R1.

### 2.7 Persistence

`apps/desktop/src-tauri/src/store/schema.rs:13`

```rust
pub const MIGRATIONS: &[&str] = &[V1, ..., V12];
```

`schema.rs:8-9` — *"Never edit an entry that has shipped: an installed database
has already run it."*

`schema.rs:329-340` — `V12` is the Codex backfill:

```sql
INSERT INTO session_evidence (environment_key, agent, session_id)
SELECT environment_key, agent, session_id FROM session WHERE agent = 'codex'
ON CONFLICT(environment_key, agent, session_id) DO NOTHING;
```

**VERIFIED** — V12 exists **only on this branch**. `origin/main` is at V11.

**VERIFIED and load-bearing** — `apps/desktop/src-tauri/src/store/mod.rs:682-712`,
`reconcile_evidence_revisions`, already performs the same enrolment for **every**
cohort agent:

```sql
INSERT INTO session_evidence (environment_key, agent, session_id)
SELECT session.environment_key, session.agent, session.session_id
  FROM session WHERE session.agent IN (...) AND NOT EXISTS (...)
```

**VERIFIED** — it has **two** call sites: `apps/desktop/src-tauri/src/lib.rs:223`
(startup) and `lib.rs:670`. Check both when validating D13's backfill timing.
*(Second site added in revision 2 — §17-R40.)*

**Adding Pi to `evidence_cohort()` therefore satisfies #244's "existing Pi
sessions are enqueued/backfilled" criterion without any migration.** See
DECISION D13.

`store/mod.rs:719-753` — the requeue `UPDATE` fires only on a source-generation
change or a revision change. A row already in a terminal state with an unchanged
generation and unchanged revisions is **not** requeued.

### 2.8 Revision constants and their blast radius

`crates/antiburn-local/src/analysis/mod.rs:83-86`

```rust
pub const PARSER_REVISION: i64 = 3;
pub const ANALYZER_REVISION: i64 = 5;
pub const METRICS_SCHEMA_REVISION: i64 = 1;
pub const EVIDENCE_SCHEMA_REVISION: i64 = 2;
```

**VERIFIED blast radius.** These are global, not per-vendor:

- `apps/desktop/src-tauri/src/insights_report.rs:20-26` — `CURRENT_EVIDENCE_PREDICATE` excludes any row where `e.parser_revision IS NOT ?4`.
- `insights_report.rs:33-40` — such a row lands in the `'stale'` denominator bucket, i.e. it **leaves** the assessed population until reanalysis.
- `store/mod.rs:719-753` — the requeue `UPDATE` resets a row to `pending` when `evidence.parser_revision IS NOT ?N`, for **every agent in the cohort**.

A `PARSER_REVISION` bump therefore invalidates every persisted Claude and Codex
evidence row. See DECISION D1.

**VERIFIED** — `insights_report.rs:28-40` also computes
`awaiting_provider_support = CASE WHEN s.started_at_epoch IS NOT NULL AND
e.status IS NULL THEN 1 ELSE 0 END`. A Pi session with no evidence row counts as
*awaiting provider support* today; cohort membership moves it into `pending`,
then `ready` or `stale`. See §13.2 and M6 in §15.

### 2.9 Unknown-type machinery that exists today

- `analysis/interface.rs:71-73` — `EvidenceObservation::UnrecognizedType { discriminator: String }`.
- `analysis/evidence.rs:8` — `EVIDENCE_STRING_CAP: usize = 256`; `evidence.rs:12` — `MAX_UNRECOGNIZED_TYPES: usize = 16`.
- `analysis/evidence.rs:454-466` — `cap_string`, which truncates on a char boundary and records `diagnostics.truncated_strings`.
- `analysis/evidence_sink.rs:411-427` — bounded insert into `diagnostics.unrecognized_types`; overflow calls `note_collection_cap` and degrades coverage to `Partial(CapExceeded)`.
- `analysis/evidence_sink.rs:159` — the call site that records a loss reason; `evidence_sink.rs:843-847` — `set_record_loss_reason`, which keeps the **first** reason only.
- `analysis/evidence_sink.rs:823-841` — `supported_value`, the mechanism by which one `record_loss_reason` degrades **every supported** group to `Partial` while unsupported groups stay `Unsupported`.
- `analysis/framing.rs:166-174` — `PartialReason` variants incl. `UnrecognizedRecordType`; `analysis/evidence.rs:31-41` — `CoverageReason`, the persisted mirror.
- `analysis/evidence_sink.rs:1050-1058` — pins the Claude behaviour: `unrecognized_types == BTreeSet::from(["telemetry_ping"])`, i.e. **the observed type string**.

Codex's inert-row allowlist is `vendors/codex.rs:425-448` — `is_recognized_eventless`,
a closed `matches!` over `(record_type, payload_type)` pairs. That is the shape
to copy for Pi.

### 2.10 The adapter surface #243 created

**VERIFIED** — `analysis/interface.rs:90-103`:

```rust
pub struct SessionSummary {
    pub cache_write_tokens_available: bool,
    pub context_window: Option<u64>,
    pub model: Option<String>,
    pub started_at_ms: Option<i64>,          // provider-declared session start
    pub coverage_gaps: Vec<PartialReason>,   // gaps known only at end of stream
    pub late_tools: Vec<(usize, ToolCall)>,
    pub initial_context: Option<InitialContextBreakdown>,
    pub skill_descriptions: HashMap<String, String>,
}
```

`interface.rs:158` folds `late_tools` into the collector; `interface.rs:186-187`
folds `coverage_gaps` into `partial_reasons`; `evidence_sink.rs:573-575` folds
them into the record-loss reason; `metrics_sink.rs:151,232` consume
`initial_context`, `skill_descriptions`, and `late_tools`.

Codex's `finish` (`codex.rs:394-416`) populates all eight fields and uses
`CodexContextAccumulator` (`initial_context.rs:383-440`). Codex pins
`started_at_ms` with `codex_characterization.rs:536`.

### 2.11 Source-validity reality

**VERIFIED** — `analysis/source_validity.rs:42-44`:

```rust
pub fn append_only_guarantee(_agent: &str) -> AppendOnlyGuarantee {
    AppendOnlyGuarantee::Absent
}
```

Every agent is `Absent`. The `recheck_prefix` / `AcceptedPrefix { boundary }`
branch at `codex.rs:127-134` is **unreachable in production**.

**VERIFIED** — the Codex suite has exactly **two** source-validity tests:
`incomplete_active_writer_tail_is_partial_and_keeps_the_valid_prefix`
(`codex_characterization.rs:462`) and
`claimed_codex_source_rejects_a_change_instead_of_publishing`
(`codex_characterization.rs:500`), plus a `source_claim()` helper at `:485`.
There is no oversized, short-read, append-after-claim, or cancellation test, and
no shared harness beyond that helper. `visit` hard-codes `cancel = &|| false`
(`codex.rs:75,80`), so cancellation is reachable only through `visit_claimed`.

### 2.12 Framing bounds

**VERIFIED** — `analysis/framing.rs:8` `MAX_RECORD_BYTES = 8 * 1024 * 1024`.
`framing.rs:21-23` `BoundedJsonlReader::new` hard-codes that limit;
`framing.rs:25` `with_max_record_bytes` is public but is **not reachable through
a production adapter entry point**. #244 requires production entry points in
tests. Codex has no oversized fixture for this reason. See §6 scenario 12.

---

## 3. Decisions

### DECISION D1 — preserve the shared projection revisions

Adding Pi changes cohort enrollment and provider routing, not the persisted
Claude or Codex projection contracts. Keep `PARSER_REVISION = 3`,
`ANALYZER_REVISION = 5`, `METRICS_SCHEMA_REVISION = 1`, and
`EVIDENCE_SCHEMA_REVISION = 2`. V13 enrolls Pi without globally staling the
existing cohort. Pi's non-turn and inherited-row observations are transient sink
inputs and add no persisted field.

### DECISION D2 — the top-level row `timestamp` is authoritative

Pi rows carry both a top-level `timestamp` and a nested `message.timestamp`.
Aggregate local observation found the two differ on essentially every sampled
assistant row.

Choose the **top-level row `timestamp`** for ordering, `TimeRange`, duration,
`OrderingObservation`, and `NormalizedEvent::ts_ms`.

Rationale: it is present on every row type including housekeeping rows, whereas
`message.timestamp` exists only under `message`. Ordering evidence must be
derivable from all rows, not a subset.

D2 also settles `SessionSummary.started_at_ms`: it comes from the `session`
header's top-level `timestamp`. When the header is absent — which is the case
for **both pinned fixtures in `analysis/tests.rs`** (§2.4) — `started_at_ms` is
`None` and coverage does **not** degrade. See §9.1 constraint 5.

Pinned by a characterization test using a fixture where the two values are in
opposite order (§6, scenario 19), and by a Codex-style `started_at_ms` test
(§6, scenario 23).

The aggregate structural measurement in §4.4 confirms this decision.

### DECISION D3 — use the Pi header timestamp as the fork boundary

The adapter checks only whether the session header contains the parent-link key.
It never reads that key's path value. For a linked child, every subsequent row
must carry a parseable top-level timestamp. Rows before the header timestamp are
inherited and contribute no child metric, evidence, or time-range value. Rows at
or after the header timestamp are owned. The sink receives an explicit inherited
observation for each excluded row, so diagnostics do not omit it.

A missing or malformed ownership timestamp fails closed with
`AttributionIncomplete`; the adapter never guesses from row types. Synthetic
fixtures pin an inherited configuration preamble, the normal owned preamble, a
fork with no inherited rows, and unresolved ownership.

### DECISION D4 — inspect parent-link presence, never its value

The parent-link value is path-shaped private data. The adapter checks key
presence only to activate D3. It never parses, copies, hashes, logs, persists,
or compares the value. Privacy tests assert that the synthetic path marker does
not reach metrics or evidence.

### DECISION D5 — four usage buckets only

Pi `usage` objects carry more keys than the four `parse_usage` reads. Aggregate
observation found, across a large sample of assistant rows:

- `totalTokens == input + output + cacheRead + cacheWrite` held on every sampled row.
- `reasoning <= output` held on every sampled row that had `reasoning`.
- `cacheWrite1h <= cacheWrite` held on every sampled row that had it.

`reasoning` is a subset of `output`; `cacheWrite1h` is a subset of `cacheWrite`.
Adding either as a fifth or sixth bucket inflates tokens and cost.

The Pi adapter reads exactly `input`, `output`, `cacheRead`, `cacheWrite`. It
ignores `totalTokens` for accounting (usable only as a self-consistency check in
a test), and ignores `cost` entirely per #244 §7 and Non-goals.

Pinned by a characterization test whose fixture sets `reasoning` and
`cacheWrite1h` to non-zero values and asserts the four buckets are unchanged
(§6, scenario 4b).

### DECISION D6 — emit the observed type string; never a `customType` value

**Revised in revision 1.** The previous formulation ("a fixed structural
discriminator drawn from a closed set the adapter owns" for unknown types) was
unimplementable — an unknown type is by construction not in a closed set — and
would have made Pi's `diagnostics.unrecognized_types` mean something different
from Claude's, which `evidence_sink.rs:1050-1058` pins as the **observed type
string**. That is exactly the Pi-only interpretation #244 §5 and Non-goals
forbid. See §17-R8.

The rule is:

1. An unknown **top-level row type** emits its observed `type` string through `cap_string` (`evidence.rs:454`), matching Claude.
2. An unknown **content-block type** emits its observed block `type` string the same way.
3. A `custom` / `custom_message` row emits **its row type only** (`"custom"` or `"custom_message"`). **The `customType` value is never emitted.**
4. Ordinary structurally inert `custom` rows emit no discriminator observation.

Rule 3's rationale, unchanged: `customType` names a user-installed extension.
Aggregate observation found the distinct-value count across a local corpus is of
the same order as `MAX_UNRECOGNIZED_TYPES = 16`, with several per session.
Emitting them would (a) persist a fingerprint of the user's installed extension
set into `session_evidence.evidence_json`, and (b) exhaust the 16-slot cap on
ordinary sessions, degrading them to `Partial(CapExceeded)` for a reason with no
analytic value. It also forecloses the #229 risk that runtime discriminators
reach the analytics catalog, whose `label` is `Option<&'static str>`.

This follows the privacy-safe structural disposition selected by the final
review. No persisted suppression field or waiver is required.

### DECISION D7 — `visit`/`visit_claimed` mirror the Codex shape exactly

Copy the structure at `vendors/codex.rs:69-189`:

- `visit` dispatches on `RawSource::{File, Jsonl, Sqlite}`; `Sqlite` bails; `Jsonl` chains a `\n` suffix when the content does not end in one; passes `cancel = &|| false`; returns `VisitOutcome::Unvalidated`.
- `visit_claimed` requires `RawSource::File`, opens a `PinnedSource`, limits reads to `claim.boundary` under `AppendOnlyGuarantee::Evidenced` and `u64::MAX` under `Absent`, then `recheck_prefix` / `recheck_full`, returning `AcceptedPrefix { boundary }`, `AcceptedFull`, or `SourceChanged(reason)`.
- On `SourceChanged`, return **before** `sink.finish`.
- `visit_reader` wraps a `BoundedJsonlReader`; `Oversized` / `IncompleteTail` become `NormalizedRecord::Unusable(skip.partial_reason())`; `ReadFailed` / `Cancelled` `bail!`; non-UTF-8 `bail!`; unparseable JSON becomes `Unusable(PartialReason::MalformedRecord)`.

Do not invent a Pi-specific outcome vocabulary. Per §2.11, only
`Unvalidated`, `AcceptedFull`, and `SourceChanged(_)` are reachable in
production; the `AcceptedPrefix` arm is written for symmetry and is dead until
someone adds a Pi arm to `append_only_guarantee`, which is out of scope.

### DECISION D8 — correct the stale discovery comment, do not change the ID source

`discovery/agents/pi.rs:76-78` claims Pi transcripts carry no in-content session
ID. The `session` header does carry `id`, and aggregate observation found it
agrees with the filename UUID on every sampled file.

Correct the comment. **Do not** switch `recover_session_id_from_path` to read
the header. The persisted `session_evidence` primary key is
`(environment_key, agent, session_id)`; changing the ID source would orphan
every existing Pi row for a benefit of zero.

### DECISION D9 — do not add a persisted evidence shape for Pi

`thinking_level_change` rows map onto the existing free-string
`NormalizedEvent::thinking_mode`. Transient `RecordTimestamp` and
`InheritedRecord` observations let the shared sink account for non-turn rows;
they add no serialized field. No new capability or DTO field is introduced, so
`EVIDENCE_SCHEMA_REVISION` stays unchanged.

### DECISION D10 — keep four focused Pi goldens, and keep cost out of them

**VERIFIED cost** — the three Codex goldens are 4,161 / 4,215 / 4,369 lines
(12,745 total, 351 KiB). Reviewing a doubled corpus of generated JSON is not
feasible.

Take goldens for `content_blocks`, `usage_all_buckets`, `unknown_row_type`, and
`unknown_content_block`. Every other scenario uses targeted assertions.
Keep the `UPDATE_GOLDENS=1` escape hatch that `codex_characterization.rs`
implements.

**Added in revision 1, softened in revision 2**: the golden payload includes
`metrics()`, whose `cost` depends on the pricing table and
`install_runtime_pricing`. Exclude `cost` from the golden JSON and assert it
separately, or use a model whose price is stable.

**VERIFIED** — the risk is currently hypothetical rather than active: all three
Codex goldens already serialize `"cost": null`, because no runtime pricing is
installed in the test binary. Excluding `cost` is still the right rule — it costs
nothing and removes a latent coupling — but it is not urgent, and an implementer
should not spend review time on it. *(§17-R41.)*

### DECISION D11 — Pi recognizes its own housekeeping rows explicitly

See §10 for the full three-way classification table. The Pi adapter carries a
closed `is_recognized_inert(row_type, role, value)` predicate, modelled on
`codex.rs:425-448`, so Pi is correct under either #229 outcome.

### DECISION D12 — `cache_write_tokens` is a per-session summary flag, not a static capability

**New in revision 1. This replaces the former GATE G3b.** See §17-R4.

`evidence_sink.rs:571-572` overwrites the declared `cache_write_tokens` with
`SessionSummary.cache_write_tokens_available`. And `metrics_sink.rs:420,438`
branch on the same flag: when it is `true` the sink uses the direct cache-
rehydration test; when it is `false` it discards those events and runs an
inferred `windows(3)` heuristic instead.

Pi today gets `true` from `generic_jsonl.rs:30` and reports a real `cacheWrite`
value. Declaring `false` would therefore (a) change existing Pi cache metrics,
violating #244 §3 parity, and (b) **synthesize inferred cache-churn events for a
source that reports the real number** — the opposite of honest.

Therefore:

- `SourceCapabilities::pi()` declares `cache_write_tokens: true`.
- `PiStreamState::finish` sets `cache_write_tokens_available` per session: `true` by default, `false` **only** when the session contains assistant turns whose `api` cannot report `cacheWrite`.
- No corpus measurement is required: the condition is computable in-adapter from the rows themselves.

§5.3 test 1 must assert both the declared struct **and** that the emitted
evidence's `cache_write_tokens` equals the summary flag. Fixture **22a**
(`mixed_api`, slice 2) asserts the degraded summary flag; fixture **22b** (same
fixture through the evidence sink, slice 4) asserts the degraded emitted
capability. §9.1 constraint 4, authored in slice 5, asserts the metrics
consequence.

### DECISION D13 — add the Pi backfill migration

Repair round 1 adopts the issue's explicit migration requirement. V13 enrolls
existing Pi sessions with `ON CONFLICT DO NOTHING`, so a ready, failed, or
processing evidence row keeps its state. Startup reconciliation remains as the
idempotent recovery path for missing rows and stale revisions.

The migration is one-way under `schema.rs:8-9`. A later defect therefore needs
a forward V14 repair rather than an edit to V13.

### DECISION D14 — three adapter mappings, stated explicitly

**New in revision 1.** See §17-R20.

1. **`NormalizedSession::context_window` / `SessionSummary.context_window` stay `None`.** No Pi row carries a context-window size. Any percentage-of-window claim must therefore be unavailable rather than estimated. Pinned by a test.
2. **Compaction** maps `tokensBefore → NormalizedEvent::compaction_pre_tokens` (`model.rs:306`); `compaction_post_tokens` and `compaction_trigger` stay `None` (`model.rs:302,310`) because Pi records neither. **No Codex-style dedupe window** — Pi writes one marker per compaction event, so `codex.rs:376-393`'s `is_duplicate_boundary` must not be ported. Do not map a `fromHook`-style field to `compaction_trigger` without a schema review.
3. **`message.model` is authoritative per turn; `model_change` is a transition marker only.** `NormalizedEvent::model` (`model.rs:266-272`) is per-record and the engine attributes usage by it. Fixture 5 must pin the **disagreement** case — a turn whose `message.model` differs from the most recent `model_change` — not merely "A is not rewritten to B".

---

## 4. Privacy-safe local-schema procedure

Real files under `~/.pi` are private source material. #244's fixture policy is
binding. This section is the procedure implementers must follow; it is not
optional. **It binds this document too** (§17-R18).

### 4.1 Hard prohibitions

Never write, print, commit, or paste — in any file, prompt, comment, log, test
output, PR body, or temporary artifact:

- a transcript value of any kind — prompt text, model output, thinking text, tool arguments, tool output, commands, diffs, source code, compaction summaries;
- a path, `cwd`, repository name, branch name, organisation name, or person name;
- a session id, message id, `parentId`, `toolCallId`, model response id, or any `*Signature` value;
- a real timestamp from a real file;
- an account, provider, or credential identifier;
- **a hash, checksum, or digest of a real file**;
- **an exact size or exact count that could fingerprint a private source.** Bucketed orders of magnitude and comparisons against a repository constant are permitted — see §4.2.

### 4.2 What may leave the inspection

Only these:

- field **names** and their JSON **types**;
- row `type` discriminator values that are Pi's own vocabulary, not user data;
- **counts and sizes bucketed to an order of magnitude**, or stated as a comparison against a repository constant (e.g. *"the largest observed line is below `MAX_RECORD_BYTES`"*, *"distinct `customType` values are of the same order as `MAX_UNRECOGNIZED_TYPES`"*);
- **ratios and boolean invariants** stated without their denominators ("held on every sampled row", "a low single-digit fraction of a percent of sampled pairs");
- structural relationships ("every non-header row has both fields").

This carve-out is deliberate and reconciles §4.1's size prohibition with the
performance sizing §13.2 needs.

### 4.3 Procedure

1. Write the inspection script to a throwaway path outside the repository (`/tmp`). Never inside the worktree, where it could be staged.
2. **Read versus print.** The script **may read** any field in memory, including `id`, `parentId`, `parentSession`, `toolCallId`, and `cwd` — G2 and G3 are otherwise unmeasurable. It **must never print** any of them, nor any derived value from which one could be reconstructed. It must never read into, nor print anything from, the content-bearing subtrees: `content`, `text`, `thinking`, `arguments`, `data`, `details`, `display`, `summary`, `command`, `output`. Only the categories in §4.2 reach stdout. *(Revised in revision 1 — see §17-R19.)*
3. Sanitize key names before printing. A map keyed by a user path would otherwise surface as a "field name". Reject any key that is not `[A-Za-z_][A-Za-z0-9_]*`.
4. Compute no hash of a real file. If a comparison needs one, keep it in memory and report only the resulting ratio.
5. Delete the script and any scratch output when the pass ends.
6. Record findings in §16's log as aggregate statements only, bucketed per §4.2.

### 4.4 Aggregate structural measurements and dispositions

The final review completed the privacy-safe aggregate measurements under §4.3.
No transcript values or parent paths entered the repository.

| Gate | Aggregate structural result | Disposition |
| --- | --- | --- |
| G1 | Top-level row timestamps cover the Pi row vocabulary and expose inherited fork prefixes relative to the header. | D2 remains authoritative. |
| G2 | A substantial fork cohort carries `parentSession` on the session header. The value is path-shaped. | Check key presence only; never read the value. |
| G3 | `parentId` does not prove a branching conversation-depth contract. | `thread_identity: false`. |

The same pass measured row, role, content-block, usage-key, version, model, and
API shapes in aggregate. It found ordinary custom extension payloads with nested
`content`, `details`, and `display` keys. This result drives the narrow
non-recursive structural test in §10.2. It also found no reason to add a total
record cap; shared per-record framing and bounded retained collections remain
the architecture contract.

### 4.5 Shapes both passes agree exist and #244's format list omits

#244's "Pi format to characterize" does not mention these. Both passes found
them. Each is a correctness or privacy hazard.

| Shape | Where | Why it matters |
| --- | --- | --- |
| `custom_message` row type | top level | Content-bearing (`content`, `display`, `details`). Not in the issue's row list. **Cannot be blanket-allowlisted as inert** — see §10.2. |
| `image` content block | inside `message.content[]` | Carries base64 `data` plus `mimeType`. Largest single record and the top leak vector. Not in the issue's block list. |
| `bashExecution` role | `message.role` | Carries `command`, `output`, `exitCode`, `truncated`, `cancelled` — raw shell text. Not in the issue's role list. **A present-but-unrecognized role fails closed today**, so every real Pi session containing one would degrade unless the adapter recognizes it — see §10.2. |
| `usage.reasoning` | assistant usage | Subset of `output`. See D5. |
| `usage.cacheWrite1h` | assistant usage | Subset of `cacheWrite`. See D5. |
| mixed `api` values in one session | assistant metadata | `anthropic-messages`, `openai-codex-responses`, `openai-completions` can co-occur. Drives D12's per-session flag. |

All six must appear in the fixture matrix (§6) and in the capability rationale
(§5).

---

## 5. Pi capability matrix

### 5.1 Proposed `SourceCapabilities::pi()`

Add to `crates/antiburn-local/src/analysis/evidence.rs` after `codex()` (line
346). Every field needs a one-line rationale in a doc comment, mirroring the
Codex fixture README's table.

| Capability | Pi | Rationale | Gate |
| --- | --- | --- | --- |
| `request_context_tokens` | **true** | Request occupancy is `Usage::context_tokens()` = `input + cacheRead + cacheWrite` (`model.rs:33-37`); `peak_context_tokens` is its running maximum (`metrics_sink.rs:105`). Pi assistant rows carry all three. | — |
| `cache_write_tokens` | **true (declared)** | Declared `true`; overwritten per session by `SessionSummary.cache_write_tokens_available` at `evidence_sink.rs:572`. See D12. | — |
| `timestamps_and_order` | **true** | Aggregate measurement confirms top-level timestamps across the recognized row vocabulary (D2). | resolved |
| `tool_invocations` | **true** | `toolCall` blocks and `toolResult` roles are already parsed (`analysis/tests.rs:333`). | — |
| `skill_mcp_attribution` | **false** | Pi rows carry no server or skill provenance the adapter can read. No ingestion path. | — |
| `tool_definitions` | **false** | No persisted tool catalogue in a transcript. | — |
| `model_identity` | **true** | `message.model` present on assistant rows in every sample; `model_change` rows give transitions (D14.3). | — |
| `token_classes` | **true** | Four disjoint buckets (D5). | — |
| `reasoning_effort_tier` | **true** | `thinkingLevel` ∈ a small closed set incl. `xhigh`, representable as the existing free-string `thinking_mode` (D9). | — |
| `fast_tier` | **false** | No speed-mode field. | — |
| `service_tier` | **false** | No service-tier field the adapter extracts. | — |
| `subagent_relationships` | **false** | D3, D4. Ownership is per file; no readable link. | — |
| `subagent_models` | **false** | Same. | — |
| `compaction_boundaries` | **true** | `compaction` rows mark a real event, one row per event, no dedupe needed (D14.2). | — |
| `thread_identity` | **false** | Aggregate structure does not prove conversational depth. | resolved |
| `quota_incidents` | **false** | No incident ingestion path in the evidence sink. Same as Codex. | — |
| `harness_version` | **false** | No version ingestion path in the evidence sink. Same as Codex. | — |

### 5.2 Detector consequences

`insights/report.rs:140-190`, verbatim requirement clauses:

| Detector | Required capabilities | Required groups |
| --- | --- | --- |
| `SessionsOverDepth` | RequestContextTokens, ModelIdentity, **ThreadIdentity**, TimestampsAndOrder | Context |
| `ModelOverthinking` | ReasoningEffortTier, ModelIdentity | Models, Eligibility |
| `OverpoweredSubagents` | ModelIdentity, SubagentModels, SubagentRelationships | Subagents, Models |
| `UnusedMcpServers` | SkillMcpAttribution, ToolInvocations | ContextSources, Tools, Eligibility |
| `UnusedBuiltInTools` | ToolDefinitions, ToolInvocations | ContextSources, Tools |
| `UnusedSkills` | SkillMcpAttribution, ToolInvocations | ContextSources, Tools, Eligibility |
| `OldModelUsage` | ModelIdentity, TimestampsAndOrder, TokenClasses | Models, TimeRange |
| `OveruseOfFastMode` | FastTier **or** ServiceTier, SubagentRelationships | Models, Subagents |
| `CacheChurn` | TimestampsAndOrder, **ThreadIdentity**, ModelIdentity, CacheWriteTokens, CompactionBoundaries | Cache, Compactions, Models |

`EvidenceGroup::Context` is gated on `request_context_tokens` alone
(`report.rs:97` → `evidence_sink.rs:672-676` → `supported_value` at `823-841`).
It does **not** depend on `initial_context`, which flows only to metrics
(`metrics_sink.rs:151`). See §17-R6.

With `thread_identity: false`, Pi satisfies `ModelOverthinking` and
`OldModelUsage`. The frozen detector test pins this resolved reach.

### 5.3 Pinning the matrix

Three tests, all in the Pi characterization suite:

1. **Exact-value test.** Assert all 17 fields of `SourceCapabilities::pi()` against a literal, **and** — per D12 — assert for each fixture that the emitted `evidence.capabilities.cache_write_tokens` equals that fixture's expected `SessionSummary.cache_write_tokens_available`, not the constructor default. Without the second half this test passes for the wrong reason.
2. **Capability ⇄ evidence agreement test.** For each fixture, assert that every group the matrix claims is supported is present in the emitted evidence, and every group it does not claim is `Unsupported`. This is the anti-overclaim artefact and is the single most valuable test in the suite.
3. **Frozen detector-eligibility table.** Assert the exact set of `DetectorId`s Pi's capability set satisfies, using `insights::requirements` and `EvidenceGroup::state`. A capability flip then fails a test that names the detector it unlocked.

   The #242 badge reducer is absent from the #243 stacking base. Do not copy it
   into this branch. Rebase onto `origin/main`, then run the existing #242
   badge/report contract against Pi as the final integration step.

---

## 6. Synthetic fixture matrix

Location: `crates/antiburn-local/tests/fixtures/pi_characterization/`.
Suite: `crates/antiburn-local/tests/pi_characterization.rs`, structured after
`tests/codex_characterization.rs` (556 lines; `fixture()` `include_str!` match
at 19–47, `fixture_names()` at 50–62, `collect()` at 71–79, `composite()` at
81–98, golden helpers at 100+, `source_claim()` at 485).

**Every fixture is hand-authored and synthetic.** Synthetic ids follow an
obvious pattern (`s-1`, `m-1`, `call-1`); synthetic text is the minimum needed
to classify (`"ok"`, `"fail"`); timestamps are round values in a fixed synthetic
year; token values are small and internally consistent.

**Provenance note.** Codex's README cites `openai/codex@e9a446d` as the public
authority for its shapes. **Pi has no public schema repository to cite.** The Pi
README must state that the fixtures were authored from *aggregate structural
observation of a local installation, recording field names, types, and
invariants only, with no captured session data* — and must not name a version, a
file, or a count that could fingerprint the source (§4.1).

### 6.1 Matrix

`§` maps to #244's numbered characterization list where one exists. **Src**:
`C` = copy an existing Codex test shape; `N` = new harness work.

| # | Fixture | § | Src | Structural fact under test | Expected outcome |
| --- | --- | --- | --- | --- | --- |
| 1 | `minimal_session.jsonl` | 1 | C | header + one user + one assistant | `Complete`; one turn each |
| 2 | `role_ordering.jsonl` | 2 | C | user → assistant → toolResult | ordered turns, correct roles |
| 3 | `content_blocks.jsonl` | 3 | C | `text`, `thinking`, `toolCall`, and `toolResult` | thinking detected and tool categories preserved; no new shared error metric |
| 4 | `usage_all_buckets.jsonl` | 4 | C | four buckets; at least one turn with **non-zero `cacheRead`** | `peak_context_tokens == max(input + cacheRead + cacheWrite)` over turns (§2.3) |
| 4b | `usage_subset_keys.jsonl` | new | N | `reasoning` and `cacheWrite1h` non-zero | buckets identical to 4; D5 pinned |
| 5 | `model_change.jsonl` | 5 | C | assistant on model A, `model_change` to B, then an assistant turn whose `message.model` **disagrees** with B | per-turn `message.model` wins; A not rewritten; D14.3 |
| 6 | `thinking_level_change.jsonl` | 6 | C | explicit level change mid-session | transition preserved; no retroactive assignment |
| 7 | `compaction_and_inert.jsonl` | 7 | C | `compaction` (with `tokensBefore`) + `session_info` + `model_change` | one boundary; `compaction_pre_tokens` set, `post`/`trigger` `None`; inert rows do **not** degrade coverage; D14.2 |
| 8 | `unknown_row_type.jsonl` | 8 | C | a top-level `type` outside the closed set | the **observed type string** in `unrecognized_types`; `Partial(UnrecognizedRecordType)`; D6 |
| 9 | `unknown_content_block.jsonl` | 9 | C | a block `type` outside the closed set | the observed block type string; `Partial` |
| 9b | `custom_rows.jsonl` | new | N | `custom` + `custom_message` with ordinary nested extension payloads | complete coverage; no discriminator persisted; `customType` values never read; D6, §10.2 |
| 10 | `malformed_middle.jsonl` | 10 | C | invalid JSON between two valid rows | neighbours retained; `Partial(MalformedRecord)` |
| 11 | `incomplete_final_record.jsonl` | 11 | C | final line truncated mid-object, no newline | prefix retained; `Partial(IncompleteTail)`; direct twin of `codex_characterization.rs:462` |
| 12 | *(runtime temp file, not committed)* | 12 | N | one line over `MAX_RECORD_BYTES` | see §6.2 |
| 13a | `header_only.jsonl` | 13 | C | header row only | no turns; honest empty outcome; no panic |
| 13b | `unsupported_version.jsonl` | 13 | N | header `version` outside the supported set | honest degraded outcome; decide and pin whether this is `UnrecognizedRecordType` or a full reject |
| 14a | *(harness)* | 14 | C | claimed full read of an unchanged file | `AcceptedFull` |
| 14b | *(harness)* | 14 | C | file appended to after the claim | `SourceChanged(_)`; sink never finishes; twin of `codex_characterization.rs:500` |
| 14c | *(harness)* | 14 | N | file replaced / identity changed after the claim | `SourceChanged(_)` |
| 14d | *(harness)* | 14 | N | cancellation via `visit_claimed` with a `cancel` closure that returns `true` | `bail!`; **not reachable through `visit`**, which hard-codes `&\|\| false` (`codex.rs:75,80`) |
| 15 | `fork_hazard.jsonl` (pair) | 15 | N | physically subsequent configuration and message rows have timestamps before the child header | inherited rows are observed but excluded; owned rows at or after the header contribute once; D3 |
| 15b | `fork_no_inherited.jsonl` | 15 | N | normal configuration preamble starts at the header timestamp | no rows are excluded; owned usage remains complete |
| 15c | *(targeted synthetic inputs)* | 15 | N | the header or a subsequent row lacks a parseable ownership timestamp | fail closed with `AttributionIncomplete` |
| 16 | *(all fixtures)* | 16 | N | `PiAdapter.normalize()` vs `PiAdapter.visit()`, called **directly on the adapter**, not through `adapter_for` (§8 export seam) | identical metrics for every fixture |
| 17 | *(all fixtures)* | 17 | N | serialized evidence privacy invariant | §13.1 list |
| 18 | *(all fixtures)* | 18 | N | capability ⇄ emitted evidence | §5.3 |
| 19 | `timestamp_disagreement.jsonl` | new | N | row `timestamp` and `message.timestamp` in **opposite** order | ordering follows D2 |
| 20 | `image_block.jsonl` | new | N | an `image` block with a short synthetic base64 payload | payload absent from evidence, metrics, goldens, diagnostics |
| 21 | `bash_execution_role.jsonl` | new | N | a `bashExecution` role row **carrying no `usage`** | passes the §10.2 shape test; recognized; does **not** degrade coverage; `command`/`output`/`exitCode` never reach evidence |
| **21b** | `bash_execution_with_usage.jsonl` | **new** | N | a `bashExecution` role row **carrying a non-zero `usage` object** | **fails closed** — `Unusable(UnrecognizedRecordType)`, `Partial`, and the usage is **not** silently counted. #229 open-question 4's guard fixture, which its author calls non-negotiable. §10.2 |
| 22a | `mixed_api.jsonl` | new | N | two assistant turns with different `api` values — **summary half** | `SessionSummary.cache_write_tokens_available == false`; D12 |
| 22b | *(same fixture, slice 4)* | new | N | the same fixture through the evidence sink — **capability half** | emitted `capabilities.cache_write_tokens == false`; needs `SourceCapabilities::pi()`; D12 |
| 23 | `session_start.jsonl` | new | C | header timestamp precedes the first event | `started_at_ms` from the header; twin of `codex_characterization.rs:536` |
| 24 | `headerless_tools.jsonl`, `headerless_usage.jsonl` | new | N | independently authored headerless tool and usage rows | full metrics, `started_at_ms: None`, and no coverage degradation; the existing batch tripwires remain unchanged |

Fixture 24 exists because those two inputs are the parity floor and are today
exercised only through `normalize_source`. Streaming them is the only way to
prove parity on precisely the pinned numbers. They are already synthetic and
already in the repository, so copying them introduces no privacy risk. See
§17-R23.

**Harness scope.** Only rows marked `C` in §6.1 have an existing Codex test to
copy. Everything marked `N` is new test code and belongs in slice 2's estimate.
There is no shared source-validity harness beyond `source_claim()`
(`codex_characterization.rs:485`). See §17-R10.

### 6.2 Scenario 12 — oversized record

**Revised in revision 1.** See §17-R9.

The fixture **cannot be a committed file**. `BoundedJsonlReader::new`
(`framing.rs:21-23`) hard-codes `MAX_RECORD_BYTES = 8 MiB`, and
`with_max_record_bytes` (`framing.rs:25`) is not reachable through a production
adapter entry point, which #244 requires tests to use. An 8 MiB fixture is 20×
the entire Codex golden corpus and would be streamed repeatedly by the
all-fixture loops.

Implement as:

- a **runtime-generated temporary file** (`tempfile::tempdir`, as `codex_characterization.rs:468` already does), holding one over-limit line between two small valid lines, consumed via `RawSource::File`;
- **excluded** from the all-fixture loops (16, 17, 18) and from the golden set;
- asserting `Partial(Oversized)` and that both neighbours are retained.

If the runtime cost is still judged too high, delete the scenario and record
that `framing.rs`'s own oversized tests already cover the framing behaviour;
the adapter contributes nothing Pi-specific to that path.

### 6.3 Fixture privacy gate

**Runs in every slice that stages a fixture — slices 1, 2, and 3 — not once at
the end.** *(Corrected in revision 2: fixtures ship in slices 1–3 but the gate
was scheduled only for slice 5, so every fixture would have shipped two to four
commits before its own privacy review. See §17-R36.)*

Steps 1–2 run **before each fixture commit**:

1. `pnpm run secrets` (secretlint).
2. Manual read of the **entire** staged fixture diff, line by line, against §4.1.

Step 3 runs in slice 5, when the goldens exist:

3. Confirm no golden contains a string that came from a real transcript. The privacy invariant test (17) is the automated half; the manual read is the half that catches a value pasted into a fixture rather than emitted by the sink.

The **full §13.1 sweep** (all six steps, including the diagnostics audit and the
`parentSession` grep) stays in slice 5 and is repeated before the final commit.

---

## 7. File and symbol change map

Line numbers are from `679a5c9` and **must be re-derived** on the D0 baseline.

### 7.1 Engine — `crates/antiburn-local`

| File | Symbol | Change |
| --- | --- | --- |
| `src/analysis/vendors/pi.rs` | **new** | `PiAdapter`, `impl VendorAdapter` (`agent`, `normalize`, `visit`, `visit_claimed`), `PiAdapter::visit_claimed`, `PiAdapter::visit_reader`, `PiStreamState` (`observe`, `process_value`, `observe_model_and_level`, `finish`), `is_recognized_inert(row_type, role, value)`, `parse_pi`, `pi_usage`, `record_to_event`. Module doc states the row/role/block vocabulary and cites D2, D3, D5, D6, D12, D14. |
| ↳ `PiStreamState::finish` | **all eight `SessionSummary` fields** | `cache_write_tokens_available` per D12; `context_window: None` per D14.1; `model`; `started_at_ms` from the header timestamp per D2, `None` when headerless; `coverage_gaps` — state which `PartialReason`s Pi reports at end-of-stream versus per-record `Unusable`; `late_tools: Vec::new()` unless a Pi-specific need appears; `initial_context: None`; `skill_descriptions: HashMap::new()`. **All eight must be set explicitly**; §2.10. |
| `src/analysis/vendors/mod.rs:7-14` | `pub(crate) mod pi;` | add the crate-visible module; expose only `PiAdapter` through `analysis`. |
| `src/analysis/mod.rs:80` | `pub use vendors::pi::PiAdapter;` | **new public API**, mirroring `pub use vendors::claude::ClaudeAdapter;` on the same line. Without it `tests/pi_characterization.rs` — a separate crate — cannot reach `PiAdapter` while unrouted. Slice 1. |
| `src/analysis/vendors/mod.rs:23` | `static PI` | `static PI: pi::PiAdapter = pi::PiAdapter;` — **slice 6 only**, together with the `adapter_for` arm. Adding it earlier is `dead_code` and fails `clippy -D warnings` (`ci.yml:117,:220`). |
| `src/analysis/vendors/mod.rs:31` | `adapter_for` | add `"pi" => &PI,` — **slice 6 only**, see §8 |
| `src/analysis/vendors/mod.rs:79` | `dedicated_adapters_are_recognized_case_insensitively` | add `"pi"` — slice 6 |
| `src/analysis/vendors/mod.rs:87` | `generic_fallback_vendors_have_no_dedicated_adapter` | **remove** `"pi"` — slice 6 |
| `src/analysis/evidence.rs:346` | `SourceCapabilities::pi()` | new constructor + doc comment carrying §5.1's rationale column |
| `src/discovery/agents/pi.rs:76-78` | doc comment | correct per D8; **do not** change `recover_session_id_from_path` |
| `src/discovery/source_version.rs:52-53` | streamability match | **optional, no behavioural effect** (§2.6). If taken, must be paired with the next row. |
| `src/discovery/tests.rs:60-86` | `AgentKind::Pi` streamability assertion | update from `WholeDocumentFallback` to `RecordStream` **only if** the previous row is taken; otherwise leave untouched |
| `tests/pi_characterization.rs` | **new** | §6 suite |
| `tests/fixtures/pi_characterization/` | **new** | fixtures + `README.md` + `goldens/` (3 files, D10) |
| `benches/memory_baseline.rs`, `benches/pipeline_baseline.rs` | Pi arms | **new**, if §13.2's measurements are taken — both are Claude-only today |
| `CHANGELOG.md` | `[Unreleased]` | §11 — lands in **slice 6**, not slice 8 |

`src/analysis/tests.rs:333` and `:416` are **not** edited. They must pass
byte-identical.

### 7.2 Shell — `apps/desktop/src-tauri`

| File | Symbol | Change | Slice |
| --- | --- | --- | --- |
| `src/analysis.rs:521-527` | `capabilities_for_vendor` | add `"pi" => Some(SourceCapabilities::pi()),` | 6 |
| `src/agents.rs:47-49` | `evidence_cohort` | return type `[&'static str; 3]`, add `AgentKind::Pi.slug()` | 6 |
| `src/agents.rs:60-66` | `every_kind_resolves_to_a_vendor_label_the_registry_recognizes` | add `AgentKind::Pi` to the `dedicated` array | 6 |
| `src/agents.rs:87-98` | `generic_fallback_agents_report_no_dedicated_adapter` | **remove `AgentKind::Pi`** — this test fails the moment the registry flips | 6 |
| `src/agents.rs:101-107` | `the_evidence_cohort_uses_the_discovery_slug` | extend to three slugs; add `assert_eq!(AgentKind::Pi.slug(), vendor_label(AgentKind::Pi))` | 6 |
| `src/insights_report.rs` tests | new | pin the `awaiting_provider_support` → `pending` bucket transition for a Pi session across cohort membership (§13.2, M6) | 6 |
| `src/store/schema.rs:13` + `V13` | `MIGRATIONS` | append the Pi backfill migration; never edit it after shipment | repair 1 |
| `src/store/tests.rs` | Pi twin of `codex_cohort_migration_queues_existing_sessions_without_resetting_evidence` | prove missing rows queue and ready rows remain ready | repair 1 |
| `src/insights_worker.rs` | terminality + `errors_carry_no_transcript_content` | re-run with Pi sources; assert no unknown slug defaults to Claude | 6 |

**Removed in revision 1**: the former row claiming
`store/tests.rs:1987 a_session_outside_the_evidence_cohort_gets_no_row` has a Pi
expectation to flip. **VERIFIED false** — that test uses
`cursor.key.agent = "cursor"` (`store/tests.rs:1989-1990`) and there is no `"pi"`
literal anywhere in `store/tests.rs`. No change is needed. See §17-R13.

### 7.3 Frontend and docs

| File | Change | Slice |
| --- | --- | --- |
| `apps/desktop/src/lib/presentation/agents.ts:100-105` | `supportsAnalysis: false` → `true` | 6 |
| `apps/desktop/src/lib/presentation/agents.test.ts` | mirror test | 6 |
| `docs/support.md` | See below | 8 |
| `CHANGELOG.md` (root) | §11 | 8 |

**`docs/support.md` — the exact edit.** The table at `:25-37` has columns
Agent / Native / WSL / Notes. It has **no analysis column**, and the Pi row
(`:35`) says nothing about analysis. The prose paragraph at `:39-42` begins
*"**Session analysis** … need a transcript format antiburn understands in
detail. Where it has only a generic parse, the session is still listed and the
analysis view says so"* and **enumerates no providers**.

Therefore there is no per-agent analysis cell to change. The honest edit is
one of:

- **(a)** nothing — the document never claimed Pi lacked analysis, so nothing is now false; or
- **(b)** a Notes-column addition on the Pi row only if a reader would otherwise be misled.

Do **not** write "state Pi has dedicated analysis on macOS and Linux" as a table
edit; the table cannot express it. Do **not** touch the WSL column or claim
Windows support. Pick (a) or (b) at slice 8 and record the choice in §16.
See §17-R16.

Separately, check whether any *other* document enumerates full-evidence
providers by name; if one does, add Pi there.

No styling changes. `scripts/check-design-drift.mjs` should not be needed; run
it only if a style file unexpectedly enters the diff.

---

## 8. Ordered implementation slices

**Restructured in revision 1.** The previous ordering claimed slices 1–6 were
"zero user-visible change" and that flipping `agents.ts` was the kill switch.
Both were false. See §17-R2.

**VERIFIED — the registry flip is the switch.** `apps/desktop/src-tauri/src/agents.rs:37-39`:

```rust
pub fn supports_analysis(kind: AgentKind) -> bool {
    has_dedicated_adapter(vendor_label(kind))
}
```

There is no Rust flag to flip. `vendor_label(AgentKind::Pi) == "pi"`
(`model/agent.rs:75`). The instant `adapter_for("pi") => &PI` lands, four things
change with no further edit:

| Site | Effect |
| --- | --- |
| `analysis.rs:1223-1225 analysis_supported` → `commands.rs:850` → DTO `supports_analysis` | the DTO starts reporting `supportsAnalysis: true` for Pi |
| `SessionPane.tsx:347` — `payload?.supportsAnalysis ?? agentSupportsAnalysis(subject.agent)` | the **DTO wins** over the frontend registry; `agents.ts` is not the gate |
| `scan.rs:943` — `if !analysis::analysis_supported(agent) { continue; }` | Pi stops being skipped in `top_up_analysis`; every Pi session in the activity window gets a synchronous whole-transcript read each pass — **unless** the cohort skip at `scan.rs:930-932` is active |
| `commands.rs:618,627` — `cache_detail_analysis` / `cache_detail_relations` | Pi detail projections start being persisted, then stop once Pi joins `evidence_cohort()` |

`scan.rs:930-932` skips cohort agents **before** the `analysis_supported` check.
So the registry flip and cohort membership must land together, or Pi gets a
synchronous full-transcript read per scan pass in the interval. And
`capabilities_for_vendor` must land in the same commit as cohort membership, or
enrolled Pi rows can pass through a terminal `unsupported` state before revision
reconciliation repairs them.

**Consequence: slices 6's five edits are one atomic commit.** They are small in
lines — one match arm, one array element, one bool, four test arrays — so
atomicity costs nothing in reviewability.

Each slice is one reviewable commit (or a tight series), with DCO sign-off
(`git commit -s`).

### Slice 0 — gate resolution (no code)

Confirm the #243 stacking baseline and record the resolved structural decisions
from §4.4. Re-derive §7's line anchors after the eventual rebase.

**Exit**: every gate in §15's table has a recorded answer or an explicit waiver.

### How slices 1–5 reach `PiAdapter` at all — the export seam

**New in revision 2. Without this the first five slices cannot compile.**
See §17-R29.

**VERIFIED** — every adapter module in `vendors/mod.rs:7-14` is private except
`pub mod claude;`, and `analysis/mod.rs:80-81` exports exactly
`pub use vendors::claude::ClaudeAdapter;` plus
`pub use vendors::{adapter_for, has_dedicated_adapter};`.
`codex_characterization.rs` is a separate crate and reaches the adapter only
through `adapter_for("codex")` (`:74,:92,:517`) and `normalize_source`
(`analysis/mod.rs:217-219`, which itself calls `adapter_for`).

So while `adapter_for("pi")` still returns `&GENERIC`, an integration test has
**no reachable path to `PiAdapter`**. Every fixture in slices 1–3 and 5 would
either fail to compile or silently exercise `GenericJsonlAdapter` and pass for
the wrong reason.

**The seam, mirroring the `ClaudeAdapter` precedent:**

| Slice | Edit |
| --- | --- |
| 1 | `vendors/mod.rs` — `pub(crate) mod pi;`; `analysis` re-exports only `PiAdapter` |
| 1 | `analysis/mod.rs:80` — `pub use vendors::pi::PiAdapter;` |
| 1 | `crates/antiburn-local/CHANGELOG.md` `[Unreleased] ### Added` — `PiAdapter` is public API from this commit |
| **6** | `vendors/mod.rs` — `static PI: pi::PiAdapter = pi::PiAdapter;` **and** the `adapter_for` arm, together |

`static PI` is deliberately **withheld until slice 6**. An unreferenced static
is `dead_code`, which fails the plan's own §12.2 gate and CI
(`.github/workflows/ci.yml:117,:220`, both `cargo clippy --all-targets --locked
-- -D warnings`). Suppressing it has no precedent: the engine contains exactly
one `#[allow(...)]` (`repositories/sessions.rs:466`, `clippy::too_many_arguments`).
Adding the static and the match arm in the same commit avoids the lint without a
suppression.

**Consequently, in slices 1–5 every test constructs the adapter directly** —
`PiAdapter.normalize(&input)` and `PiAdapter.visit(&input, &mut sink)` — never
`adapter_for("pi")` or `normalize_source`. Slice 6 adds one test that the
registry now returns it. §6.1 rows 16 and 24 are phrased accordingly.

### Slice 1 — `PiAdapter` skeleton, batch path only, unrouted

Add `vendors/pi.rs` with `agent()`, `normalize()`, `parse_pi()`, `pi_usage()`,
`record_to_event()`, `is_recognized_inert()`. Add `pub(crate) mod pi;`, the
`pub use vendors::pi::PiAdapter;` export, and the engine changelog `### Added`
line. **Do not** add `static PI` and **do not** touch `adapter_for`.

Fixtures 1, 2, 3, 4, 4b, 5, 6, 7 with targeted assertions, all driven through
`PiAdapter.normalize()` directly.

**Exit**: engine tests green; `adapter_for("pi")` still returns generic;
`clippy -D warnings` clean with no suppression. **Run §6.3 steps 1–2 on the
staged fixtures before committing.**

### Slice 2 — streaming path

Add `visit`, `visit_claimed`, `visit_reader`, `PiStreamState` per D7, with all
eight `SessionSummary` fields per §7.1.

Fixtures: **10, 11, 12, 13a, 15, 19, 20, 22a, 23, 24**, plus source-validity
cases 14a–14d. These are the framing, ownership, ordering, and summary
scenarios — none of them needs D6's discriminator wiring.

- **15 (`fork_hazard`)** lands here: streaming compares every subsequent row timestamp with the child header timestamp.
- **22a** is the `mixed_api` fixture's **summary half** only: assert `SessionSummary.cache_write_tokens_available == false`. Its emitted-capability half is 22b in slice 4, because that needs `SourceCapabilities::pi()`. *(§17-R33.)*
- **24** lands here in full, and its "no coverage degradation" half is re-asserted in slice 3 once the recognition path exists. *(§17-R35.)*

Add the **streaming ⇄ batch parity** assertion (16) over every fixture built so
far, comparing `PiAdapter.normalize()` against `PiAdapter.visit()`.

Parity is essential: the two pinned tests in `analysis/tests.rs` use
`normalize_source`, so without it the batch path could silently diverge from the
path evidence uses.

**Exit**: engine tests green; still unrouted; §6.3 steps 1–2 run on the staged
fixtures.

### Slice 3 — recognition and coverage semantics

Freeze the classification in §10.2. Implement D6. Wire
`EvidenceObservation::UnrecognizedType` and
`NormalizedRecord::Unusable(UnrecognizedRecordType)` as `claude.rs:163-171` does.

Fixtures: **8, 9, 9b, 13b, 21, 21b**. They pin bounded discriminators,
`Partial(UnrecognizedRecordType)`, inert extension payloads, and the §10.2 shape
test, so none can pass before this slice.
*(Moved out of slice 2 in revision 2 — §17-R32.)*

Ship the adopted strict fail-closed policy while #229 owns the broader shared
rollout. Do **not** invent Pi-only coverage semantics.

**Exit**: fixtures 8, 9, 9b, 13b, 21, 21b produce the expected discriminators and
coverage; fixture 24's "no degradation for a missing header" half now holds
against the live recognition path; §6.3 steps 1–2 run on the staged fixtures.

### Slice 4 — capabilities  *(first slice requiring #243 merged)*

Add `SourceCapabilities::pi()` with `thread_identity: false` and D12 implemented. Add §5.3's
three pinning tests, and fixture **22b** (emitted `capabilities.cache_write_tokens
== false` for the mixed-`api` session).

**Exit**: capability tests green; the detector table matches the resolved matrix.

### Slice 5 — privacy, goldens, README, and the pre-flip metrics guard

Privacy invariant test (17) across every fixture, explicitly including 20, 21,
21b, 9b, 7. Generate the four goldens (D10, cost excluded). Write the fixture
`README.md`. Run the **full** §13.1 sweep and a manual staged-diff read.

**Also author the §9.1 constraint 4 guard here**, before slice 6 makes it
unobservable: a test that runs each fixture through **both**
`GenericJsonlAdapter` (Pi's adapter today) and `PiAdapter`, and asserts the
cache-rehydration event counts match for every fixture whose D12 flag is `true`,
and that fixture 22a/22b's degraded session takes the inferred path
deliberately. Slice 6 is atomic and user-visible; if this guard is not written
first there is no commit at which the before/after comparison can be made.
*(§17-R34.)*

**Exit**: the engine is complete and self-consistent; the pre-flip metrics guard
is green; Pi is **still on the generic adapter** and nothing user-visible has
changed.

### Slice 6 — the promotion commit  *(atomic; this is the switch)*

One commit containing **all** of:

- `vendors/mod.rs` — `adapter_for("pi") => &PI` and both registry test arrays;
- `analysis.rs:521-527` — `capabilities_for_vendor("pi")`;
- `agents.rs:47-49` — `evidence_cohort()` → 3 slugs;
- `agents.rs:60-66`, `:87-98`, `:101-107` — the three mirror tests;
- `agents.ts:100-105` + `agents.test.ts` — `supportsAnalysis: true`;
- the `insights_report` bucket-transition test;
- `crates/antiburn-local/CHANGELOG.md` — the engine entry (§11, M5).

**Exit**: both cargo workspaces and the frontend suite green; §13.2's scan-pass
and bucket-transition observations recorded.

### Slice 7 — migration *(taken in repair round 1, D13)*

Append V13 for Pi backfill with `ON CONFLICT DO NOTHING`, plus the store test
twin. Reconciliation remains the idempotent startup recovery path.

### Slice 8 — docs and root changelog

`docs/support.md` per §7.3, the root `CHANGELOG.md` entry, and any other document
enumerating full-evidence providers. Full verification (§12).

**Ordering constraints that are not negotiable:** slice 6 is atomic and is the
first user-visible change; slice 7, if taken, is one-way and lands after slice 6
is green in CI.

---

## 9. Metrics, evidence, source validity, and fork semantics

### 9.1 Metrics parity — hard constraints

1. `analysis/tests.rs:416` remains unchanged as the existing batch tripwire. New streaming fixtures use independent synthetic values and do not repeat captured usage or cost data. Keep its deliberately loose `tokens_in > 0` assertion unchanged. (§17-R14.)
2. `analysis/tests.rs:333` — tool categories `Read`, `Edit`, `Test`; `bash` + `cargo test` → `Test`; a `thinking` block is detected. This issue adds no shared tool-error metric.
3. Four disjoint buckets (D5), asserted positively (sum equals `totalTokens`) and negatively (adding `reasoning` or `cacheWrite1h` breaks the sum). Occupancy uses the **three-term** `context_tokens()` (§2.3).
4. **Cache-rehydration event parity (new).** `metrics_sink.rs:420,438` select two different algorithms on `cache_write_tokens_available`. Assert the emitted cache-rehydration event count is **identical before and after the registry flip** for every fixture whose expected flag is `true`, and that fixture 22a/22b's degraded session takes the inferred path deliberately. Without this, D12's per-session flag can silently change existing Pi metrics. (§17-R4.)

   **Owning slice: 5.** *(Assigned in revision 2 — §17-R34.)* "Before and after the registry flip" cannot be observed across slice 6, which is atomic. The test therefore runs each fixture through **both** `GenericJsonlAdapter` (Pi's adapter today, `generic_jsonl.rs:23-34`) and `PiAdapter` **in the same test body**, in slice 5, and compares. It is named in slice 5's exit criteria and in §15.
5. **Header-less sources must produce full metrics (new).** Both pinned inputs are bare `type:"message"` rows with no `session` header, no `id`, no `parentId` (§2.4). The adapter must therefore produce complete metrics from header-less content, set `started_at_ms: None`, and **not** degrade coverage for a missing header. Pinned by fixture 24. (§17-R15.)
6. **Streaming ⇄ batch parity for every fixture.**

### 9.2 Evidence

`SessionEvidenceAccumulator` is fed through `CompositeSink`, as
`codex_characterization.rs:81-98` does. Pi's transient timestamp and inherited
observations add no persisted shape (D9). `ThreadLink` is not emitted because
`thread_identity` is false; emitting it would be
a silent inconsistency between the declared matrix and the emitted evidence, and
test 18 must catch that.

### 9.3 Source validity — the reachable set only

Per §2.11, `append_only_guarantee` returns `Absent` for every agent, so
`AcceptedPrefix { boundary }` is unreachable in production. The outcomes Pi can
actually produce are:

- `VisitOutcome::Unvalidated` — unclaimed `visit`;
- `VisitOutcome::AcceptedFull` — claimed read of an unchanged file;
- `VisitOutcome::SourceChanged(_)` — identity mismatch, short-at-open, or any post-read recheck failure;
- an `Err` from `bail!` on cancellation, a read failure, or non-UTF-8 — cancellation only via `visit_claimed` (`codex.rs:75,80`).

The suite covers exactly that set (fixtures 14a–14d). Extending Pi to
`AcceptedPrefix` would require a Pi arm in `append_only_guarantee`, which
`source_validity.rs:39-41`'s own comment argues against and which is **out of
scope**. §15's acceptance criterion names this subset explicitly. (§17-R10.)

Pi writes one file per session with an append-only writer, but the repository
cannot prove external writer behaviour, so Pi uses the same full recheck Codex
uses — never a weaker claim.

### 9.4 Fork semantics

Per D3 and D4:

- `subagent_relationships: false`, `subagent_models: false`.
- No `ForkOwnership`, no owned-offset scan, no lookbehind.
- The adapter checks parent-link key presence but never reads its path value (D4).
- Fixture 15 places inherited model-change and message rows after the header but timestamps them before child start. It proves inherited data does not contribute child facts.
- A linked child with missing or malformed ownership timestamps fails closed with `AttributionIncomplete`.
- The fixture README states the unsupported expectation and the condition under which it could change: Pi would need a non-path parent identifier **and** a demonstrated replay mechanism.

---

## 10. Unknown-row policy — integration with #229

### 10.1 What #229 governs for Pi

Today the behaviour is strict: an unrecognized row emits an `UnrecognizedType`
observation and an `Unusable(UnrecognizedRecordType)` record
(`claude.rs:163-171`); the first loss reason is recorded
(`evidence_sink.rs:159` → `set_record_loss_reason` at `:843-847`); and
`supported_value` (`evidence_sink.rs:823-841`) then degrades every **supported**
top-level group to `Partial`, leaving unsupported groups `Unsupported`. A
detector with any required group partial increments `eligible` but never
`assessed` (`report.rs:328-346`).

### 10.2 Pi's three-way classification — normative

The final policy adopts #229's recommended strict fail-closed rule for fields
the shared parser can read while #229 remains open for broader shared rollout.

**Row types**

| Class | Rows | Behaviour |
| --- | --- | --- |
| **Semantic** | `message`, `model_change`, `thinking_level_change`, `compaction` | emit normalized events / evidence |
| **Recognized-inert** | `session`, `session_info`; and a `custom` / `custom_message` row **only when it passes the shape test** below | recognized; emit no event; **do not** degrade coverage |
| **Unknown — fail closed** | any other top-level `type`; and any `custom`-family row **failing** the shape test | emit the observed type string per D6; `Unusable(UnrecognizedRecordType)`; degrade |

**The shape test applies to every recognized-inert class — rows *and* roles.**
*(Scope widened in revision 2; it was previously scoped "to one family". See
§17-R30.)*

The shape test examines exactly the top-level object and `/message` object, plus
analysis-bearing block types in their `content` arrays. It fails closed on
usage, model, reasoning or thinking level, tool, role, or compaction signals.
It does not recurse into extension payloads. Nested keys such as
`data.content`, `data.details`, and `data.display` remain inert. A top-level
string `content`, `display`, or `details` field also remains inert because the
shared parser does not read it as an analysis-bearing block array.

**The test is applied per record at runtime, not presumed from the type or role
name.** A `session_info` row that one day starts carrying `usage` must fail
closed on the day it appears, not on the day someone notices.

**Roles** (`message.role`)

| Class | Roles | Behaviour |
| --- | --- | --- |
| **Semantic** | `user`, `assistant`, `toolResult` | as today |
| **Recognized, conditionally inert** | `bashExecution` | recognized and non-degrading **only when the record passes the shape test above**. Its `command`, `output`, `exitCode`, `truncated`, `cancelled` fields are **never** read into evidence. A `bashExecution` record that carries `usage` **fails closed** and degrades, exactly like an unknown role. |
| **Unknown — fail closed** | anything else | observed role string as the discriminator; degrade |

`session_info.name` is transcript-derived (an autoname) and is **never** read.

**Why the conditional matters.** #229's final proposal contains a correction its
author flags as *"changes the design, not just the wording"*:

> the unknown path is reached when there is neither a recognized role *nor* a
> standard turn `type`. A record with `role: "agent"` — present but
> unrecognised — also lands there, and it could carry usage. So the shape test
> cannot be "no role"; a present-but-unrecognised role must fail closed.

Its inertness list reads *"no `role` key at any level (present-but-unrecognised
role ⇒ **not** inert)"*, and its open question 4 calls the corresponding guard
fixture *"non-negotiable"*.

The previous revision declared `bashExecution` flatly non-degrading with no
usage-free proof. Nothing in the machinery would have caught the error:
`parse_usage` (`jsonl.rs:652-689`) reads its four keys off whatever record the
adapter hands it, and if `PiAdapter` emitted neither an event nor an `Unusable`
for such a row, `record_loss_reason` would never be set
(`evidence_sink.rs:159`, `:843-847`), so `supported_value` (`:823-841`) would
return `Complete` for every supported group. Downstream, `badges.rs:78-84` on
`origin/main` turns `Observation::NoFinding` + `coverage == Complete` into
`BadgeStatus::Clean`. That is a **false clean on unread usage** — precisely the
FR-14 violation #229 exists to prevent.

`bashExecution` is an explicitly recognized Pi housekeeping role, but the same
shape test still rejects usage or other evidence-bearing fields. Fixture 21b
pins that fail-closed path.

Pi therefore adopts #229's recommended structural policy now. Any later shared
DTO or report work remains in #229 and must not create a Pi-only interpretation.

### 10.3 `customType` privacy

The adapter checks only the closed row discriminator `custom` or
`custom_message`. It never reads or persists `customType`. Unsafe custom rows
use that closed row discriminator through the existing bounded
`UnrecognizedType` path. Ordinary structurally inert custom rows emit no
unrecognized discriminator.

---

## 11. Changelog requirements

**VERIFIED** — both `[Unreleased]` sections are currently empty.

**VERIFIED** — the #243 branch diff (`git diff --stat origin/main...HEAD`)
touches **neither** changelog. **Do not inherit that omission.** #244's own
acceptance criteria also omit changelogs; §15 adds them.

### `crates/antiburn-local/CHANGELOG.md` — written across **slices 1, 4, and 6**

Audience: a crate consumer (file header lines 8–13).
`.github/workflows/release-engine.yml` reads the tagged section and refuses the
release if it is absent.

**Timing corrected again in revision 2** (§17-R29). Revision 1 put the whole
entry in slice 6, but the export seam means **public API lands in slice 1**:
`pub use vendors::pi::PiAdapter;` is a semver surface from that commit. The
`[Unreleased]` section is therefore appended to three times, and every one of
those commits is individually releasable.

| Slice | Entry |
| --- | --- |
| 1 | **Added** — `PiAdapter` is exported from `analysis`. |
| 4 | **Added** — `SourceCapabilities::pi()`. |
| 6 | **Changed** — `adapter_for("pi")` now returns a dedicated adapter and `has_dedicated_adapter("pi")` is now `true`. That is a behaviour change on public functions, and it transitively changes `supports_analysis` for every consumer. |

If a release is cut between slices, the section already present is correct for
what shipped. That is the point of splitting it.

The deferred crate entry must not claim a global revision change.

### Root `CHANGELOG.md` — lands in **slice 8**

Audience: a person about to install (file header lines 12–18).
`.github/workflows/release-app.yml` reads the tagged section and **fails the
release if it is absent**.

Pi promotion is user-visible: an agent the app currently reports as unsupported
starts producing analysis and insights. Requires an `### Added` entry under
`[Unreleased]`, written as user impact.

The final entry can state that Pi gains dedicated local session analysis and
insights on supported platforms.

---

## 12. Verification

**VERIFIED** — there is no root `Cargo.toml`. `apps/desktop/src-tauri/Cargo.toml`
and `crates/antiburn-local/Cargo.toml` are **two separate cargo workspaces**.
#244's verification block collapses them into one and is therefore incomplete.

### 12.1 Focused loop (while iterating)

```sh
# engine
cd crates/antiburn-local
cargo test --locked --test pi_characterization
cargo test --locked --lib analysis::tests::pi_
cargo test --locked --lib analysis::vendors::tests
cargo test --locked --lib analysis::evidence
cargo test --locked --lib discovery::tests          # only if source_version.rs is touched

# regenerate the four goldens deliberately, never blindly
UPDATE_GOLDENS=1 cargo test --locked --test pi_characterization

# shell
cd apps/desktop/src-tauri
cargo test --locked agents::tests::
cargo test --locked insights_worker::
cargo test --locked insights_report::
cargo test --locked store::tests::                  # V13 and reconciliation

# frontend
pnpm --filter @antiburn/desktop test -- agents
```

### 12.2 Full verification (before every commit that closes a slice)

```sh
# engine workspace
cd crates/antiburn-local
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked            # nextest equivalent: cargo nextest run --locked --lib --tests

# shell workspace  — MISSING from the issue's verification block
cd apps/desktop/src-tauri
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked

# desktop frontend
pnpm --filter @antiburn/desktop format      # prettier --check .   (MISSING from the issue)
pnpm --filter @antiburn/desktop lint
pnpm --filter @antiburn/desktop type-check
pnpm --filter @antiburn/desktop knip        # MISSING from the issue
pnpm --filter @antiburn/desktop test
pnpm --filter @antiburn/desktop build

# repository gates  — MISSING from the issue, required by CONTRIBUTING.md:71
pnpm run slop:all
pnpm run secrets

# only if dependencies changed
pnpm run notices:check

# only if a style file unexpectedly enters the diff
node scripts/check-design-drift.mjs
```

`CONTRIBUTING.md` is the authority for this list, not the issue.

---

## 13. Privacy and performance review

### 13.1 Privacy review (before the fixture commit and before the final commit)

1. **Fixture diff read.** Every staged fixture line, against §4.1.
2. **Golden read.** Confirm no golden contains a string that did not originate in a fixture the reviewer wrote.
3. **Leak-path test (17).** Over every fixture, explicitly including `image_block`, `bash_execution_role`, `custom_rows`, and `compaction_and_inert`. Assert the serialized evidence contains none of:

   block text · thinking text · tool arguments · tool output · shell `command` · shell `output` · `exitCode` · compaction `summary` · `compaction.firstKeptEntryId` · base64 `image.data` · `mimeType` · file paths · `cwd` · session `id` · message `id` · `parentId` · `toolCallId` · `parentSession` · `customType` values · `session_info.name` · `textSignature` · `thinkingSignature` · `thoughtSignature` · `responseId` · `errorMessage` · `provider` · `api`

   *(list expanded in revision 1 — §17-R17)*
4. **Diagnostics audit.** Grep the new adapter for every `format!`, `bail!`, `context`, `tracing::`, and `Debug` derive that could reach a log or an error string. Errors must carry bounded structural diagnostics — a record index and a reason kind — never a record body. `insights_worker`'s `errors_carry_no_transcript_content` must pass with Pi sources.
5. **Secretlint.** `pnpm run secrets`.
6. **Parent-link check.** The adapter may test key presence for D3. It must never call a value getter, copy, format, hash, or persist the path value.

### 13.2 Performance review

Constraint (`CONTRIBUTING.md`): reads, allocations, concurrency, retained data,
CPU work, and disk I/O stay bounded by the visible feature's needs. Antiburn is
an always-running tray application.

**Harness reality, corrected in revision 1** (§17-R24). None of the three
integration tests the previous version named is a Pi-usable baseline:

| Harness | What it actually is |
| --- | --- |
| `tests/streaming_metrics_memory.rs` | asserts `accumulator.retained_bytes()` on a **Claude-only** synthetic corpus (`:27,32`). Not peak heap. |
| `tests/pipeline_corpus.rs` | its module doc states it *"asserts outcome shape … never timing"*. Claude-only (`:35,63`). |
| `tests/source_validity_timing.rs` | `ClaudeAdapter`-only throughout. |
| `benches/memory_baseline.rs` | where `measure_peak` lives. Claude-only. |
| `benches/pipeline_baseline.rs` | throughput. Claude-only. |
| `benches/BASELINE.md` | states the numbers are *"not CI-enforced thresholds"*. |

There is **no per-vendor baseline** and no seam to produce one without new
harness code. So:

- Drop "no regression against the existing baseline" — it is not achievable.
- Either add Pi arms to `benches/memory_baseline.rs` and `benches/pipeline_baseline.rs` (listed in §7.1, with their review cost accounted), **or** take the measurements as one-offs and report the numbers in the PR body.

Measurements to take and report as numbers:

| Measurement | Where | Reported as |
| --- | --- | --- |
| Peak heap on the largest Pi fixture tier | `benches/memory_baseline.rs`, Pi arm | absolute number, compared to the Claude number for context |
| Retained bytes per record | `tests/streaming_metrics_memory.rs`, Pi arm | absolute bound |
| Throughput on a Pi corpus pass | `benches/pipeline_baseline.rs`, Pi arm | absolute number |
| Backfill enrolment time | slice 6 (reconcile) or slice 7 (migration), synthetic database | absolute number |
| Worker drain time and app responsiveness during backfill | manual | drain time plus a qualitative note |
| **Scan-pass behaviour across slice 6** | manual, slice 6 | confirm `scan.rs:930-932` skips Pi **before** `:943` is reached, so no synchronous whole-transcript read is introduced |
| **Report bucket transition** | `insights_report` test, slice 6 | Pi sessions move `awaiting_provider_support` → `pending` → `ready`/`stale`; state the expected transient coverage dip |

Two Pi-specific hazards:

- **`MAX_RECORD_BYTES = 8 MiB`** (`framing.rs:8`). Aggregate observation puts the largest observed Pi line **below** the cap, so the oversized path will rarely fire in the field — and a multi-megabyte `image` line **will** be fully buffered and JSON-parsed. That is the peak-heap risk. Measure it; do not assume the cap protects against it.
- **Backfill volume.** A local Pi corpus can be large. `reconcile_evidence_revisions` enrols them all in one statement at startup. The insert is cheap; the worker drain is not. Confirm the permit machinery (`insights_worker::permit_for`, `PermitKind::Source`) bounds concurrency as expected with three cohort members instead of two.

---

## 14. Rollback

| Slice | Rollback | Blast radius after rollback |
| --- | --- | --- |
| 0 | Delete this document. | Zero. |
| 1–5 | `git revert`. `adapter_for("pi")` was never touched, so Pi still resolves to the generic fallback and `supports_analysis(Pi)` stays `false`. | **Zero behavioural change**, but **not zero surface**: slice 1 exports `PiAdapter` and slice 4 exports `SourceCapabilities::pi()`. Both are semver surfaces on the crate (§8 export seam). If either slice has been included in a published `antiburn-local-v*` tag, reverting is a **breaking change for crate consumers** and needs a major/minor decision plus a `### Removed` changelog entry. Within an unreleased series, revert freely. *(Corrected in revision 2 — §17-R29.)* |
| 6 (promotion) | `git revert` the single commit. Pi returns to the generic adapter; the DTO reverts to `supportsAnalysis: false`; Pi leaves the cohort. | **Not zero — this is the real kill switch.** Any `session_evidence` row already written for Pi becomes orphaned and inert. V13 is one-way, so rollback needs a forward migration if those rows must be removed. |
| 7 (V13 migration) | **One-way.** `user_version` only advances and `schema.rs:8` forbids editing a shipped entry. A defect needs a **forward** V14 migration. | V13 preserves existing evidence with `ON CONFLICT DO NOTHING`. |
| 8 (docs, root changelog) | `git revert`. | Zero. Documentation only. |

---

## 15. Acceptance criteria

From #244, plus the additions this plan's evidence requires. Additions are
marked **(+)**.

**Preconditions**

- [ ] **(+)** `gh pr view 243` reports `MERGED`; the merge SHA is recorded in §16 (required before slice 4).
- [x] **(+)** Pi adopts #229's recommended strict structural policy while #229 owns the broader shared rollout.
- [x] **(+)** Shared revisions stay at `3/5/1/2`; V13 enrollment does not globally stale Claude or Codex.
- [x] **(+)** G1, G2, and G3 are resolved by the aggregate structural measurements in §4.4.
- [x] **(+)** V13 remains the next migration after #243 and its preservation test passes.

**Engine**

- [ ] `adapter_for("pi")` resolves to `PiAdapter`; `has_dedicated_adapter("pi")` is `true`; `"pi"` has moved between the two registry tests.
- [ ] Pi metrics retain parity — `analysis/tests.rs:333` and `:416` pass **unchanged**, and `:426`'s loose `tokens_in > 0` is **not** tightened.
- [ ] **(+)** Cache-rehydration event counts are identical across the registry flip for every fixture whose D12 flag is `true` (§9.1 constraint 4). **The guard test is authored in slice 5**, running each fixture through both `GenericJsonlAdapter` and `PiAdapter`, because slice 6 is atomic and leaves no commit at which the comparison could otherwise be made.
- [x] **(+)** `PiAdapter` is the only new vendor-specific public export; the module stays crate-visible and vendor constants are absent.
- [ ] **(+)** Slices 1–5 assert through `PiAdapter` **directly**, never through `adapter_for("pi")` or `normalize_source`, both of which route to the generic adapter until slice 6.
- [ ] **(+)** Header-less inputs produce full metrics, `started_at_ms: None`, and no coverage degradation (§9.1 constraint 5, fixture 24).
- [ ] `visit` and `visit_claimed` use bounded framing and return correct source-validity outcomes — specifically `Unvalidated`, `AcceptedFull`, `SourceChanged(_)`, and `bail!` on cancellation. **`AcceptedPrefix` is out of scope** because `append_only_guarantee` returns `Absent` for every agent (§9.3).
- [ ] `SourceCapabilities::pi()` is documented and test-pinned field by field, **and** the emitted `cache_write_tokens` equals the per-session summary flag (D12).
- [ ] **(+)** All eight `SessionSummary` fields are set explicitly by `PiStreamState::finish`, including `started_at_ms`, `coverage_gaps`, `initial_context: None`, and `skill_descriptions: empty`.
- [ ] **(+)** `context_window` stays `None`; compaction maps `pre` only, with no dedupe; `message.model` beats `model_change` on disagreement (D14).
- [ ] **(+)** A frozen detector-eligibility table pins which detectors Pi's capability set satisfies.
- [ ] **(+)** Streaming ⇄ batch parity is asserted for every fixture.
- [ ] **(+)** A test proves `reasoning` and `cacheWrite1h` are not summed as additional buckets (D5), and occupancy uses the three-term `context_tokens()`.
- [ ] Unknown rows and content emit bounded observed type strings; unsafe custom rows emit only `custom` or `custom_message`, never a `customType` value (D6).
- [ ] **(+)** Ordinary nested extension payloads stay complete. Top-level or `/message` usage, model, tool, reasoning, role, or compaction signals fail closed. `bashExecution` remains complete only without such signals.
- [ ] **(+)** The §10.2 shape test is applied **per record at runtime** to every recognized-inert class — rows and roles — never presumed from a type or role name.
- [ ] **(+)** §4.4's re-measurement records which roles and row types carry a `usage` object, so `bashExecution`'s inertness is a finding rather than a presumption.
- [ ] Truncated, malformed, oversized, cancelled, and changed-source cases produce honest bounded outcomes without leaking content.
- [ ] Model transitions, tool calls and results, thinking, usage classes, and compaction are characterized; no retroactive model or level assignment.
- [ ] Fork ownership uses top-level timestamps against the child header, excludes inherited rows explicitly, and fails closed when ownership timestamps are unusable (D3).
- [ ] **(+)** `image`, `bashExecution`, `custom_message`, mixed-`api`, timestamp-disagreement, and `started_at_ms` fixtures exist and pass.

**Desktop**

- [ ] Pi is routed through the evidence worker; `capabilities_for_vendor("pi")` returns `Some`.
- [ ] Pi is in `evidence_cohort()`; the cohort test pins three slugs.
- [ ] **(+)** `agents.rs:87-98 generic_fallback_agents_report_no_dedicated_adapter` no longer lists `AgentKind::Pi`.
- [ ] **(+)** The registry flip, `capabilities_for_vendor`, `evidence_cohort()`, the four test edits, and `agents.ts` all land in **one commit** (§8 slice 6).
- [ ] **(+)** The worker never defaults an unknown slug to Claude, re-asserted with Pi present.
- [ ] Existing Pi sessions are enqueued — by `reconcile_evidence_revisions` at startup (D13), and optionally by the slice-7 migration. If the migration is taken, a pre-existing `ready` or `failed` Pi row is **not** reset.
- [ ] **(+)** `errors_carry_no_transcript_content` passes with Pi sources.
- [ ] **(+)** A test pins the `awaiting_provider_support` → `pending` bucket transition for a Pi session across cohort membership.

**Surfacing and docs**

- [ ] The DTO support state agrees with `has_dedicated_adapter("pi")` — automatic, since `supports_analysis` is derived (§8).
- [ ] Pi evidence appears in the pane and report without a generic unsupported empty state.
- [ ] After rebasing onto `origin/main`, Pi report and badge coverage matches #242 with no copied reducer or Pi-only coverage semantics.
- [ ] **(+)** `docs/support.md`: option (a) or (b) from §7.3 is chosen and recorded; the WSL column is unchanged and no Windows support is claimed.
- [ ] **(+)** `crates/antiburn-local/CHANGELOG.md` `[Unreleased]` has `### Added` and `### Changed` entries, **in the slice-6 commit**.
- [ ] **(+)** Root `CHANGELOG.md` `[Unreleased]` has an `### Added` entry, in slice 8.

**Privacy and performance**

- [ ] Every committed fixture is minimal synthetic; the fixture `README.md` states provenance honestly and names no version, file, or count that fingerprints a local source.
- [ ] **(+)** §6.3 steps 1–2 ran **in each slice that staged a fixture** (1, 2, 3), not once at the end. No fixture reached `main` before its own privacy review.
- [ ] No private `~/.pi` content, hash, exact size, or exact count appears in the repository, this plan, the PR body, or any comment.
- [ ] The privacy invariant test covers every fixture and the full §13.1(3) list.
- [ ] `pnpm run secrets` and a manual staged-diff read are both complete.
- [ ] **(+)** Peak heap, retained bytes, throughput, backfill enrolment time, scan-pass behaviour, and the report bucket transition are measured and stated as **absolute numbers** in the PR body. No "no regression" claim is made against a baseline that does not exist.

**Verification**

- [x] Both cargo workspaces pass `fmt --check`, `clippy --all-targets --locked -- -D warnings`, and `test --locked`.
- [x] `pnpm --filter @antiburn/desktop` `format`, `lint`, `type-check`, `knip`, `test`, `build` all pass.
- [x] `pnpm run slop:all` and `pnpm run secrets` pass; `notices:check` was not required because dependencies did not change.
- [ ] Every commit carries a DCO sign-off (`git commit -s`).

### Gate summary

| Gate | Question | Blocks | Default if unresolved |
| --- | --- | --- | --- |
| **G0** | Is #243 merged? What is the baseline SHA? | slices 4–8 (1–3 may proceed) | hard block on slice 4 |
| **G1** | Which timestamp is authoritative? | resolved | top-level row `timestamp` (D2) |
| **G2** | Does the header carry `parentSession`? | resolved | inspect key presence only; never read the value (D4) |
| **G3** | Does `parentId` prove depth? | resolved | no; `thread_identity: false` |
| **G4** | Which #229 policy applies? | resolved for Pi | strict narrow structural policy; broader rollout remains #229 |
| **G5** | Do global revisions move? | resolved | no; preserve `3/5/1/2` |
| **G7** | Which migration enrolls Pi? | resolved | V13; reconciliation remains the recovery path |

*(The former G3b is retired; D12 replaces it — see §17-R4.)*

---

## 16. Progress and decision log

Implementers append to this table. One row per gate answer, decision change, or
surprise.

| Date | Slice | Entry | Evidence |
| --- | --- | --- | --- |
| — | 0 | Plan authored against `679a5c9`. #243 OPEN, #229 OPEN, #242 merged to `main` but absent from this branch. | §1.1 |
| — | 0 | Revision 1: first adversarial review applied. 26 findings accepted, 2 rebutted. Former G3b retired into D12; slice ordering rebuilt around the registry flip. | §17 |
| — | 0 | Revision 2: second review applied. 13 findings accepted, 0 rebutted, 4 areas confirmed PASS and preserved. Export seam added (slices 1–5 could not have compiled); `bashExecution` narrowed to a runtime shape test per #229's fail-closed correction; six slice/fixture assignment defects fixed. | §18 |
| 2026-08-27 | 0 | G0 waived for worktree implementation only: PR #243 remains open at `679a5c9662e5a12e86319deb0a6a3c3be896b667`. Acceptance remains blocked until it merges. | `gh pr view 243`; task directive |
| 2026-08-27 | final repair | Aggregate structural measurements resolve G1–G3: use top-level timestamps, detect parent-link presence only, and keep `thread_identity: false`. | §4.4; fork and ordering fixtures |
| 2026-08-27 | final repair | Adopted #229's recommended narrow structural policy. Ordinary nested extension payloads stay complete; evidence-bearing parser shapes fail closed. | `custom_rows`; `inert_signal_guard`; `bash_execution_with_usage` |
| 2026-08-27 | final repair | Fork ownership compares each subsequent top-level timestamp with the header and explicitly observes inherited rows. Missing ownership timestamps fail closed. | D3; fork fixtures |
| 2026-08-27 | final repair | Restored shared revisions to `3/5/1/2`, removed out-of-scope shared metric changes, and retained V13 Pi enrollment. | revision and migration tests |
| 2026-08-27 | final repair | Removed the copied #242 badge reducer. Badge integration waits for the rebase onto `origin/main`. | stacking disposition |
| 2026-08-27 | final repair | Removed the Pi total record limit. Pi follows the shared per-record framing and retained-metrics contract. | Locked Decisions 9 and 14 |
| 2026-08-27 | final integration | Rebased the #243 stack onto merged #242, verified Pi against the real badge reducer, and updated both changelogs. | Pi badge characterization; root and engine `[Unreleased]` entries |

### Implementation command ledger

| Command | Result |
| --- | --- |
| `cargo test --locked --manifest-path crates/antiburn-local/Cargo.toml --lib analysis::vendors::tests` | PASS — 3 tests |
| `cargo test --locked --manifest-path crates/antiburn-local/Cargo.toml --lib analysis::tests::pi_` | PASS — 2 unchanged parity tests |
| `cargo test --locked --manifest-path crates/antiburn-local/Cargo.toml --test pi_characterization` | PASS — 22 tests |
| `cargo test --locked --manifest-path apps/desktop/src-tauri/Cargo.toml agents::tests::` | PASS — 5 tests |
| Focused shell routing, backfill, report transition, and worker capability tests | PASS — 1 test each |
| `pnpm --filter @antiburn/desktop test -- agents` | PASS — 87 files, 890 tests |
| Engine `cargo fmt --check` and `cargo clippy --all-targets --locked -- -D warnings` | PASS |
| Engine `cargo test --locked` | PASS — 882 unit tests, all integration suites, and doc tests; one pre-existing ignored test |
| Shell `cargo fmt --check` and `cargo clippy --all-targets --locked -- -D warnings` | PASS |
| Shell `cargo test --locked` | PASS — 567 tests |
| Desktop `format`, `lint`, `type-check`, `knip`, `test`, and `build` | PASS — 87 test files and 890 tests; build emitted only the existing chunk-size advisory |
| `pnpm run slop:all` | PASS — score 100, no diagnostics |
| `pnpm run secrets` | PASS |

### Repair round 1 command ledger

| Command | Result |
| --- | --- |
| `cargo test --locked --test pi_characterization` | PASS — 24 tests |
| Focused inert-shape and tool-result parser tests | PASS — 1 test each |
| Focused V13 migration test | PASS — 1 test |
| Focused Pi worker → persistence → report test | PASS — 1 test |
| Engine `cargo fmt --check`, Clippy with `-D warnings`, and `cargo test --locked` | PASS — 883 unit tests, all integration suites, and doc tests; one pre-existing ignored test |
| Shell `cargo fmt --check`, Clippy with `-D warnings`, and `cargo test --locked` | PASS — 569 tests |
| Desktop `format`, `lint`, `type-check`, `knip`, `test`, and `build` | PASS — 87 test files and 891 tests; build emitted only the existing chunk-size advisory |
| `pnpm run slop:all` and `pnpm run secrets` | PASS — score 100 and no secret findings |
| `git diff --check` | PASS |

### Repair round 2 command ledger

| Command | Result |
| --- | --- |
| `cargo test --locked --test pi_characterization` | PASS — 26 tests |
| Engine `cargo fmt --check`, Clippy with `-D warnings`, and `cargo test --locked` | PASS — 883 unit tests, all integration suites, and doc tests; one pre-existing ignored test |
| Shell `cargo fmt --check`, Clippy with `-D warnings`, and `cargo test --locked` | PASS — 569 tests |
| Frontend full command rerun from the second verification ledger | PASS — format, lint, type-check, knip, 891 tests, and build |
| Repository gates after final edits | PASS — slop score 100, secrets clean, and `git diff --check` clean |
| Conditional dependency and design gates | SKIP — no dependency, lockfile, style, or design-contract changes |

### Final integration command ledger

| Command | Result |
| --- | --- |
| `git range-diff e5fcf8a..679a5c9 8156a6e..HEAD` | PASS — all four #243 patches remained equivalent after rebase |
| Pi badge characterization against merged #242 | PASS — supported reasoning badge is clean; unsupported badges stay capability-missing; partial evidence never reads clean |
| Engine `cargo fmt --check`, Clippy with `-D warnings`, and `cargo test --locked` | PASS — 884 unit tests, all integration suites including 26 Pi tests, and doc tests; one pre-existing ignored test |
| Shell `cargo fmt --check`, Clippy with `-D warnings`, and `cargo test --locked` | PASS — 575 tests |
| Desktop `lint`, `type-check`, `test`, and `build` | PASS — 88 test files and 901 tests; build emitted only the existing chunk-size advisory |
| `pnpm run slop:all`, `pnpm run secrets`, and `git diff --check` | PASS — score 100, no secret findings, and a clean diff check |

### Synthetic performance observations

- Pi has no total record limit. It retains metric-bearing records under the
  same contract as existing adapters while bounded JSONL framing limits each
  raw record.
- The focused one-row synthetic backfill enrollment test completed below the
  test harness's 0.01-second reporting resolution. Reconciliation uses one SQL
  insert, and the worker keeps one CPU permit and one file-source permit.
- `scan::tests::top_up_analysis_skips_the_evidence_cohort` passed with Pi in the
  cohort, so promotion adds no synchronous whole-transcript scan pass.

### Template for a gate resolution

```
| YYYY-MM-DD | 0 | G3 resolved: thread_identity = <true|false>.
  Measurement: <bucketed fraction of sampled sessions whose parentId graph branches>,
  method per §4.3, aggregate only.
  Detector consequence: <detectors unlocked or not>.
  Maintainer: <who agreed>. | <file:line or comment URL> |
```

### Notes for whoever picks this up

1. **Re-derive every line number in §7 first.** They are from an unmerged branch head.
2. **Read §8's export seam before writing any test.** `PiAdapter` is unreachable from the integration-test crate unless it is exported, and `static PI` must not exist before slice 6 or `clippy -D warnings` fails. Slices 1–5 call `PiAdapter` directly; `adapter_for("pi")` and `normalize_source` both go to the generic adapter until slice 6.
3. **`bashExecution` is inert only when a runtime shape test says so.** Never presume it from the role name. #229 calls the usage-carrying guard fixture non-negotiable; it is fixture 21b.
4. **Slice 6 is the switch, and it is atomic.** `supports_analysis` is derived from `adapter_for`; there is no second flag. Flipping `agents.ts` alone changes nothing, because `SessionPane.tsx:347` prefers the DTO.
5. **Do not enrol Pi in the cohort without `capabilities_for_vendor`.** Keep the promotion edits atomic.
6. **V13 is one-way.** Use a forward migration for any later repair. Reconciliation remains idempotent.
7. **When a measurement is inconclusive, take the `false` branch** — for `thread_identity`. But **not** for `cache_write_tokens`: D12 shows `false` there is a metrics change *and* synthesizes inferred events, so `false` is the less honest answer.
8. **Write the §9.1 constraint 4 guard in slice 5, not later.** Slice 6 is atomic; after it lands there is no commit at which the before/after metrics comparison can be made.
9. **The two pinned Pi tests at `analysis/tests.rs:333` and `:416` are the tripwire.** If either changes, the promotion altered behaviour that was already correct. Stop and find out why.
10. **Keep this document authoritative.** The former sibling plan was removed before implementation. Record later decision changes here.

---

## 17. Review log — revision 1

Every finding from the first adversarial review, with the verification performed
and the disposition. **Accepted** findings are reflected in the body above.
**Rebutted** findings are argued here.

### Accepted

**R1 — `Streamability` is dead metadata; the "scope gap" claim was false.**
*Verified*: repository-wide grep returns only `discovery/mod.rs:44` (re-export)
and `discovery/tests.rs:86`. Nothing in `apps/desktop/src-tauri` reads it.
*Fixed*: §2.6's bolded scope-gap paragraph deleted and replaced with the
verified reader list; the change demoted to optional/cosmetic in §7.1; the
acceptance criterion removed. **Additionally**, the review's second half is also
correct and was missed by the first version: `discovery/tests.rs:60-86` builds a
`SessionLog { agent_type: AgentKind::Pi }` and asserts `WholeDocumentFallback`.
That test is now listed in §7.1 as a required companion edit.

**R2 — slice 5, not slice 9, was the user-visible switch.**
*Verified*: `agents.rs:37-39` derives `supports_analysis` from
`has_dedicated_adapter`; `analysis.rs:1223-1225` → `commands.rs:850` → DTO;
`SessionPane.tsx:347` prefers the DTO over the registry; `scan.rs:943` gates
`top_up_analysis` on it while the cohort skip sits earlier at `:930-932`;
`commands.rs:618,627` gate detail caching on cohort membership.
*Fixed*: §8 rewritten. Slices collapsed to 8, with an atomic **slice 6
promotion commit** containing the registry flip, `capabilities_for_vendor`,
`evidence_cohort()`, all four mirror tests, `agents.ts`, and the engine
changelog. §14 rewritten — slices 1–5 are genuinely zero, slice 6 is the kill
switch. The scan-pass hazard is now a §13.2 measurement.

**R3 — `agents.rs:87-98` was missing from the change map.**
*Verified*: `generic_fallback_agents_report_no_dedicated_adapter` lists
`AgentKind::Pi` and asserts `!supports_analysis(kind)`. It fails at the registry
flip. *Fixed*: added to §2.6, §7.2, and §15.

**R4 — `cache_write_tokens` is overwritten per session; G3b's `false` default was
wrong twice over.**
*Verified*: `evidence_sink.rs:571-572` assigns
`self.capabilities.cache_write_tokens = summary.cache_write_tokens_available`.
`metrics_sink.rs:420` gates the direct rehydration test on the flag and `:438`
returns early on it, so `false` discards those events and runs an inferred
`windows(3)` heuristic instead. `generic_jsonl.rs:30` gives Pi `true` today.
*Fixed*: **GATE G3b retired**; replaced by **DECISION D12** (declare `true`,
compute the per-session flag in `finish`). §5.3 test 1 strengthened to assert the
emitted value against the summary flag. New §9.1 constraint 4 pins
cache-rehydration event parity across the flip. §5.2's stakes recomputed — G3
now carries both extra detectors alone.

**R5 — the `custom` family and `bashExecution` were mishandled.**
*Verified*: `custom_message` carries `content`/`display`/`details` (§4.5); an
unrecognized role fails closed today. *Fixed*: §10.2 replaced with a normative
three-way table for row types **and** roles, including a shape test that admits
a `custom` row as inert only when it carries no role, usage, model, level, tool
call, tool result, or thinking block. `bashExecution` is explicitly recognized
and non-degrading. The final repair narrows the test to shared parser locations,
so ordinary extension payloads remain complete while direct signals degrade.

**R7 — D3 misreported the fork evidence and fixture 15 was toothless.**
*Verified*: the two passes do not agree; one reports a small nonzero shared-row
rate. *Fixed*: D3 rewritten to state the anomaly honestly and to name the
invariant actually relied on — **ownership is per file, no cross-file
subtraction**. Fixture 15 rewritten as the hazard case (child with duplicated
leading rows).

**R8 — D6 was unimplementable and created Pi-only semantics.**
*Verified*: `evidence_sink.rs:1050-1058` pins Claude's `unrecognized_types` to
the observed type string (`"telemetry_ping"`). A placeholder would give the same
persisted field a different meaning per vendor — the Pi-only interpretation #244
forbids. *Fixed*: D6 rewritten to four numbered rules — observed type string via
`cap_string` for unknown rows and blocks, row type only for the `custom` family,
never a `customType` value. The final repair removes any persisted count and
needs no waiver. Fixture 8's expectation remains corrected.

**R9 — the oversized fixture cannot be a committed file.**
*Verified*: `framing.rs:21-23` hard-codes `MAX_RECORD_BYTES`;
`with_max_record_bytes` at `:25` is not reachable through a production entry
point; Codex has no oversized fixture. *Fixed*: new §6.2 — runtime-generated
temp file via `RawSource::File`, excluded from the all-fixture loops and the
golden set, with an explicit "or delete the scenario" fallback.

**R10 — `AcceptedPrefix` is unreachable and the "shared harness" does not exist.**
*Verified*: `source_validity.rs:42-44` returns `Absent` for every agent; the
Codex suite has exactly two validity tests (`:462`, `:500`) plus a
`source_claim()` helper at `:485`; `visit` hard-codes `cancel = &|| false`
(`codex.rs:75,80`). *Fixed*: §9.3 rewritten to the reachable set only. §6.1 row
14 split into 14a–14d with an explicit **Src** column marking copies versus new
harness work, and 14d noting cancellation is `visit_claimed`-only. §15's
criterion names the subset. The phrase "where the shared harness supports a
case" is gone.

**R11 — the migration is redundant; D1 leaves terminal `unsupported` rows
unrecoverable; §14's slice-7 rollback was wrong.**
*Verified*: `store/mod.rs:682-712` `reconcile_evidence_revisions` enrols every
missing row for cohort agents; `lib.rs:223` runs it at startup. The requeue
`UPDATE` at `:719-753` fires only on a generation or revision change.
`analysis.rs:540-542` → `ParentUnsupported` → `insights_worker.rs:69,270` is the
terminal path. *Fixed*: new **DECISION D13** (migration optional, immediacy
only, default skip); D1 gained an explicit consequence clause requiring the
atomic slice 6 and a forward `DELETE ... WHERE agent='pi' AND
status='unsupported'` on any rollback; §14 row 6 rewritten to name the trap;
The migration decision is tracked as G7.

**R12 — two competing untracked plan documents.**
*Verified*: `git status` shows both. *Fixed*: `docs/plans/244-pi-provider-plan.md`
is now cited by path in the header and in §4.4, and §16 note 7 requires
reconciling to exactly one document before implementation.

**R13 — `store/tests.rs:1987` has no Pi expectation.**
*Verified*: the test uses `cursor.key.agent = "cursor"` (`:1989-1990`); there is
no `"pi"` literal anywhere in `store/tests.rs`. *Fixed*: the §7.2 row deleted,
with the refutation recorded inline so a later reader does not re-add it.

**R14 — `tokens_in > 0` was omitted from the parity list.**
*Verified*: `analysis/tests.rs:424-426`. *Fixed*: added to §2.4 and §9.1
constraint 1 with an explicit instruction not to tighten it.

**R15 — neither pinned Pi fixture has a `session` header.**
*Verified*: `analysis/tests.rs:334-343` and `:417` are bare `type:"message"`
rows. *Fixed*: recorded in §2.4; new §9.1 constraint 5 requires full metrics,
`started_at_ms: None`, and no degradation from header-less content; slice 1's
exit criteria reference it; fixture 24 pins it.

**R16 — `docs/support.md` has no analysis column.**
*Verified*: `:25-37` columns are Agent / Native / WSL / Notes; `:39-42` names no
providers. *Fixed*: §7.3 now names the two honest options (a) do nothing or (b)
a Notes-column addition, forbids the un-expressible edit, and requires the choice
to be recorded.

**R17 — the leak list was incomplete.**
*Accepted in full.* §13.1(3) expanded with `textSignature`,
`thinkingSignature`, `thoughtSignature`, `responseId`, `errorMessage`,
`provider`, `api`, `session_info.name`, `compaction.summary`,
`compaction.firstKeptEntryId`, `parentSession`, `mimeType`, `exitCode`.
`*Signature` also added to §4.1.

**R18 — the plan broke its own privacy rules.**
*Verified*: §4.1 prohibited recording a size or count of a real file, while the
body carried exact denominators and a "near 2 MiB" figure. *Fixed*: every corpus
count in this document is now bucketed or stated as a comparison against a
repository constant; the 2 MiB figure is replaced with "below `MAX_RECORD_BYTES`";
§4.2 gained an explicit carve-out permitting bucketed orders of magnitude and
constant-relative comparisons, which reconciles the prohibition with §13.2's
sizing need; §0 states the convention.

**R19 — §4.3 made G2 and G3 unmeasurable.**
*Verified*: the previous step 2 forbade descending into `id` and `parentId`,
which are exactly what G3 measures. *Fixed*: step 2 rewritten as **read versus
print** — identifier fields may be read in memory, must never be printed;
content-bearing subtrees may be neither read into nor printed from.

**R20 — three adapter mappings were undecided.**
*Verified*: `model.rs:302,306,310` for the compaction fields; `model.rs:266-272`
for per-record `model`; no context-window field exists in Pi. *Fixed*: new
**DECISION D14** covering `context_window: None`, compaction pre-only with no
dedupe, and `message.model` beating `model_change` on disagreement. Fixtures 5
and 7 updated; §15 gained a criterion.

**R21 — goldens embed `cost` and will churn on pricing updates.**
*Accepted.* D10 gained the requirement to exclude `cost` from the golden JSON or
use a stable-priced model.

**R22 — the crate changelog was scheduled too late.**
*Verified*: the public-API change ships in slices 4–6 and slice 5 leaves the
engine independently mergeable; `release-engine.yml` gates on the section.
*Fixed*: §11 splits the two changelogs — the engine entry lands in the slice-6
commit, the root entry in slice 8.

**R23 — the two pinned inputs are never streamed.**
*Accepted.* New fixture 24 copies both inputs into the Pi suite so parity is
asserted on precisely the pinned numbers through the streaming path. They are
already synthetic and already in the repository, so this adds no privacy risk.

**R24 — §13.2 misattributed every perf harness.**
*Verified*: `streaming_metrics_memory.rs:27,32` is Claude-only and measures
`retained_bytes`; `pipeline_corpus.rs` asserts outcome shape, not timing;
`source_validity_timing.rs` is `ClaudeAdapter`-only; `benches/memory_baseline.rs`
and `benches/pipeline_baseline.rs` exist and are Claude-only; `benches/BASELINE.md`
disclaims CI enforcement. *Fixed*: §13.2 rewritten with a harness-reality table,
"no regression against the existing baseline" deleted, absolute-number reporting
required, and Pi harness arms listed in §7.1 with their cost acknowledged.
§15 updated.

**R25 — G0 over-blocked.**
*Accepted.* §1.2 now scopes G0: slices 1–3 add only new files and may proceed
against `679a5c9` with a later rebase; slice 4 onward requires `MERGED`.

**R26 — `pnpm run notices:check` was omitted.**
*Accepted.* Added to §12.2, marked as needed only when dependencies change.

**R27 — the report bucket transition was unasserted.**
*Verified*: `insights_report.rs:28-40` computes `awaiting_provider_support` from
`e.status IS NULL`. *Fixed*: recorded in §2.8; a pinning test added to §7.2 and
§15; the transient coverage dip added to §13.2.

**R28 — `SessionSummary` gained fields in #243 that the adapter surface omitted.**
*Verified*: `interface.rs:90-103` carries `started_at_ms`, `coverage_gaps`,
`late_tools`, `initial_context`, `skill_descriptions`; `interface.rs:158,186-187`
and `evidence_sink.rs:573-575` consume them; `codex.rs:394-416` sets all eight;
`codex_characterization.rs:536` pins `started_at_ms`. *Fixed*: new §2.10; §7.1
gained a dedicated `PiStreamState::finish` row listing all eight fields; D2
extended to cover `started_at_ms`; fixture 23 added; §15 gained a criterion.

### Rebutted

**R6 — "`EvidenceGroup::Context` may be `Unsupported` when `initial_context` is
`None`, so §5.2's `SessionsOverDepth` prediction is untested."**

**Rejected.** The two are unconnected.

`report.rs:97` — `Self::Context => state(&evidence.context)`.
`evidence_sink.rs:672-676` — `context: self.supported_value(context,
self.capabilities.request_context_tokens, self.context_cap_exceeded)`.
`supported_value` (`evidence_sink.rs:823-841`) returns `Unsupported` **only**
when its `supported` argument is `false`. That argument is
`capabilities.request_context_tokens`, which §5.1 sets `true` for Pi.

`initial_context` never reaches an evidence group at all. It is a
`SessionSummary` field consumed by the **metrics** sink (`metrics_sink.rs:151`,
`:160-161`) and surfaced in `SessionMetrics`, not `SessionEvidence`. There is no
code path by which `initial_context: None` can make `EvidenceGroup::Context`
anything other than what `request_context_tokens` and the loss reason dictate.

§5.2's prediction therefore stands as written.

The *adjacent* observation in the same finding — that `initial_context` and
`skill_descriptions` are `SessionSummary` fields the plan's adapter surface
omitted — is correct and is accepted under **R28**, which now requires
`PiStreamState::finish` to set both explicitly (`None` and empty).

**R-alt — "collapse slices 5, 7, and 9 into one commit *or* introduce a gate so
`adapter_for` routing does not imply `analysis_supported`."**

**Second option rejected; first adopted.** Decoupling `analysis_supported` from
`adapter_for` would mean reintroducing the second vendor list that
`vendors/mod.rs:41-45` and #244 §1 explicitly forbid (*"Keep
`has_dedicated_adapter` derived from `adapter_for`; do not introduce another
backend vendor list"*). The atomic promotion commit achieves the same safety
without touching that invariant, and it is small: one match arm, one array
element, one bool, four test arrays, one changelog entry.

---

## 18. Review log — revision 2

Second adversarial pass, run by two independent reviewers. **13 findings
accepted, 0 rebutted, 4 areas confirmed PASS and preserved unchanged.** Every
code claim below was re-verified against `679a5c9` before acceptance.

### Confirmed PASS — preserved as written

These were checked and explicitly passed by the second review. **Do not
re-litigate them.**

| Area | What was verified |
| --- | --- |
| **Migration and revision reasoning** (§2.7, §2.8, D1, D13, G7) | The requeue predicate at `store/mod.rs:697-753` confirms an `unsupported` row is excluded from the analysis-freshness trigger and that only a generation or revision change requeues it — so D1's terminal-`unsupported` consequence is real. D13 is correct: `reconcile_evidence_revisions` enrols cohort agents, making the migration genuinely optional. G7's `MIGRATIONS.len()` rule is right (currently 12 ⇒ V13). §14's rollback `DELETE … agent = 'pi'` uses the correct slug (`AgentKind::Pi.slug() == "pi"`). |
| **Changelog completion** (§11, §15) | Both `[Unreleased]` sections verified empty; both release workflows gate on the tagged section; both entries covered by §15 criteria. *(Slice assignment was still refined — see R29 — but the completeness requirement passed.)* |
| **Acceptance commands** (§12) | §12.2 is a strict superset of `CONTRIBUTING.md:40-72`. Every script exists: `slop:all`, `secrets`, `notices:check` at the root; `format`, `lint`, `type-check`, `knip`, `test`, `build` under the desktop package; `scripts/check-design-drift.mjs`. Every `--lib` filter in §12.1 resolves to a real `mod tests` block. |
| **Private-fixture procedure** (§4.1, §4.2, §4.3) | Internally consistent after revision 1's R18/R19. The read-versus-print rule makes G2 and G3 measurable without opening a leak path. Only its *scheduling* was defective — see R36. |

The first reviewer independently re-verified roughly 45 cited anchors and
confirmed the capability matrix (§5.1), the detector table (§5.2), the
source-validity set (§9.3), the revision blast radius (§2.8), the cohort and
reconcile reasoning (§2.7, D13), and the registry-flip analysis (§8) all match
code exactly, together with every baseline fact in §1.1.

### Accepted — blockers

**R29 — slices 1–5 could not have compiled: `PiAdapter` was unreachable while unrouted.**

*Verified*: `vendors/mod.rs:7-14` makes every adapter module private except
`pub mod claude;`. `analysis/mod.rs:80-81` exports only `ClaudeAdapter`,
`adapter_for`, and `has_dedicated_adapter`. `codex_characterization.rs` is a
separate crate reaching the adapter solely via `adapter_for("codex")`
(`:74,:92,:517`) and `normalize_source`, which itself dispatches through
`adapter_for` (`analysis/mod.rs:217-219`). With `adapter_for("pi") => &GENERIC`,
every fixture in slices 1–3 and 5 would have exercised `GenericJsonlAdapter` and
passed for the wrong reason — the exact failure §5.3 exists to prevent.

*Verified, second half*: an unreferenced `static PI` is `dead_code`, failing both
§12.2's own gate and CI (`.github/workflows/ci.yml:117,:220`). Suppression has no
precedent — the engine contains exactly one `#[allow(...)]`
(`repositories/sessions.rs:466`, `clippy::too_many_arguments`), and `AGENTS.md`
forbids adding dead-code suppressions without explicit maintainer agreement.

*Fixed*: new **export seam** subsection opening §8 — a crate-visible Pi module
plus the `PiAdapter` re-export in slice 1, with `static PI` withheld until slice 6
so no suppression is ever needed. Three knock-ons corrected: §11's engine
changelog is now written across slices 1, 4, and 6 because public API ships in
slice 1; §14's "genuinely zero" for slices 1–5 is downgraded to "zero
behavioural change, non-zero semver surface"; and §6.1 rows 16 and 24 are
re-specified to call `PiAdapter` directly rather than through `normalize_source`.
§7.1 and §15 updated.

**R30 — `bashExecution` was declared non-degrading with no usage-free proof, contradicting #229.**

*Verified* against #229's final-proposal comment, in the passage its author flags
as *"changes the design, not just the wording"*:

> the unknown path is reached when there is neither a recognized role *nor* a
> standard turn `type`. A record with `role: "agent"` — present but
> unrecognised — also lands there, and it could carry usage. So the shape test
> cannot be "no role"; a present-but-unrecognised role must fail closed.

Its inertness list reads *"no `role` key at any level (present-but-unrecognised
role ⇒ **not** inert)"*, and open question 4 calls the guard fixture
*"non-negotiable"*.

*Verified* that nothing downstream would have caught the error:
`parse_usage` (`jsonl.rs:652-689`) reads its four keys off whatever record the
adapter hands it; with no event and no `Unusable` emitted,
`record_loss_reason` is never set (`evidence_sink.rs:159`, `:843-847`), so
`supported_value` (`:823-841`) returns `Complete` for every supported group; and
`badges.rs:78-84` on `origin/main` then turns `NoFinding` + `Complete` into
`BadgeStatus::Clean`. A false clean on unread usage.

*Verified* that G4 does not cover it: G4's default is "ship strict for *unknown*
types", and `bashExecution` was being **recognized**, so it never reached the
strict path.

*Fixed*: §10.2's shape test widened from the `custom` family to **every**
recognized-inert class, rows and roles, applied **per record at runtime** rather
than presumed from a name. `bashExecution` is reclassified *recognized,
conditionally inert*. New fixture **21b** carries #229 open-question 4's guard: a
`bashExecution` row with non-zero `usage` must fail closed. D11's correctness
claim is narrowed to what the shape test can carry. §4.4 gains "which roles and
row types carry a `usage` object" as a mandatory measurement, so the inertness is
a finding rather than a presumption. §15 gains three criteria.

### Accepted — slice and fixture sequencing

**R31 — fixture 15 (`fork_hazard`) was assigned to no slice.** *Verified* by
reading §8: slice 1 took fixtures 1–7, slice 2 took "8–13b, 19–24", slices 3–5
named only 8/9/9b/21, §5.3's tests, and 17. Fixture 15 falls in none of those
ranges, yet D3, §9.4, and a §15 criterion all depend on it. *Fixed*: assigned to
slice 2, which is where `visit` and per-file ownership first exist. §9.4 states
the owning slice inline.

**R32 — slice 2 could not have exited green with the recognition fixtures it was given.**
*Verified*: fixtures 8, 9, 9b, 13b, and 21 all assert D6 discriminators,
`Partial(UnrecognizedRecordType)`, the §10.2 shape test, or the suppression
count — every one of which slice 3 delivers. Slice 3's own exit re-claimed
8/9/9b/21, so the two slices contradicted each other. *Fixed*: those five (plus
new 21b) moved to slice 3. Slice 2 now holds only the framing, ownership,
ordering, and summary fixtures — 10, 11, 12, 13a, 15, 19, 20, 22a, 23, 24, and
14a–14d.

**R33 — fixture 22 (`mixed_api`) spanned slices 2 and 4.** *Verified*: its
expected outcome required both `SessionSummary.cache_write_tokens_available ==
false` (slice 2's `finish`) and the emitted `capabilities.cache_write_tokens ==
false`, which needs `SourceCapabilities::pi()` from slice 4. *Fixed*: split into
**22a** (summary half, slice 2) and **22b** (capability half, slice 4) as
separate §6.1 rows.

**R34 — §9.1 constraint 4 had no owning slice and no named artifact.** *Verified*:
it is the only guard against D12 silently changing existing Pi cache metrics, yet
no slice exit named it, and "before and after the registry flip" is unobservable
because slice 6 is atomic. *Fixed*: assigned to **slice 5**, re-specified as a
single test body that runs each fixture through both `GenericJsonlAdapter` and
`PiAdapter` and compares, and named in slice 5's exit criteria, in §9.1
constraint 4, in §15, and in §16's implementer notes.

**R35 — slice 1's exit criterion cited fixture 24, which slice 2 owns.**
*Verified*: §6.1 and slice 2 both place fixture 24 in slice 2, and its "no
coverage degradation" half additionally depends on slice 3's recognition path.
*Fixed*: slice 1's exit no longer references it; fixture 24 lands whole in slice
2, and its degradation half is re-asserted in slice 3's exit.

**R36 — the §6.3 fixture privacy gate ran after the fixture commits.** *Verified*:
§6.3 said "before the fixture commit", fixtures ship in slices 1–3, but §8
scheduled the review only in slice 5 — every fixture would have reached `main`
two to four commits before its own privacy review. *Fixed*: §6.3 steps 1–2 now
run in **each** slice that stages a fixture (1, 2, 3); step 3 and the full §13.1
sweep stay in slice 5. Each of those three slice exits names the gate, and §15
gains a criterion.

### Accepted — precision and completeness

**R37 — the badge/report divergence was asserted but never tested.** *Verified*:
`badges.rs` on `origin/main` is fully capability-derived (`badge_status:59-71`
reuses `requirements` and `EvidenceGroup::state`), so Pi's badge eligibility
follows from §5.3 test 3 automatically — but `badges.rs:78-84` adds a
**session-wide** condition the report does not apply (`groups_complete &&
evidence.coverage == EvidenceCoverage::Complete`), and the divergence is
deliberately pinned on `main` by
`session_wide_partial_coverage_diverges_from_report_by_design` (`badges.rs:270`).
A Pi session could therefore be report-`Clean` and badge-`NotAssessed`
unasserted. *Fixed*: §5.3 test 3 gains that assertion; §15 gains a criterion.

**R38 — §2.1 mis-cited the vendor-label anchor.** *Verified*: `model/agent.rs:75`
is inside `AgentKind::slug()`, not `vendor_label`. `vendor_label` lives at
`apps/desktop/src-tauri/src/agents.rs:23-30`, where `other => other.slug()` makes
`vendor_label(AgentKind::Pi) == "pi"`. The conclusion §8 leans on is unchanged.
*Fixed*: §2.1 rewritten with both anchors and the `analysis.rs:1113` →
`analysis.rs:521` path stated explicitly.

**R39 — §1.2's G0 scope was understated.** *Verified* at the merge base:
`git show e5fcf8a:…/interface.rs` shows `SessionSummary` with **six** fields —
`started_at_ms` and `coverage_gaps` are #243's additions. Slice 2's "all eight
fields" and D7's `visit_claimed` shape therefore depend on #243's **API
surface**, not merely on files it touched, so slices 2–3 are rework-exposed and
not merely rebase-exposed. *Fixed*: §1.2 states this plainly and offers the
alternative of holding slice 2 until G0 clears.

**R40 — §2.7 cited one reconcile call site; there are two.** *Verified*:
`lib.rs:223` and `lib.rs:670`. *Fixed*: both cited, with a note to check the
second when validating D13's backfill timing.

**R41 — D10/R21's cost-churn concern was overstated.** *Verified*: all three
Codex goldens already serialize `"cost": null`, because no runtime pricing is
installed in the test binary. *Fixed*: D10 keeps the rule — it is free and
removes a latent coupling — but now says plainly that the risk is hypothetical
and not worth review time.

### Rebutted

None. Every finding in the second pass was verified as valid.
