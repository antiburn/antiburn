# Issue #229 — Best-effort assessment when a session contains unrecognized record types

**Issue:** [antiburn#229](https://github.com/antiburn/antiburn/issues/229) (`enhancement`, `area-engine`)
**Branch:** `feat/issue-229-best-effort-unrecognized-records`
**Depends on:** #222 / PR #231 (the thirteen-name housekeeping allowlist) — landed on `main` (`3a39d2d`, `a5aae8c`).
**Verified against:** `main` @ `8156a6e`.
**Adopted option:** **C — structural recognition** (issue comment
[#5436645995](https://github.com/antiburn/antiburn/issues/229#issuecomment-5436645995)), as corrected by the fact-check comment
[#5436628681](https://github.com/antiburn/antiburn/issues/229#issuecomment-5436628681).

This document is the durable implementation plan. It resolves every open design
question against the code on `main`, states the policy and its fail-closed
boundary, and lists the ordered work. **No production code is written by this
plan.** Symbols are cited by name, not by line number, because line references
in the issue thread are already drifting. §15 is the plan review record: what
was reviewed, which corrections were accepted, which were rejected and why.

---

## 1. Current behavior (evidence)

### 1.1 The unknown path

`ClaudeAdapter::visit_reader` (`crates/antiburn-local/src/analysis/vendors/claude.rs`)
frames one record, runs `state.context.observe(&value)` (initial-context pass),
emits every `evidence_observations(&value)` result, collects skill markers, and
then calls `parse_record(&value)`. When `parse_record` returns `None`:

```rust
if !is_recognized_eventless(&value) {
    sink.record(NormalizedRecord::Observation(Box::new(UnrecognizedType { discriminator })));
    sink.record(NormalizedRecord::Unusable(PartialReason::UnrecognizedRecordType));
}
```

Both emissions sit **inside** the name test, so today the allowlist decides the
verdict. §2.1 rule 3 and §3 invert that: the structural predicate decides, and
the name only silences the observation for a record already proven inert.

`parse_record` (`analysis/vendors/jsonl.rs`) returns `None` in two cases:

- the value is not a JSON object (`value.as_object()?` — a top-level array,
  number, string, or `null` line), or
- `resolve_role` finds neither a recognized `message.role` / `role`
  (`assistant` | `user` | `system` | `tool` | `toolResult`) **nor** a standard
  turn `type` (`assistant` | `user` | `system` | `tool_use` | `function_call` |
  `tool_result` | `toolResult` | `function_call_output`).

A _present but unrecognized_ role (`role: "agent"`) therefore also reaches the
unknown path — this is the correction that shapes the design.

`is_recognized_eventless` holds sixteen names: `attachment`, `summary`,
`file-history-snapshot`, plus the thirteen housekeeping names from #222.
`record_discriminator` returns the literal `"<missing>"` when `type` is absent
or not a string.

Only `claude.rs` reaches this path today; no other adapter calls
`is_recognized_eventless` / `record_discriminator`.

### 1.2 What `Unusable` does

`SessionEvidenceAccumulator::observe` (`analysis/evidence_sink.rs`) on
`NormalizedRecord::Unusable`:

- `diagnostics.records_observed += 1`;
- `diagnostics.records_unusable += 1`;
- `diagnostics.unusable_reasons[reason] += 1`;
- `set_record_loss_reason(reason)` — **first** reason wins.

`record_loss_reason` is then read by `supported_value` and by the bespoke
`models` / `subagents` / `cache` / `previous_turn` arms of
`SessionEvidenceAccumulator::evidence`, so **every supported top-level group**
becomes `EvidenceValue::Partial { reason }`. Unsupported groups
(`quota_incidents`, `tool_definitions`, `service_tiers`, `provider_eviction`,
`harness_version`) stay `Unsupported`. `evidence.coverage` becomes
`Partial(UnrecognizedRecordType)`; the loss reason outranks a cap reason
(pinned by `a_record_loss_reason_outranks_a_cap_reason_in_coverage`).

The architecture reference (`docs/plans/local-insights-architecture.md`, the
"Source capabilities, coverage, and provenance" section) says an unknown
variant degrades "the affected evidence group". The code degrades _all
supported_ groups. That divergence is real and is **not** fixed here (§11, §14).

### 1.3 What partial coverage does to statuses

`EfficiencyReportAccumulator::observe_session` (`insights/report.rs`) increments
`counts.eligible` when capabilities hold and no required group is `Unsupported`,
and increments `counts.assessed` only when **all** required groups are
`Complete`. `detectors::status` returns `Findings` first, then
`NotAssessed(NoSessionsInWindow)` when the cohort is empty, then
`NotAssessed(CapabilityMissing)` when `eligible == 0`, then `Clean` only when
`assessed == eligible` with no contract gap, otherwise
`NotAssessed(IncompleteEvidence)` (FR-14).

Note the vocabulary trap: `EfficiencyReport::assessed_sessions` is the **cohort
size** (`observe_session` increments it unconditionally), while
`DetectorCounts::assessed` is per detector. The pane renders the former as
"{n} in the assessed cohort".

Claude satisfies the capability clauses for **seven** of the nine categories;
Overpowered Subagents (needs `subagent_models` — now `true`, see
`SourceCapabilities::claude`) and Unused Built-in Tools (`tool_definitions =
false`) are the two that vary independently of this policy. The practical blast
radius of one unknown housekeeping row today is therefore up to seven
categories, not five.

`session_badges` (`insights/badges.rs`) applies the stricter session-scope rule:
it additionally requires `evidence.coverage == EvidenceCoverage::Complete`, so a
session-wide `Partial` makes all three badges `NotAssessed(IncompleteEvidence)`.
The intentional divergence between the badge rule and the detector-scope report
rule is pinned by `session_wide_partial_coverage_diverges_from_report_by_design`.

### 1.4 Caps

`observe_observation`'s `UnrecognizedType` arm caps the discriminator with
`cap_string(...)` (`EVIDENCE_STRING_CAP = 256`) and the set at
`MAX_UNRECOGNIZED_TYPES = 16`. Two independent triggers set
`session_cap_exceeded`:

- **truncation** — the _first_ discriminator longer than 256 bytes fires it and
  inserts `"diagnostics.unrecognized_types"` into `diagnostics.truncated_strings`;
- **set overflow** — a **new** discriminator arriving when
  `unrecognized_types.len() == MAX_UNRECOGNIZED_TYPES` (so the seventeenth
  _distinct_ value) fires it and inserts `"diagnostics.unrecognized_types"`
  into `diagnostics.capped_collections` via `note_collection_cap`.

`session_cap_exceeded` yields `CoverageReason::CapExceeded` → `Partial`
**independently** of `record_loss_reason`. Pinned by
`diagnostics_unrecognized_types_overflows_to_partial` and
`diagnostics_unrecognized_type_string_overflows_to_partial`.

### 1.5 What the reader sees

Nothing. `EfficiencyReport::coverage_reasons` is accumulated and retained but
dropped by `impl From<EfficiencyReport> for InsightsReportPayload`
(`apps/desktop/src-tauri/src/dto.rs`). `InsightsPane.tsx` shows the generic
`incompleteEvidence` wording: _"Not assessed — evidence is incomplete, so a
clean result cannot be claimed"_. `diagnostics.unrecognized_types` never leaves
the engine. This is the complaint in the issue body, and it applies to the cap
path exactly as much as to the unknown-record path (§7.2 fixes both).

### 1.6 What analytics can carry

`Properties::label` / `Properties::detail` are `Option<&'static str>` and
`EventName` is a closed enum (`apps/desktop/src-tauri/src/analytics/event.rs`).
`the_documented_catalog_matches_the_code` checks that each `EVERY_EVENT` name
**appears somewhere** in `docs/analytics.md` — it does not check a row, a "When
it fires" cell, or a "Carries" cell. `no_variant_escapes_the_catalog` pins
`EVERY_EVENT` (currently 7); `every_document_that_counts_the_fields_counts_the_same_number`
pins the "thirteen fields" wording in four documents. Runtime discriminator
strings cannot travel without changing the payload type and the privacy
contract.

---

## 2. Policy

### 2.1 The rule

> An unrecognized record is classified **inert** only when the record contains
> none of the evidence-bearing shapes the parser reads. An inert unknown record
> is counted, its discriminator is retained, and the session keeps `Complete`
> coverage. Any other unrecognized record is **evidence-bearing** and fails
> closed exactly as today.

Three behavioral rules follow.

1. **Inert unknown.** Emit the `UnrecognizedType` observation with
   `inert: true` **unless the name is allowlisted**. Do **not** emit `Unusable`.
   `records_observed` counts the record whenever the observation is emitted; a
   new `records_unrecognized_inert` diagnostic makes every skip auditable.
   Coverage stays `Complete`, so the session enters detector denominators and
   can be assessed.
2. **Evidence-bearing unknown.** `UnrecognizedType { inert: false }` **and**
   `Unusable(UnrecognizedRecordType)`; supported groups go `Partial`;
   `counts.assessed` is not incremented; `Clean` becomes impossible; findings
   still survive. Shallow parser-readable evidence fails closed even for an
   allowlisted name. This _is_ the fallback if the presumption is ever wrong.
3. **Known eventless compatibility.** `is_recognized_eventless` suppresses the
   observation for inert records. It also preserves `Complete` coverage for a
   command echo or a nested scalar evidence-key name that `parse_record` cannot
   read. This prevents the #222 housekeeping regression without allowing
   shallow usage, model, role, speed, or effort shapes through.

Rule 2 closes a real hole rather than restating today's behavior: today an
allowlisted name that starts carrying `message.usage` (a future `attachment` or
`cost-state` variant) is skipped silently with `Complete` coverage and no
diagnostic. After this change it fails closed like any other shallow
evidence-bearing record. Nested scalar keys remain compatible when the parser
cannot read them, and §10.4 plus the address-findings tests pin both boundaries.

### 2.2 Why this keeps FR-14 rather than excepting it

FR-14 forbids `Clean` from **incomplete coverage**. Option B (a "best-effort
tier" that permits `Clean` over `Partial` coverage) keeps FR-14's words and
breaks its meaning: it writes "some silently dropped evidence is acceptable"
into the contract, and every future reader has to re-derive why.

Option C never asserts coverage it lacks. The claim it makes is narrow and
checkable: _this skipped record contained none of the fields the evidence
contract reads, so skipping it dropped no evidence_. That is a property of our
own parser, testable in CI, not a guess about vendor behavior. Coverage stays
`Complete` because it **is** complete.

### 2.3 The fail-closed boundary

A record with a genuinely unknown name fails closed (stays `Unusable`) when
**any** of the following is true. The list is derived from every read
`parse_record` performs to build a `NormalizedEvent`. It is mirrored by a
`const` table **in code** next to the predicate (§10.1), not only by this prose.
For a known eventless name, the scalar key rows apply only at the root and in
the root `message` object. Tool and compaction rows remain conservative at any
depth.

| Shape present                                                                                                                                                                                                                   | Parser reader it mirrors                                                                               | Why it fails closed                                                                   |
| ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------- |
| The record is not a JSON object (array, number, string, `null`)                                                                                                                                                                 | `parse_record`'s `value.as_object()?`                                                                  | Nothing can be walked, so nothing can be proven                                       |
| A `role` key at any depth, with **any** value (including a non-string or an unrecognized string)                                                                                                                                | `resolve_role` reads top level and `message`; deeper locations fail closed conservatively              | A present-but-unrecognized role could be a real turn; a recognized one already parses |
| A `usage` key at any depth                                                                                                                                                                                                      | `parse_usage` and `ev.speed` read top level and `message`; deeper locations fail closed conservatively | Token quantities                                                                      |
| A `model` key at any depth                                                                                                                                                                                                      | `ev.model` reads top level and `message`; deeper locations fail closed conservatively                  | Model identity                                                                        |
| A `speed` key at any depth                                                                                                                                                                                                      | `ev.speed` reads top level and `usage`; deeper locations fail closed conservatively                    | Fast-tier signal                                                                      |
| `effort` / `reasoning_effort` at any depth                                                                                                                                                                                      | `ev.thinking_mode` reads top level and `message`; deeper locations fail closed conservatively          | Effort tier                                                                           |
| `tool_calls` at any depth (any value, including `[]`)                                                                                                                                                                           | the OpenAI loop reads top level and `message`; deeper locations fail closed conservatively             | Tool invocations                                                                      |
| A content block of type `tool_use`, `toolCall`, `tool_result`, or `thinking`, at any level                                                                                                                                      | `process_content`, `has_tool_result_block`                                                             | Tool invocations, tool turns, thinking                                                |
| `compactMetadata`, or `subtype == "compact_boundary"`                                                                                                                                                                           | the compaction arm                                                                                     | Compaction boundaries                                                                 |
| An object carrying a **non-empty string** `name` together with `input` or `arguments`, **or** an object carrying `name` whose sibling-or-parent object carries `input` / `arguments` (the `function_call` `payload.name` split) | `push_named_tool` / `push_named_tool_str`                                                              | Tool-shaped record under a new `type` name                                            |

**Deliberately not evidence-bearing** (each entry is a row in the same in-code
table with `expected_inert = true`, so the exemption is executable, not prose):

| Exempt shape                                                            | Why nothing is lost                                                                                                                                                                                                                                                                                               |
| ----------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `timestamp` / `ts` / `created_at` / `createdAt`                         | A timestamp with no turn produces nothing; every housekeeping record carries one                                                                                                                                                                                                                                  |
| `uuid` / `parentUuid`                                                   | `EvidenceObservation::ThreadLink` is emitted for _every_ record before `parse_record`                                                                                                                                                                                                                             |
| `isSidechain` (and `message.role`-derived delegation)                   | `EvidenceObservation::DelegatedTurn` is likewise emitted for every record, and `ev.source` is unreachable without a role. See the §11 note: this makes `OverpoweredSubagents::ContractIncomplete` reachable on sessions that used to be `Partial`, which is the conservative direction and is pinned by a fixture |
| `attachment`                                                            | Context sources are read by `evidence_observations` for every record                                                                                                                                                                                                                                              |
| `message.id` alone                                                      | Dedup key only                                                                                                                                                                                                                                                                                                    |
| Free `text` content, `content` as a bare string, `content` as an object | `process_content` reads arrays only; a string or object content produces nothing without a role                                                                                                                                                                                                                   |
| `name` with an **empty-string** value plus `input`                      | `push_named_tool_str` filters empty names, so the parser reads nothing. Stated so the conservative answer is deliberate, not accidental                                                                                                                                                                           |

Three facts make this safe and worth stating in the code comment:

- The **initial-context** pass (`ClaudeContextAccumulator::observe`) and the
  skill-base-marker scan run on every record before classification.
- `<command-name>` markers can synthesize late tool calls only for parsed
  events. A genuinely unknown record containing this marker fails closed. A
  known eventless record can only echo the marker, so it remains inert.
- `SessionMetricsAccumulator` ignores `Unusable` and `Observation` records
  entirely (it matches only `NormalizedRecord::MetricsEvent`). Classification
  changes no existing metrics event or characterization golden.

### 2.4 Resolved design questions

| Question                                                                                                                | Decision                                                                                                                                                                                                                                                                              | Evidence                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| ----------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Q1** Shared `jsonl.rs` or per-vendor `claude.rs` for the shape test?                                                  | **Shared `jsonl.rs`**, adjacent to `parse_record`.                                                                                                                                                                                                                                    | The predicate must mirror `parse_record`'s reads exactly; both live in `jsonl.rs`, which already serves Claude, the generic JSONL adapter, and the SQLite adapter's embedded-JSON cells. A per-vendor copy would drift from the parser it must mirror. Vendors opt in by calling it; only `claude.rs` does today, and the Codex work in flight (PR #243) can adopt it without a second copy.                                                              |
| **Q2** Runtime discriminator strings in analytics?                                                                      | **No, not in this ticket.** Ship the closed-vocabulary event required by the standing-policy comment and this implementation task.                                                                                                                                                    | `Properties::label`/`detail` are `Option<&'static str>` by design. The standing-policy comment explicitly says to ship one closed-vocabulary event now and defer runtime discriminator strings to a separate privacy decision. Follow-up in §14.                                                                                                                                                                                                          |
| **Q3** Fix per-group degradation (`supported_value` vs the architecture text)?                                          | **No.** Out of scope; the architecture text is corrected to describe the code plus this policy, and the mismatch is filed as a follow-up.                                                                                                                                             | Changing which groups degrade would change verdicts for malformed/oversized/pinned-prefix losses too — a separate blast radius, separate tests, separate review.                                                                                                                                                                                                                                                                                          |
| **Q4** Fixture proving an unrecognized _role_ with real `usage` still fails closed?                                     | **Yes — non-negotiable.** New fixture `unrecognized_role_with_usage.jsonl` plus an assertion.                                                                                                                                                                                         | This is the permanent guard on the presumption.                                                                                                                                                                                                                                                                                                                                                                                                           |
| **D1** Allowlist and structural predicates                                                                              | **Use the strict any-depth predicate for new names.** For known eventless names, reject scalar evidence keys only where `parse_record` reads them: the root and root `message` object. Keep tool and compaction shapes conservative at any depth.                                     | Final review findings 1 and 2 reproduced #222 regressions from command echoes and nested `attachment.config.model`. New regression tests preserve those known eventless shapes while the existing `cost-state` plus `message.usage` fixture still fails closed.                                                                                                                                                                                           |
| **D2** A session whose records are **all** inert unknowns                                                               | **Behavior is unchanged in kind and is pinned, not widened.** Such a session reaches `Complete` coverage, enters `eligible` and `counts.assessed`, and can read `Clean` with no turns.                                                                                                | This is already true today for a session containing only allowlisted housekeeping names, so the ticket widens the _set_ of such sessions and creates no new class of false clean. `in_denominator` excludes zero-work sessions for `UnusedMcpServers` / `UnusedSkills` only. Widening that rule to every detector would change today's housekeeping-only sessions too — a separate blast radius. Pinned by a test (§10.5) and filed as a follow-up (§14). |
| **D3** Non-object records and `<missing>` discriminators                                                                | Non-object record ⇒ **not inert** (fail closed). An object with an absent or non-string `type` is classified structurally like any other; the engine keeps the literal `"<missing>"` discriminator (auditable) and the **pane** renders it as "records with no type" rather than raw. | `record_discriminator`'s `unwrap_or("<missing>")`; §7.2 and §10.7.                                                                                                                                                                                                                                                                                                                                                                                        |
| **D5** Cap visibility                                                                                                   | **Surface both causes.** `UnrecognizedRecords` gains `capped_sessions` for set overflow and `truncated_sessions` for an overlong type name. The pane gives each cause its own sentence.                                                                                               | A 17-distinct-type session and a one-type session with a long discriminator both leave the assessed set. Combining them made the one-type sentence factually false.                                                                                                                                                                                                                                                                                       |
| **Discriminator-cap behavior**                                                                                          | **The external result is preserved, but group propagation changed.** A truncated discriminator or a seventeenth distinct one still yields `Partial(CapExceeded)` and is not assessed. The implementation also sets `record_loss_reason` so every supported group becomes partial.     | On `main`, `session_cap_exceeded` changed session coverage only, so detector groups could remain complete and the report could still assess the session. The new propagation makes the intended denominator result true. Other session-scope caps deliberately keep their existing coverage-only behavior.                                                                                                                                                |
| **Badge vs report divergence** ([#5437642766](https://github.com/antiburn/antiburn/issues/229#issuecomment-5437642766)) | Divergence rule **unchanged**, but the badge _output_ changes for affected sessions and that is called out in the changelog and pinned by a new badge test (§10.5, §9).                                                                                                               | Under Option C an inert unknown leaves coverage `Complete`, so badges and report now **agree** for exactly the case this ticket is about. `session_wide_partial_coverage_diverges_from_report_by_design` stays green and unmodified.                                                                                                                                                                                                                      |

---

## 3. Data flow after the change

```text
Claude JSONL record
  → BoundedJsonlReader (unchanged)
  → ClaudeContextAccumulator::observe            (unchanged, every record)
  → evidence_observations()                       (unchanged, every record:
                                                   ContextSource, SubagentSpawn,
                                                   DelegatedTurn, ThreadLink)
  → parse_record() == None
        └── is_recognized_eventless(value)
             ├── true  → is_inert_recognized_eventless(value)
             │            (root and root-message scalar evidence keys fail closed;
             │             command echoes and unread nested scalar keys stay inert)
             └── false → is_inert_unrecognized(value)
                          (strict any-depth scan; a command marker also fails closed)
                    ├── false → Observation(Box::new(UnrecognizedType{ inert: false }))
                    │            + Unusable(UnrecognizedRecordType)
                    │            coverage: Partial(UnrecognizedRecordType)
                    └── true  → allowlisted: nothing emitted
                                 new name: Observation(Box::new(UnrecognizedType{ inert: true }))
                                 coverage: Complete unless a cap fires

SessionEvidence (schema revision 3)
  → session_evidence row (revision-gated)
  → EfficiencyReportAccumulator::observe_session
        → detector eligible/assessed  (inert session now assessable)
        → coverage_reasons            (only failed-closed and capped sessions now)
        → NEW: UnrecognizedRecords summary (bounded types + five counts)
  → EfficiencyReport
        ├── InsightsReportPayload.unrecognizedRecords → InsightsPane coverage section
        └── analytics::record_unrecognized_records(app, &report)
                → consent gate (analytics::allowed) → bucketed, closed-vocabulary event
```

---

## 4. Exact files and symbols

### 4.1 Engine — `crates/antiburn-local`

| File                             | Symbol                                                                                                  | Change                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| -------------------------------- | ------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/analysis/vendors/jsonl.rs`  | **new** `is_inert_unrecognized` and `is_inert_recognized_eventless`                                     | The strict predicate checks evidence keys and tool shapes at any depth. The known-eventless predicate limits scalar evidence keys to parser-readable depth while retaining conservative tool and compaction checks. `BoundedJsonlReader` limits each scan to 8 MiB. The table supplies behavioral coverage, while a source fingerprint covers `parse_record`, its helpers, `parse_usage`, and `parse_ts`.                                                                                                                         |
| `src/analysis/vendors/jsonl.rs`  | `is_recognized_eventless`                                                                               | Its doc comment records the known-eventless compatibility boundary.                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| `src/analysis/interface.rs`      | `EvidenceObservation::UnrecognizedType`                                                                 | Add `inert: bool`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| `src/analysis/vendors/claude.rs` | `ClaudeAdapter::visit_reader`                                                                           | Select the known-eventless or strict predicate per §3. A command marker fails closed only for a genuinely unknown name. Emit `Unusable` only when not inert.                                                                                                                                                                                                                                                                                                                                                                      |
| `src/analysis/evidence.rs`       | `ParseDiagnostics`                                                                                      | Add `pub records_unrecognized_inert: u64` (camelCase `recordsUnrecognizedInert` on the wire); initialize in `ParseDiagnostics::new`. Add the struct-level doc comment the type currently lacks, stating what `records_observed` counts (metrics events, unusable records, and observed inert unknowns — **not** allowlisted eventless records) so the field does not silently mean three things across revisions.                                                                                                                 |
| `src/analysis/evidence_sink.rs`  | `observe_observation` (`UnrecognizedType` arm)                                                          | When `inert`, add to `records_observed` and `records_unrecognized_inert` before cap checks. When `!inert`, the paired `Unusable` counts the record. Discriminator truncation and set overflow now also set `record_loss_reason(CapExceeded)` so supported detector groups become partial; specific record loss later replaces this weaker cap reason. Other session-scope caps remain coverage-only.                                                                                                                              |
| `src/analysis/mod.rs`            | `PARSER_REVISION`, `EVIDENCE_SCHEMA_REVISION`                                                           | `3 → 4` and `2 → 3`. `ANALYZER_REVISION` and `METRICS_SCHEMA_REVISION` unchanged (§5).                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| `src/insights/report.rs`         | **new** `pub struct UnrecognizedRecords`, **new** `pub const MAX_REPORT_UNRECOGNIZED_TYPES: usize = 16` | Fields: `types: BTreeSet<String>`, `types_truncated: bool`, `sessions_with_types: u64`, `inert_sessions: u64`, `evidence_bearing_sessions: u64`, `capped_sessions: u64`, `truncated_sessions: u64`. The five counts are **not** exclusive. Every string is already ≤ `EVIDENCE_STRING_CAP` bytes because only revision-current rows enter the cohort.                                                                                                                                                                             |
| `src/insights/report.rs`         | `EfficiencyReport`, `EfficiencyReportAccumulator` (`observe_session`, `finish`)                         | Accumulate per cohort session from `evidence.diagnostics`: `sessions_with_types` when `unrecognized_types` is non-empty; `inert_sessions` when `records_unrecognized_inert > 0`; `evidence_bearing_sessions` when `unusable_reasons` holds `UnrecognizedRecordType`; `capped_sessions` from `capped_collections`; and `truncated_sessions` from `truncated_strings`. A session-local set cap also sets `types_truncated`. Union type strings up to `MAX_REPORT_UNRECOGNIZED_TYPES`, setting `types_truncated` on report overflow. |
| `src/insights/mod.rs`            | re-exports                                                                                              | Export `UnrecognizedRecords`, `MAX_REPORT_UNRECOGNIZED_TYPES`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| `src/insights/detectors/mod.rs`  | module doc                                                                                              | One paragraph: an unknown record blocks `Clean` only when it is evidence-bearing or capped; inert unknowns are proven to drop nothing; a session of only inert records is a zero-work session, which `in_denominator` handles for two detectors only (§14 follow-up).                                                                                                                                                                                                                                                             |
| `src/insights/badges.rs`         | —                                                                                                       | **No code change**, but the _output_ changes for inert-unknown sessions (all three badges move from `NotAssessed(IncompleteEvidence)` to a real status). New test in §10.5; changelog entry in §9.                                                                                                                                                                                                                                                                                                                                |

### 4.2 Desktop shell — `apps/desktop/src-tauri`

| File                                                               | Symbol                                                                                                                                                                                                                                                                                                    | Change                                                                                                                                                                                                                                                                                                                                                                                       |
| ------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/dto.rs`                                                       | **new** `InsightsUnrecognizedRecordsPayload`                                                                                                                                                                                                                                                              | `types: Vec<String>` (alphabetical, from the `BTreeSet`), `types_truncated: bool`, `sessions_with_types: u64`, `inert_sessions: u64`, `evidence_bearing_sessions: u64`, `capped_sessions: u64`, `truncated_sessions: u64`. The doc comment cites the 256-byte and 16-value bounds and states that the counts are non-exclusive.                                                              |
| `src/dto.rs`                                                       | `InsightsReportPayload`, `impl From<EfficiencyReport>`                                                                                                                                                                                                                                                    | New field `unrecognized_records`. `InsightsCoveragePayload` keeps its `Copy` derive and its FR-12 partition (see D6 below). The `report()` test helper builds through `EfficiencyReportAccumulator::finish`, so it needs no edit; only the pinned top-key list does.                                                                                                                         |
| `src/analytics/event.rs`                                           | `EventName`, `EventName::as_str`, `EVERY_EVENT`, `no_variant_escapes_the_catalog`                                                                                                                                                                                                                         | Add `UnrecognizedRecordsObserved` → `antiburn.unrecognized_records_observed`; count `7 → 8`.                                                                                                                                                                                                                                                                                                 |
| `src/analytics/mod.rs`                                             | **new** `pub fn record_unrecognized_records(app: &tauri::AppHandle, summary: &UnrecognizedRecords)`, **new** named `UnrecognizedOutcome`, **new** `static LAST_UNRECOGNIZED: Mutex<Option<UnrecognizedOutcome>>`, **new** `fn unrecognized_outcome_is_new(...) -> bool`, **new** `fn reset_suppression()` | §6. Named fields prevent label/bucket swaps. The narrow argument excludes report fields that analytics does not need. `reset_suppression` clears `LAST_SCAN` and `LAST_UNRECOGNIZED`, so consent withdrawal leaves no residue.                                                                                                                                                               |
| `src/commands.rs`                                                  | `get_insights_report`                                                                                                                                                                                                                                                                                     | Pass only `&report.unrecognized_records` before `Ok(report.into())`.                                                                                                                                                                                                                                                                                                                         |
| `src/insights_report.rs`, `src/insights_worker.rs`, `src/store/**` | —                                                                                                                                                                                                                                                                                                         | **No change.** The revision bumps flow through the existing `CURRENT_EVIDENCE_PREDICATE` and the lazy requeue query. `insights_ipc.rs` builds reports through `EfficiencyReportAccumulator::finish`, so the new field breaks no literal, and the cancelled/empty path never reaches `record_unrecognized_records` (the call site is in `commands.rs`, after a successful reduction or join). |

### 4.3 Desktop frontend — `apps/desktop/src`

| File                                  | Change                                                                                                                                                                                                                                                                                                                                                                                                      |
| ------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/lib/insightsIpc.ts`              | Add `InsightsUnrecognizedRecordsPayload` and `unrecognizedRecords` on `InsightsReportPayload`.                                                                                                                                                                                                                                                                                                              |
| `src/views/settings/InsightsPane.tsx` | Render an `UnrecognizedRecordsNote` inside `CoverageSection`, **outside and after** the `COVERAGE_ROWS` `<ul>`, whenever `sessionsWithTypes > 0` — including when nothing was blocked. Existing semantic utilities only (`type-footnote`, `text-label`, `text-label-secondary`); no new token, stylesheet, or `useEffect`, so `design.md` needs no update and `scripts/check-design-drift.mjs` stays green. |

---

## 5. Schema, revisions, and migration

| Constant                   | Now | After | Why                                                                                                  |
| -------------------------- | --- | ----- | ---------------------------------------------------------------------------------------------------- |
| `PARSER_REVISION`          | 3   | **4** | Classification changes which records produce `Unusable`, so the same bytes yield different evidence. |
| `EVIDENCE_SCHEMA_REVISION` | 2   | **3** | `ParseDiagnostics` gains `records_unrecognized_inert`; the persisted evidence JSON shape changes.    |
| `ANALYZER_REVISION`        | 5   | 5     | No analyzer rule changes.                                                                            |
| `METRICS_SCHEMA_REVISION`  | 1   | 1     | `SessionMetricsAccumulator` ignores `Unusable` and `Observation`; metrics output is byte-identical.  |

**No SQLite migration.** The revision columns already exist
(`store/mod.rs`, `session_evidence`). Requeue is lazy per FR-11: the pending
query in `Store` marks revision-mismatched rows for reprocessing without
incrementing `source_generation`.

**Between ship and requeue.** `CURRENT_EVIDENCE_PREDICATE`
(`insights_report.rs`) excludes revision-mismatched rows as **`stale`**, so
affected sessions leave the assessed cohort entirely rather than showing a wrong
answer. `session_hygiene_payload` (`commands.rs`) likewise returns
`not_assessed("stale", …)`. Expect a visible dip in `assessedSessions` and
badge coverage until reanalysis catches up — safe, honest, and already the
shipped behavior for every revision bump.

**Deserialization.** Two readers deserialize stored `evidence_json`
(`insights_report.rs` cohort query and `commands.rs::session_hygiene_payload`)
and both are revision-gated _before_ `serde_json::from_str`, so no old row is
ever deserialized against the new struct. A third column,
`session_evidence.diagnostics_json` (written by `insights_worker.rs`), is
**never deserialized in production** — only `store/tests.rs` compares it as a
string — so it needs no gate and no `serde(default)` either. **Decision D8:** do
**not** add `#[serde(default)]` to the new diagnostic; the revision gate is the
contract, and a silent default would hide exactly the drift the revision exists
to catch. The downgrade direction (an older build meeting `parserRevision = 4`)
is equally safe: a mismatch is stale in both directions.

**Verdict direction.** After requeue, verdicts may move `NotAssessed →
Clean` (or to `Findings` where a finding was already visible), and session
badges may move from `NotAssessed(IncompleteEvidence)` to a real status. That
direction is expected. The reverse must never follow from an allowlist edit
alone; once a type is later name-recognized, behavior is already identical —
only quieter diagnostics and analytics.

---

## 6. Analytics: privacy and consent contract

**Accepted scope.** This event does **not** transmit the discriminator strings
suggested by the issue's initial acceptance sketch. The later standing-policy
comment explicitly replaces that sketch with one closed-vocabulary event now
and a separate privacy decision for runtime strings. This implementation task
repeats that requirement. No additional issue comment is needed before step 8.

### 6.1 The event

| Property | Value                                                                                                                                                                                                                                                                          |
| -------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Name     | `antiburn.unrecognized_records_observed`                                                                                                                                                                                                                                       |
| Fires    | When `get_insights_report` returns a report whose cohort contains at least one session with an unrecognized record type, and the outcome differs from the last one reported in this run.                                                                                       |
| `bucket` | `event::bucket(sessions_with_types)` — one of `1-9`, `10-49`, `50-199`, `200-999`, `1000+`. `"0"` is unreachable because the event only fires when the count is positive. The bucket counts **sessions carrying an unrecognized type**, not the number of distinct type names. |
| `label`  | Closed vocabulary of three: `evidence_bearing` when `evidence_bearing_sessions > 0`; else `inert_capped` when `capped_sessions > 0` or `truncated_sessions > 0`; else `inert_only`. The precedence mirrors the engine, where a record loss reason outranks a cap reason.       |
| `detail` | `None`. The report is native-scope only; a second dimension would say more about the reader without answering a product question.                                                                                                                                              |

**Sampling bias, stated in `docs/analytics.md`.** The only call site is
`get_insights_report`, so the event fires only for readers who open
Settings → Insights, and at most once per distinct outcome per run.
`InsightsController::report` also _joins_ an in-flight reduction, so a caller
can fire the event for a reduction it did not cause. The figure is therefore
**not** a population rate and must never be read as one.

### 6.2 What it deliberately does not carry

The discriminator strings themselves. `Properties::label` is
`Option<&'static str>` and the vocabulary is closed by design; runtime strings
require a payload-type change and its own privacy review (Q2, follow-up in
§14). The consequence is stated honestly in `docs/analytics.md`: new names are
discovered from the local diagnostic, the Insights coverage note, and support
threads until the separate decision is made.

### 6.3 Consent and suppression

- **Consent.** `record_unrecognized_records` calls `analytics::record`, whose
  first statement is the `allowed(app)` gate: the endpoint must be configured,
  `analytics_enabled` must be set, **and** onboarding must have completed. A
  clean checkout cannot transmit at all (`config::configured()` is false;
  pinned by `a_clean_checkout_cannot_transmit`).
- **Single choke point.** The call site lives inside `analytics/mod.rs`;
  `commands.rs` passes the report and nothing else. No path can reach
  `Store::queue_analytics_event` without passing `allowed`.
- **Suppression.** Mirror `record_scan`: check `allowed` **first**, then compare
  `(label, bucket)` against `LAST_UNRECOGNIZED` (an in-memory `Mutex`, like
  `LAST_SCAN`, because it is a suppression hint with no business on the
  reader's disk) and drop an unchanged outcome. A clean cohort records the
  internal outcome `("none", "none")` **without transmitting anything**, exactly
  as `record_scan` records its failure outcome, so an
  `evidence_bearing → clean → evidence_bearing` sequence reports twice rather
  than being suppressed by a stale value.
- **Withdrawal.** `handle_settings_transition` calls the new
  `reset_suppression()`, which clears `LAST_SCAN` and `LAST_UNRECOGNIZED`
  together. Without this, opting out and back in inside one run would suppress
  the first report the reader actually consented to — a consent regression the
  existing `LAST_SCAN` comment explicitly forbids.

### 6.4 Documentation obligations

- `docs/analytics.md` event-catalog table gains a row.
  `the_documented_catalog_matches_the_code` only asserts that the **name string
  appears somewhere in the file**; it does not verify the row, the "When it
  fires" cell, or the "Carries" cell. Write the row by hand and review it as
  copy — the test is a floor, not a check.
- The sentence under the table, _"Two of those are deliberately not sent once
  per occurrence"_, becomes **three** and must name the new event's suppression
  rule. Nothing enforces this sentence; it is on the checklist (§13, step 8).
- `EVERY_EVENT` and the exhaustive match in `no_variant_escapes_the_catalog`
  gain the variant; the length assertion moves `7 → 8`.
- No new payload field, so `every_document_that_counts_the_fields_counts_the_same_number`
  stays at "thirteen fields" and `PrivacyPane.tsx`, `docs/privacy-policy.md`,
  `docs/support.md` need no change.
- Add the "why not the names" sentence and the sampling-bias sentence (§6.1).

---

## 7. Report and UI surfacing

### 7.1 Payload

```jsonc
// InsightsReportPayload.unrecognizedRecords
{
  "types": ["relay_probe", "telemetry_ping"], // alphabetical, ≤ 16, each ≤ 256 chars
  "typesTruncated": false,
  "sessionsWithTypes": 3,
  "inertSessions": 3,
  "evidenceBearingSessions": 0,
  "cappedSessions": 0,
  "truncatedSessions": 0,
}
```

The five counts are **non-exclusive**: one session can hold inert and
evidence-bearing unknowns and can hit either limit. Therefore,
`inertSessions + evidenceBearingSessions` may exceed `sessionsWithTypes`. Only
`sessionsWithTypes` is a cohort-subset count suitable for an "N of M" sentence.

`cappedSessions` comes from `diagnostics.capped_collections`, while
`truncatedSessions` comes from `diagnostics.truncated_strings`. Both use the
`"diagnostics.unrecognized_types"` marker. Those diagnostic sets are themselves
capped at `MAX_DIAGNOSTIC_FIELDS`, so each count is best-effort and the coverage
reason remains authoritative.

**Decision D6 — placement.** The issue comment proposed folding this into
`InsightsCoveragePayload`. It ships as a **sibling field on
`InsightsReportPayload`, rendered inside the Coverage section**. Rationale:
`InsightsCoveragePayload` is the FR-12 partition — every member is a count of
discovered sessions in exactly one exclusive bucket, it derives `Copy`, and
`InsightsPane.tsx` iterates it as `key: keyof InsightsCoveragePayload` with
`coverage[key] > 0`. A string array inside it breaks the partition's meaning,
the `Copy` derive, and that iteration's typing. The durable requirement from the
issue — _survive DTO conversion and render whenever present, including when
nothing was blocked_ — is met in full.

**Privacy note for review.** `types` are `type` discriminators: schema
vocabulary, never payloads, capped at 16 per session and 256 bytes per value,
capped again at 16 at report level. The precedent is
`InsightsQuotaFindingsPayload::affected_models`, which already carries
transcript-derived model names across the same boundary. They stay on the
device; §6 keeps them out of analytics.

### 7.2 Pane copy (draft; wording is the pane's, identifiers are the engine's)

Rendered **after** the `COVERAGE_ROWS` `<ul>`, not inside it: the list is the
FR-12 exclusive-denominator partition, and a cohort-subset count inside it would
read as a seventh bucket. The note names its population explicitly.

Lead line, when `sessionsWithTypes > 0`:

> **3 of the 12 sessions in the assessed cohort contained record types antiburn
> does not recognise:** `relay_probe`, `telemetry_ping`.

- The type list renders `<missing>` as _records with no type_.
- ` and more` is appended when `typesTruncated` is true.
- Singular forms use the pane's existing `n === 1 ? "session" : "sessions"`
  style; the "1 of the 12 sessions" case is tested.

Then exactly one of these, in this order:

- `evidenceBearingSessions > 0`:
  > 1 of those sessions contained a record that could carry usage data, so some
  > checks cannot report a result for it.
- `cappedSessions > 0`:
  > 1 of those sessions contained more unrecognised types than antiburn records,
  > so some checks cannot report a result for it.
- `truncatedSessions > 0`:
  > 1 of those sessions contained a record type that antiburn could not record
  > in full, so some checks cannot report a result for it.
- when all three blocked counts are zero:
  > These records carry no usage data, so antiburn can still report results for
  > those sessions.

All applicable blocked sentences render, with evidence-bearing first. Avoid the
word "assessed" in the second clause:
under FR-12 the _cohort_ membership is what "assessed" names in this card, and
what is actually withheld is a per-category result.

Constraints: `type-footnote text-label-secondary` (and `text-label` for the
lead line) only; no hard-coded color, size, radius, or duration; no new
stylesheet; no `useEffect` (the value is derived during render from the
snapshot).

---

## 8. Documentation work

| Document                                                                                       | Change                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| ---------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `docs/plans/local-insights-architecture.md` (§"Source capabilities, coverage, and provenance") | Replace the "An unknown schema variant degrades the affected evidence group…" paragraph with the Option C statement: an unknown variant degrades coverage **unless** it is structurally proven inert against every field the evidence contract reads; the discriminator set stays bounded and capped either way, and a capped session still degrades. Note that the code degrades all _supported_ top-level groups (not "the affected group") and that the narrowing is a separate follow-up. |
| `crates/antiburn-local/src/analysis/vendors/jsonl.rs`                                          | The in-code policy home: the `is_inert_unrecognized` doc comment states the rule, the reader-to-shape mapping, every exemption with its reason, the fail-closed default for non-object records, the framing bound, and why a present-but-unrecognized role fails closed. ASD-STE100 per `AGENTS.md`.                                                                                                                                                                                          |
| `crates/antiburn-local/src/analysis/evidence.rs`                                               | New `ParseDiagnostics` struct doc stating what `records_observed` counts after this revision.                                                                                                                                                                                                                                                                                                                                                                                                 |
| `crates/antiburn-local/src/insights/detectors/mod.rs`                                          | Module doc: how the policy interacts with FR-14 and `Clean`, plus the zero-work-session note.                                                                                                                                                                                                                                                                                                                                                                                                 |
| `crates/antiburn-local/tests/fixtures/claude_characterization/README.md`                       | Update the `unrecognized_type.jsonl` row (now proves `Complete` coverage plus a retained discriminator); add rows for the four new fixtures in a clearly marked sub-table stating they carry **no golden** and are exercised by `tests/unrecognized_records.rs`; add a coverage-matrix note that unknown variants degrade only when evidence-bearing or capped.                                                                                                                               |
| `docs/analytics.md`                                                                            | New catalog row, the "why not the names" sentence, the sampling-bias sentence, and the "three of those" correction (§6.4).                                                                                                                                                                                                                                                                                                                                                                    |
| `docs/plans/local-insights-followups.md`                                                       | Append the §14 entries in the document's five-field shape. Update the "Additional Claude JSONL row types" entry to record that #229 shipped.                                                                                                                                                                                                                                                                                                                                                  |

---

## 9. Changelog work

Both changelogs are load-bearing and release-gated. `## [Unreleased]` in the
root `CHANGELOG.md` is currently empty, so the entry creates the `### Changed`
subsection.

**`CHANGELOG.md` → `## [Unreleased]` → `### Changed`** (reader-facing; describe
the user impact, not the mechanism):

> - **Sessions containing record types antiburn does not recognise are now
>   assessed again.** A new housekeeping record from a coding agent used to make
>   a whole session's evidence read as incomplete, which quietly held back
>   results for most checks and left every session badge unassessed. antiburn
>   now proves the skipped record carries no usage data before assessing the
>   session, names the unrecognised types in the Insights coverage section, and
>   still declines to assess a session when a skipped record could have carried
>   usage data or when one session carries more unknown types than antiburn
>   records. Insights results and session badges refresh as sessions are
>   re-read.

**`crates/antiburn-local/CHANGELOG.md` → `## [Unreleased]`**:

- `### Changed` — **Breaking:** `EvidenceObservation::UnrecognizedType` gains an
  `inert` field, `ParseDiagnostics` gains `records_unrecognized_inert`, and
  `EfficiencyReport` gains `unrecognized_records`. None of these types is
  `#[non_exhaustive]`, so every added field breaks downstream literal
  construction and exhaustive matches — including the additive-sounding report
  field. `PARSER_REVISION` is 4 and `EVIDENCE_SCHEMA_REVISION` is 3, so stored
  evidence from earlier versions is stale and is reprocessed lazily. A
  structurally inert unrecognized record no longer degrades session coverage,
  so `session_badges` output changes for affected sessions. An
  evidence-bearing record whose `type` is on the eventless allowlist now fails
  closed instead of being skipped silently.
- `### Added` — `insights::UnrecognizedRecords` and
  `insights::MAX_REPORT_UNRECOGNIZED_TYPES`, with the bounded discriminator set
  and the four per-cohort counts.

---

## 10. Test matrix

### 10.1 Engine — predicate (`src/analysis/vendors/jsonl.rs`, `#[cfg(test)]`)

| Test                                                                      | Asserts                                                                                                                                                                                 |
| ------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `a_bare_housekeeping_record_is_inert`                                     | `{"type":"telemetry_ping","timestamp":…,"payload":{"ok":true}}` → inert                                                                                                                 |
| `an_unrecognized_role_is_not_inert`                                       | `role: "agent"` (top level and under `message`) → not inert                                                                                                                             |
| `a_non_string_role_is_not_inert`                                          | `role: 7`, `role: null`, `role: {}` → not inert                                                                                                                                         |
| `a_non_object_record_is_not_inert`                                        | `[]`, `7`, `"text"`, `null` → not inert (D3)                                                                                                                                            |
| `usage_model_speed_or_effort_is_not_inert`                                | one case per key, top level and under `message`, plus `usage: {}`, `usage: null`, and `usage.speed`                                                                                     |
| `tool_calls_at_either_level_is_not_inert`                                 | top level, `message.tool_calls`, and the empty array `tool_calls: []`                                                                                                                   |
| `a_tool_or_thinking_content_block_at_any_level_is_not_inert`              | `tool_use`, `toolCall`, `tool_result`, `thinking`, including nested one level below `message.content`                                                                                   |
| `a_name_with_input_or_arguments_is_not_inert`                             | top level, under `message`, under `payload`, and the `function_call` split shape `{"payload":{"name":"Bash"},"arguments":{…}}`                                                          |
| `an_empty_name_with_input_is_inert`                                       | `{"name":"","input":{…}}` → inert, because `push_named_tool_str` filters empty names (the decided conservative answer, stated in the doc comment)                                       |
| `message_or_content_of_the_wrong_json_type_is_inert`                      | `message` as an array/string, `content` as an object or bare string → inert (`process_content` reads arrays only)                                                                       |
| `compaction_metadata_is_not_inert`                                        | `compactMetadata`; `subtype: "compact_boundary"`                                                                                                                                        |
| `allowlisted_names_use_the_recognized_eventless_predicate`                | Every one of the sixteen `is_recognized_eventless` names is inert through the production predicate; the same name plus `message.usage` is **not** inert.                                |
| `recognized_eventless_records_fail_closed_on_every_parser_readable_shape` | The production predicate rejects every scalar key at the root and root `message`, plus any-depth tool and compaction shapes. It permits an unread nested scalar key and a command echo. |
| `large_and_deep_allowlisted_records_stay_inert`                           | A 500-entry snapshot and depth-eight summary remain inert; the framing limit is the scan bound                                                                                          |
| `every_field_parse_record_reads_appears_in_the_inertness_table`           | The table covers evidence-bearing readers and named exemptions. It is behavioral coverage, not an automatic drift detector                                                              |
| `parse_record_changes_require_an_inertness_review`                        | A source fingerprint over `parse_record` and its evidence-shape helpers fails when that block changes, forcing the table and predicate to be reviewed                                   |

### 10.2 Engine — sink (`src/analysis/evidence_sink.rs`)

| Test                                                                                   | Asserts                                                                                                                                                                                                                                                                                                                    |
| -------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `an_inert_unrecognized_record_keeps_complete_coverage`                                 | coverage `Complete`; `records_unusable == 0`; `unusable_reasons` empty; `records_observed == 1`; `records_unrecognized_inert == 1`; discriminator retained                                                                                                                                                                 |
| `an_evidence_bearing_unrecognized_record_still_fails_closed`                           | coverage `Partial(UnrecognizedRecordType)`; every supported group `Partial`; `records_unrecognized_inert == 0`; `records_observed == 1` (no double count)                                                                                                                                                                  |
| `diagnostics_unrecognized_types_overflows_to_partial` (existing)                       | Mechanically unchanged — it constructs `EvidenceObservation::UnrecognizedType` directly and never emits `Unusable`, so it already drives the inert path. It needs only the new `inert: true` field. Extend it to assert `records_unrecognized_inert` counts **all seventeen** records, so the audit count survives the cap |
| `diagnostics_unrecognized_type_string_overflows_to_partial` (existing)                 | Same: add `inert: true`; truncation still produces `Partial(CapExceeded)`                                                                                                                                                                                                                                                  |
| **new** `a_capped_inert_session_and_an_evidence_bearing_record_report_the_loss_reason` | Mixed session: cap exceeded **and** one evidence-bearing unknown ⇒ coverage is `Partial(UnrecognizedRecordType)`, not `CapExceeded`                                                                                                                                                                                        |
| `a_record_loss_reason_outranks_a_cap_reason_in_coverage` (existing)                    | unchanged                                                                                                                                                                                                                                                                                                                  |

Both existing cap tests are the only in-crate construction sites of
`EvidenceObservation::UnrecognizedType` besides the production match arm, so the
new field costs two mechanical edits.

### 10.3 Engine — serialization and revisions

| Test                                                                                                                                   | Change                                                                                                                     |
| -------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------- |
| `complete_session_evidence_serializes_to_the_exact_object` / `partial_session_evidence_serializes_to_the_exact_object` (`evidence.rs`) | Add `"recordsUnrecognizedInert": 0`; update `parserRevision` 3→4 and `evidenceSchemaRevision` 2→3 wherever they are pinned |
| `apps/desktop/src-tauri/src/analysis.rs` revision assertions                                                                           | Follow the constants                                                                                                       |

### 10.4 Engine — fixtures and integration

**Fixture plumbing decision (D10).** The four new fixtures join
`tests/claude_characterization.rs::fixture()` (new `include_str!` arms) and
`evidence_fixture_names()` (`[&str; 18] → [&str; 22]`) so the persisted-evidence
privacy scan `evidence_holds_no_prompt_or_message_text` covers them. They do
**not** join `fixture_names()`, which is the golden set: metrics are unchanged
by design, so a golden would pin nothing this ticket can break, and staying out
avoids four new golden files, a `[&str; 15]` length change, and a second
`include_str!` match arm in `src/analysis/tests.rs`. Behavioral assertions live
in a new `tests/unrecognized_records.rs`, which loads fixtures through a new
`tests/support/claude_fixture.rs` (`#[path = …] mod` — the pattern
`pipeline_corpus.rs` already uses for `support/corpus.rs`) exposing
`read_fixture(name) -> String`, `session_input(name)`, and `stream_composite`.
No helper is copied.

| Fixture / test                                                                               | Change                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| -------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `tests/fixtures/claude_characterization/unrecognized_type.jsonl`                             | **Unchanged bytes.** Line 2 (`{type, timestamp, payload}`) is inert; line 3 parses today via `message.role`. `src/analysis/tests.rs::the_characterization_fixtures_report_their_expected_coverage` flips this fixture to `RecordCoverage::Complete` with **no** partial reasons. `golden_unrecognized_type` must stay byte-identical (goldens contain only `normalizedSession` + `sessions`; verify with `UPDATE_GOLDENS=1` + `git diff --exit-code`)                                                                                                         |
| `tests/claude_characterization.rs::evidence_coverage_is_complete_for_every_clean_fixture`    | **Newly exercises `unrecognized_type`**: it currently `continue`s on `RecordCoverage::Partial`, and that fixture stops being partial. Verify it now asserts `evidence.coverage == Complete` **and** `evidence.context` is `Complete(_)`. No edit expected; confirm rather than assume                                                                                                                                                                                                                                                                         |
| **new** `unrecognized_role_with_usage.jsonl`                                                 | Q4's permanent guard: `{"type":"telemetry_ping","role":"agent","usage":{…}}` between two valid records ⇒ coverage `Partial(UnrecognizedRecordType)`, supported groups `Partial`, neighbours survive, the detector cannot read `Clean`                                                                                                                                                                                                                                                                                                                         |
| **new** `unrecognized_evidence_shapes.jsonl`                                                 | One record per evidence-bearing shape class under unknown `type` names — tool-call, thinking block, compaction metadata, model/usage, the `function_call` `payload.name` + `arguments` split, and an **allowlisted** name (`cost-state`) carrying `message.usage` ⇒ each fails closed. The last line is the §2.1 rule 2 pin                                                                                                                                                                                                                                   |
| **new** `unrecognized_inert_records.jsonl`                                                   | Several distinct inert housekeeping types **plus real assistant turns carrying `usage` and `model`** (so the session is not a zero-work session and cannot pass for the wrong reason) ⇒ coverage `Complete`, discriminators retained, session enters detector denominators, and a named detector reads `Clean`. Assert on a detector whose Claude eligibility is stable and independent of this ticket — **not** `OverpoweredSubagents` (varies with delegation evidence) and **not** `UnusedBuiltInTools` (`tool_definitions = false` ⇒ `CapabilityMissing`) |
| **new** `unrecognized_inert_sidechain.jsonl`                                                 | An inert unknown carrying `isSidechain: true` with no other subagent evidence ⇒ coverage `Complete` and the resulting `OverpoweredSubagents` observation is pinned (`ContractIncomplete` is now reachable where the session used to be `Partial`). Pins the §2.3 `isSidechain` exemption's downstream effect rather than leaving it to be discovered                                                                                                                                                                                                          |
| `tests/unrecognized_records.rs` (**new file**)                                               | Home for the four fixtures' assertions plus the report-level and denominator tests of §10.5 that need a real fixture. Keeps `claude_characterization.rs` (1290 lines) from growing                                                                                                                                                                                                                                                                                                                                                                            |
| `tests/claude_characterization.rs`                                                           | Add the `fixture()` arms and the `evidence_fixture_names()` entries only. `housekeeping_records_keep_complete_coverage_and_no_unrecognized_diagnostics` stays green **unmodified** (every record in that fixture is structurally inert **and** allowlisted, so nothing is emitted). `unrecognized_type_without_role_is_dropped_but_with_role_is_kept` also stays green **unmodified** — name it in the PR as a deliberately unchanged test, because it reads like a test about this feature                                                                   |
| `tests/pipeline_corpus.rs::housekeeping_tail_with_unrecognized_types_degrades_coverage_only` | The generated records are `{type, timestamp, payload:{sweep}}` ⇒ now inert. Rename to `housekeeping_tail_with_inert_unrecognized_types_keeps_coverage` and assert `EvidenceCoverage::Complete`, retained discriminators, and `records_unrecognized_inert > 0`. Add a sibling case driving the new evidence-bearing knob and asserting `Partial(UnrecognizedRecordType)`                                                                                                                                                                                       |
| `tests/support/corpus.rs`                                                                    | Add `SessionSpec::evidence_bearing_unrecognized_every: Option<usize>` **and** the matching `Tallies::evidence_bearing_unrecognized_records` counter, so the existing `tallies.unrecognized_records` assertion in `pipeline_corpus.rs` stays unambiguous once two kinds of unknown record exist                                                                                                                                                                                                                                                                |

### 10.5 Engine — report and badges

| Test                                                                                             | Asserts                                                                                                                                                                                                                                                                                                                                                                                          |
| ------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `unrecognized_records_summarizes_the_cohort`                                                     | counts for inert-only, evidence-bearing, capped, and **mixed** cohorts, including a session that is both inert and evidence-bearing (proving the counts are non-exclusive and may sum above `sessions_with_types`)                                                                                                                                                                               |
| `the_report_type_set_is_capped`                                                                  | more than `MAX_REPORT_UNRECOGNIZED_TYPES` distinct types across sessions ⇒ set capped, `types_truncated` true, and the retained set alphabetical                                                                                                                                                                                                                                                 |
| `an_inert_unknown_session_enters_the_denominator_and_is_assessed`                                | for the named detector: `eligible == assessed == 1`; status `Clean`                                                                                                                                                                                                                                                                                                                              |
| `an_evidence_bearing_unknown_session_is_eligible_but_not_assessed`                               | `eligible == 1`, `assessed == 0`, status `NotAssessed(IncompleteEvidence)`                                                                                                                                                                                                                                                                                                                       |
| `a_capped_unknown_session_is_eligible_but_not_assessed`                                          | seventeen distinct inert types ⇒ `Partial(CapExceeded)`, `assessed == 0`, and `capped_sessions == 1` in the summary                                                                                                                                                                                                                                                                              |
| **new** `a_session_of_only_inert_unknowns_is_a_zero_work_session` (D2)                           | Pins today's arithmetic explicitly: the session is in the cohort, is `eligible` for the seven capability-satisfied detectors, is counted in `counts.assessed`, and reads `Clean`; `UnusedMcpServers` / `UnusedSkills` are excluded by `in_denominator`. The test's doc comment states that this already happens for allowlisted housekeeping-only sessions today and points at the §14 follow-up |
| `tests/unrecognized_records.rs::an_inert_unknown_session_enters_the_denominator_and_is_assessed` | Streams the inert fixture, asserts denominator/status, and derives all three badges from the resulting evidence so classification regressions fail the test                                                                                                                                                                                                                                      |
| `badges.rs::session_wide_partial_coverage_diverges_from_report_by_design` (existing)             | unchanged                                                                                                                                                                                                                                                                                                                                                                                        |

### 10.6 Desktop shell

| Test                                                                                | Change                                                                                                                                                                                                                                                                                                               |
| ----------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `dto.rs::the_report_payload_serializes_camel_case_counts_and_nothing_else`          | Insert `"unrecognizedRecords"` into the pinned alphabetical top-key list, **between `quotaPressure` and `windowEndEpoch`**; assert the nested key set and that the payload carries no session identifier. The local `report()` helper builds through `EfficiencyReportAccumulator::finish`, so it compiles unchanged |
| **new** `dto.rs::unrecognized_types_survive_dto_conversion`                         | Engine summary → payload, including the truncated flag, the capped count, and the alphabetical `Vec<String>` ordering                                                                                                                                                                                                |
| `analytics/event.rs::no_variant_escapes_the_catalog`                                | `7 → 8` plus the new match arm                                                                                                                                                                                                                                                                                       |
| `analytics/event.rs::the_documented_catalog_matches_the_code`                       | Passes once the name appears in `docs/analytics.md`. Remember it checks nothing else (§6.4)                                                                                                                                                                                                                          |
| **new** `analytics/mod.rs::an_unrecognized_records_report_becomes_a_bucketed_event` | `inert_only` + correct bucket for an inert-only cohort; `inert_capped` when only capping blocked; `evidence_bearing` when any session failed closed (winning over a simultaneous cap); no runtime string reaches `Facts`                                                                                             |
| **new** `analytics/mod.rs::only_a_changed_unrecognized_outcome_is_worth_an_event`   | Mirrors `only_a_changed_scan_outcome_is_worth_an_event`, including the clean-cohort `("none","none")` outcome and the return trip out of it                                                                                                                                                                          |
| **new** `analytics/mod.rs::withdrawing_consent_clears_every_suppression_hint`       | `reset_suppression()` clears `LAST_SCAN` **and** `LAST_UNRECOGNIZED`, so opting out and back in reports the first outcome the reader consented to. Tested against `reset_suppression` directly, because `handle_settings_transition` needs an `AppHandle` that this crate cannot construct                           |
| `analytics/mod.rs::a_clean_checkout_cannot_transmit` (existing)                     | Unchanged; it is the consent-gate floor for the new event too                                                                                                                                                                                                                                                        |

**Consent-test limitation, stated deliberately.** `src-tauri` has no Tauri mock
harness (`tauri::test` is not enabled), so no test can construct an `AppHandle`
with a `Store` and assert "nothing is queued while the switch is off". The gate
is enforced structurally instead: the only queue writer is
`analytics::record`, whose first statement is `allowed(app)`, and the new call
site lives inside `analytics/mod.rs`. Enabling `tauri`'s `test` feature to add
an end-to-end consent test is recorded as a follow-up (§14).

### 10.7 Desktop frontend

| Test                                                                  | Change                                                                                                                                                                                                                             |
| --------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `InsightsPane.test.tsx` helper `report()`                             | Add the new field. The helper is annotated `InsightsReportPayload`, so `type-check` catches an omission                                                                                                                            |
| `InsightsPane.portability.test.tsx`                                   | **Runtime break, not cosmetic.** Its `invoke.mockImplementation` returns a bare object literal with no type annotation, so a missing `unrecognizedRecords` fails only when the note dereferences it. Add the field to that literal |
| **new** `unrecognized record types are named in the coverage section` | Types render, the "N of M sessions in the assessed cohort" population phrase renders, and the note appears **even when every category is assessed**; the note sits outside the coverage `<ul>`                                     |
| **new** `an evidence-bearing unknown is called out`                   | The evidence-bearing sentence renders only when `evidenceBearingSessions > 0`, and the word "assessed" is not used for the withheld result                                                                                         |
| **new** `a capped session is called out`                              | The cap sentence renders when `cappedSessions > 0` and `evidenceBearingSessions == 0`; both render when both are positive                                                                                                          |
| **new** `a truncated type list says so`                               | ` and more` appears when `typesTruncated`                                                                                                                                                                                          |
| **new** `a record with no type reads as words, not a placeholder`     | `<missing>` renders as "records with no type"                                                                                                                                                                                      |
| **new** `one session reads in the singular`                           | `sessionsWithTypes === 1` uses "session", matching the pane's existing ternary style                                                                                                                                               |
| **new** `a very long type name does not break the note`               | A 256-character discriminator renders inside the footnote without a layout assertion beyond the existing class contract                                                                                                            |
| `SettingsView.test.tsx`                                               | No change; it constructs no report                                                                                                                                                                                                 |

---

## 11. Risks and mitigations

| Risk                                                                                                       | Mitigation                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| ---------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| The predicate drifts from `parse_record` and silently starts dropping evidence                             | The table supplies behavioral coverage. `parse_record_changes_require_an_inertness_review` fingerprints the parser and evidence-helper block, so edits force an explicit predicate/table review. The comment above `parse_record` names both obligations                                                                                                                                                                                                                                   |
| Inverting the allowlist makes a real `attachment` / `summary` / `file-history-snapshot` record fail closed | The predicate's shapes are narrow, and the two recognized-eventless predicate tests (§10.1) plus the unchanged `housekeeping_records` fixture pin every shipped shape as inert. A genuine evidence-bearing attachment _should_ fail closed — that is rule 2                                                                                                                                                                                                                                |
| A session of only inert unknowns reads `Clean` with no work done                                           | Not new (allowlisted housekeeping-only sessions already do), explicitly pinned by `a_session_of_only_inert_unknowns_is_a_zero_work_session`, and filed as a follow-up rather than silently inherited (D2, §14)                                                                                                                                                                                                                                                                             |
| A future vendor adapter reaches the unknown path without the predicate                                     | Predicate lives in shared `jsonl.rs`; the Codex work in flight (PR #243) adopts it rather than copying. Noted in the follow-ups entry                                                                                                                                                                                                                                                                                                                                                      |
| Visible dip in `assessedSessions` and badges between ship and requeue                                      | Expected and honest (rows are `stale`, never wrong). Called out in the PR description; the changelog says results and badges refresh as sessions are re-read                                                                                                                                                                                                                                                                                                                               |
| Cohort-wide discriminator strings reaching the UI look like new data exposure                              | Bounded schema vocabulary with existing precedent (`affectedModels`); documented in the DTO doc comment and in the PR's privacy section; never sent off-device                                                                                                                                                                                                                                                                                                                             |
| An analytics slot spent on an event that cannot name a type                                                | The standing-policy comment and implementation task explicitly require this closed-vocabulary signal. Sampling bias is documented so nobody reads the bucket as a population rate                                                                                                                                                                                                                                                                                                          |
| `aislop` `complexity/file-too-large` on touched files                                                      | **Measured, not assumed:** `npx aislop ci crates/antiburn-local` on `8156a6e` reports `errors: 0, warnings: 0` across 83 files, with `evidence_sink.rs` at 1819 lines — the 1500-line limit is not firing on Rust files at these sizes today. The new integration test file stays as a **hygiene** decision (keeping a 1290-line test file from growing), not as a gate workaround. `jsonl.rs` (1248 lines) is the file this change grows most; step 0a re-measures before the work starts |
| The structural walk costs time on a pathological record                                                    | It runs only on the unknown path. The existing 8 MiB framing limit bounds its work. An iterative walk avoids recursion depth failure                                                                                                                                                                                                                                                                                                                                                       |
| Analytics event fires repeatedly as the pane recomputes                                                    | `(label, bucket)` suppression mirroring `record_scan`, checked after the consent gate, cleared on withdrawal                                                                                                                                                                                                                                                                                                                                                                               |
| Merge friction with PRs #243 / #246 / #247, which touch adapters and evidence                              | Land the engine commits early; the shared predicate and the `inert` field are additive; coordinate the `PARSER_REVISION` / `EVIDENCE_SCHEMA_REVISION` values with whichever lands first (they are single-line constants)                                                                                                                                                                                                                                                                   |
| Residual invisibility                                                                                      | Accepted: `coverage_reasons` stays dropped in `From<EfficiencyReport>`, so losses **other than** unrecognized types (malformed, oversized, pinned prefix) remain behind the generic `incompleteEvidence` string. This ticket surfaces the unrecognized-type and cap paths only                                                                                                                                                                                                             |

---

## 12. Validation commands

```bash
# Engine (standalone workspace)
cd crates/antiburn-local
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test

# Goldens, in this order: add fixtures → generate → stage → prove nothing moved.
# (New fixtures are untracked, so `git diff --exit-code` is meaningless until
#  they are staged. The new fixtures carry no golden by D10, so the only
#  expected diff is none at all.)
git add tests/fixtures
UPDATE_GOLDENS=1 cargo test --test claude_characterization
git diff --exit-code tests/fixtures

# Desktop shell (standalone workspace)
cd apps/desktop/src-tauri
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test

# Desktop frontend (repository root)
pnpm install
pnpm --filter @antiburn/desktop lint
pnpm --filter @antiburn/desktop type-check
pnpm --filter @antiburn/desktop test
pnpm --filter @antiburn/desktop build

# Repository gates
pnpm run slop:all    # CONTRIBUTING requires this before pushing
pnpm run slop        # the changed-files variant PR CI runs
pnpm run secrets
node scripts/check-design-drift.mjs   # expected no-op: no token or stylesheet change
# `pnpm run notices:check` is not needed: no dependency changes.
```

Every authored commit uses `git commit -s` (DCO).

---

## 13. Ordered implementation checklist

Each step is a self-contained commit that keeps the workspace green.

0. **Before any code.**
   - **0a.** Run `pnpm run slop:all` on a scratch commit to record the current
     baseline (expected: 0 errors) so any later finding is attributable.
   - **0b.** Confirm that the standing-policy comment and implementation task
     require the closed-vocabulary analytics event while keeping runtime type
     names local.
1. **Predicate.** Add `is_inert_unrecognized`, the in-code reader/exemption
   table, the parser-source fingerprint, and the doc comment to `jsonl.rs`.
   Add the §10.1 unit tests. No caller yet, so no behavior change.
2. **Observation carries `inert`.** Add the field to
   `EvidenceObservation::UnrecognizedType`; invert `claude.rs` per §3;
   `evidence_sink` counts the inert path; add
   `ParseDiagnostics::records_unrecognized_inert` and the struct doc comment.
   Add the §10.2 tests.
3. **Revisions.** `PARSER_REVISION` 3→4, `EVIDENCE_SCHEMA_REVISION` 2→3; update
   the pinned serialization tests and the desktop revision assertions (§10.3).
4. **Fixtures and integration tests.** The four new fixtures, the new
   `tests/support/claude_fixture.rs` and `tests/unrecognized_records.rs`, the
   `fixture()` / `evidence_fixture_names()` additions, the flipped
   `src/analysis/tests.rs` expectation, the corpus knob **and** tally, the
   renamed corpus test, and the fixture README rows (§10.4).
5. **Report summary and badges.** `UnrecognizedRecords` on `EfficiencyReport`
   and its accumulation, with caps and `capped_sessions`; the §10.5 tests
   including the badge test and the zero-work-session pin;
   `insights/mod.rs` re-exports; detector module doc.
6. **DTO.** `InsightsUnrecognizedRecordsPayload`, the `From` conversion, the
   updated wire-shape key list, and the new DTO test (§10.6).
7. **Frontend.** IPC types, the `UnrecognizedRecordsNote` in `CoverageSection`
   (outside the coverage list), the `<missing>` mapping, and the §10.7 tests
   including the portability fixture.
8. **Analytics** _(gated on step 0b)_. `EventName::UnrecognizedRecordsObserved`,
   `EVERY_EVENT`, the facts/suppression functions and `reset_suppression` in
   `analytics/mod.rs`, the `handle_settings_transition` change, the
   `commands.rs` call site, the `docs/analytics.md` catalog row plus the
   "why not the names", sampling-bias, and "three of those" sentences, and the
   §10.6 analytics tests.
9. **Documentation.** Architecture-reference paragraph, followups entries
   (§14), and the fixture README coverage-matrix note if not already done in
   step 4.
10. **Changelogs.** Root `CHANGELOG.md` (creating `### Changed` under
    `## [Unreleased]`) and `crates/antiburn-local/CHANGELOG.md` (§9).
11. **Full validation** (§12), then open the PR describing user impact, tests,
    the privacy effect of the new event and the new DTO field, the expected
    requeue dip, the badge flip, and the preserved cap behavior.

---

## 14. Out of scope — follow-ups to append to `docs/plans/local-insights-followups.md`

Entries use the document's five-field shape.

**Runtime unknown-type discriminators in analytics**

- **What was found:** The catalog event for unrecognized records cannot name a
  type, because `Properties::label` is a closed `&'static str` vocabulary.
- **Found by seam:** antiburn#229.
- **Why deferred:** Sending runtime strings is a payload-type change and needs
  its own privacy review. Until then, new vocabulary is discovered from the
  local diagnostic, the Insights coverage note, and support threads.
- **Kind:** `enhancement`
- **Disposition:** `file-issue` after the first
  `antiburn.unrecognized_records_observed` data arrives.

**Per-group degradation on record loss**

- **What was found:** `supported_value` degrades every _supported_ top-level
  group on any record loss, while the architecture reference describes
  degrading only the affected group.
- **Found by seam:** antiburn#229.
- **Why deferred:** Narrowing it changes verdicts for malformed, oversized, and
  pinned-prefix losses too, so it needs its own blast-radius analysis and test
  matrix.
- **Kind:** `enhancement`
- **Disposition:** `file-issue` with its own test matrix.

**Zero-work sessions in detector denominators**

- **What was found:** A session with no assistant turns still enters `eligible`
  and `assessed` for seven detectors and can read `Clean`. `in_denominator`
  excludes zero-work sessions for `UnusedMcpServers` and `UnusedSkills` only.
  antiburn#229 widened the set of sessions that reach this state from sixteen
  allowlisted names to any structurally inert unknown type.
- **Found by seam:** antiburn#229.
- **Why deferred:** Widening the rule to every detector changes today's
  housekeeping-only sessions as well, which is a separate verdict change.
- **Kind:** `enhancement`
- **Disposition:** `file-issue` with the detector-by-detector denominator rule.

**End-to-end analytics consent test**

- **What was found:** `src-tauri` has no Tauri mock harness, so consent gating
  is proven structurally rather than by an integration test.
- **Found by seam:** antiburn#229.
- **Why deferred:** Enabling `tauri`'s `test` feature is its own build and
  dependency change.
- **Kind:** `enhancement`
- **Disposition:** `file-issue` so one test can assert that nothing is queued
  while the switch is off.

---

## 15. Plan review record

### 15.1 Review passes

| Pass                                             | Scope                                                                                               | Outcome                                                                             |
| ------------------------------------------------ | --------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------- |
| 1 — authoring                                    | Design options A/B/C against the issue thread and `main`                                            | Option C adopted; first draft of this document                                      |
| 2 — independent review A (architecture / policy) | Control flow, FR compliance, analytics slot, missing plumbing                                       | 4 blockers, 4 product-policy items, 6 omissions, 6 factual corrections              |
| 3 — independent review B (test / release)        | Test matrix, revisions, changelogs, validation commands                                             | 6 blockers, 8 test gaps, 6 release items, 4 validation items, 4 factual corrections |
| 4 — reconciliation (this pass)                   | Every point re-checked against `main` @ `8156a6e` by reading the cited symbols and running `aislop` | Corrections applied below; three claims rejected with evidence                      |

### 15.2 Accepted corrections

| #   | Correction                                                                                                                                                                                                                                                                              | Where applied                                                                                                                                           | Verification performed                                                                                                                                                                                                                                                                      |
| --- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1   | The allowlist still gated the verdict, so an allowlisted name carrying evidence could fail open. Invert: predicate decides, allowlist only silences the observation for an already-inert record                                                                                         | §2.1 rule 2/3, §3, §4.1 (`claude.rs`), §10.1, §10.4                                                                                                     | Read `visit_reader`: both emissions are inside `if !is_recognized_eventless`. Read every line of `housekeeping_records.jsonl` and the `attachment` lines of `mcp_and_skill_sources.jsonl` / `records_all_kinds.jsonl`: all are structurally inert, so the inversion keeps those tests green |
| 2   | A session of only inert unknowns could read `Clean` with no work done                                                                                                                                                                                                                   | D2 in §2.4, §10.5 test, §14 follow-up                                                                                                                   | Read `observe_session` and `detectors::in_denominator` / `status`: only `UnusedMcpServers` / `UnusedSkills` exclude zero-work sessions. Confirmed the same is already true today for allowlisted housekeeping-only sessions, so this is pinned rather than introduced                       |
| 3   | The parser-mirror guard's exemption list lived only in prose; `isSidechain` and the `function_call` `payload.name` + `arguments` split were missing                                                                                                                                     | §2.3 (both tables), §10.1 guard is now one in-code `const` table with `expected_inert`                                                                  | Read `parse_record`, `resolve_role`, `process_content`, `push_named_tool(_str)`, `evidence_observations`                                                                                                                                                                                    |
| 4   | `<missing>` discriminators would render raw; non-object records were undecided                                                                                                                                                                                                          | D3 in §2.4, §7.2, §10.1, §10.7                                                                                                                          | Read `record_discriminator` (`unwrap_or("<missing>")`) and `parse_record`'s `value.as_object()?`                                                                                                                                                                                            |
| 5   | Pane copy misused "assessed" (FR-12 cohort vs category) and sat inside the exclusive-denominator `<ul>`                                                                                                                                                                                 | §7.2, §4.3                                                                                                                                              | Read `CoverageSection`: `COVERAGE_ROWS` is iterated as `keyof InsightsCoveragePayload` with `coverage[key] > 0`, two elements below "{n} in the assessed cohort"                                                                                                                            |
| 6   | The analytics event does not transmit runtime type names; sampling bias and the "joins an in-flight reduction" nuance were unstated                                                                                                                                                     | §6 preamble, §6.1, §13 step 0b                                                                                                                          | The standing-policy comment supplies the closed-vocabulary decision. Read `get_insights_report` and `InsightsController::report` for sampling behavior                                                                                                                                      |
| 7   | §6.4 overstated `the_documented_catalog_matches_the_code`                                                                                                                                                                                                                               | §6.4, §10.6                                                                                                                                             | Read the test: it asserts `doc.contains(name.as_str())` only                                                                                                                                                                                                                                |
| 8   | Cap-exceeded sessions stayed invisible, and the "still assessed" copy would have been false for them                                                                                                                                                                                    | D5 in §2.4, `capped_sessions` in §4.1/§4.2/§7.1/§7.2, tests in §10.5/§10.7                                                                              | Read the `UnrecognizedType` cap arm, `note_collection_cap`, and `cap_string`: both paths key on the field name `"diagnostics.unrecognized_types"`                                                                                                                                           |
| 9   | `LAST_UNRECOGNIZED` was never cleared on consent withdrawal                                                                                                                                                                                                                             | §4.2 `reset_suppression`, §6.3, §10.6 test                                                                                                              | Read `handle_settings_transition`, which clears `LAST_SCAN` with a comment forbidding exactly this residue                                                                                                                                                                                  |
| 10  | The badge flip is user-visible and was listed as "no change" with no test                                                                                                                                                                                                               | §4.1 `badges.rs` row, §10.5 badge test, §9 changelogs                                                                                                   | Read `badges.rs`: the rule requires `evidence.coverage == EvidenceCoverage::Complete`                                                                                                                                                                                                       |
| 11  | The "enters the denominator" fixture could pass for the wrong reason                                                                                                                                                                                                                    | §10.4 (`unrecognized_inert_records.jsonl` carries real assistant turns; the asserted detector excludes `OverpoweredSubagents` and `UnusedBuiltInTools`) | Read `SourceCapabilities::claude` (`tool_definitions: false`) and `overpowered_subagents::evaluate`                                                                                                                                                                                         |
| 12  | `evidence_coverage_is_complete_for_every_clean_fixture` newly exercises `unrecognized_type`                                                                                                                                                                                             | §10.4                                                                                                                                                   | Read the test: it `continue`s on `RecordCoverage::Partial`, which that fixture stops being                                                                                                                                                                                                  |
| 13  | Fixture plumbing (fixed-size arrays, two `include_str!` matches, golden requirement, helper duplication) was unbudgeted                                                                                                                                                                 | D10 in §10.4, §13 step 4                                                                                                                                | Read `fixture_names() -> [&str; 15]` (the golden set), `evidence_fixture_names() -> [&str; 18]`, `fixture()`, `src/analysis/tests.rs::input`, and `pipeline_corpus.rs`'s `#[path] mod corpus` pattern                                                                                       |
| 14  | The corpus needed a second tally, not just a spec knob                                                                                                                                                                                                                                  | §10.4 last row                                                                                                                                          | Read `Tallies::unrecognized_records` and its assertion in `pipeline_corpus.rs`                                                                                                                                                                                                              |
| 15  | `records_observed` gains a third meaning with no doc comment                                                                                                                                                                                                                            | §4.1 (`evidence.rs` row), §8                                                                                                                            | Read `SessionEvidenceAccumulator::observe`: only `MetricsEvent` and `Unusable` increment it; `ParseDiagnostics` has no doc comment                                                                                                                                                          |
| 16  | Follow-ups did not match the document's five-field shape                                                                                                                                                                                                                                | §14 rewritten                                                                                                                                           | Read `docs/plans/local-insights-followups.md`                                                                                                                                                                                                                                               |
| 17  | `diagnostics_json` was unmentioned in the D8 argument                                                                                                                                                                                                                                   | §5                                                                                                                                                      | Read `insights_worker.rs` (writes it) and confirmed only `store/tests.rs` reads it, as a string                                                                                                                                                                                             |
| 18  | The engine changelog understated the API break (no `#[non_exhaustive]`); the root changelog must create `### Changed` and name badges                                                                                                                                                   | §9                                                                                                                                                      | Read both changelogs and the type definitions                                                                                                                                                                                                                                               |
| 19  | Analytics semantics: bucket `0` unreachable, bucket meaning ambiguous, capped-inert cohort indistinguishable, and the clean-cohort `None` left a stale suppression value                                                                                                                | §6.1 (three-value vocabulary), §6.3 (`("none","none")`), §10.6                                                                                          | Read `event::bucket`, `record_scan`, `scan_outcome_is_new`                                                                                                                                                                                                                                  |
| 20  | `docs/analytics.md`'s "Two of those…" sentence goes stale and nothing enforces it                                                                                                                                                                                                       | §6.4, §13 step 8                                                                                                                                        | Read the sentence under the catalog table                                                                                                                                                                                                                                                   |
| 21  | Validation: `slop:all` is the pre-push gate; the goldens command needs an ordering note; `notices:check` is not needed                                                                                                                                                                  | §12                                                                                                                                                     | Read `CONTRIBUTING.md` and `package.json` scripts                                                                                                                                                                                                                                           |
| 22  | Predicate edge cases (`message`/`content` of the wrong type, `tool_calls: []`, `usage: {}` / `null`, empty `name` with `input`) needed decided answers, not just tests                                                                                                                  | §2.3 exemption table, §10.1                                                                                                                             | Read `process_content`, `has_tool_result_block`, `push_named_tool_str`                                                                                                                                                                                                                      |
| 23  | The `isSidechain` exemption makes `OverpoweredSubagents::ContractIncomplete` newly reachable                                                                                                                                                                                            | §2.3 exemption table, §10.4 `unrecognized_inert_sidechain.jsonl`                                                                                        | Read the `DelegatedTurn` arm (`if *is_sidechain`) and `overpowered_subagents::evaluate`                                                                                                                                                                                                     |
| 24  | The existing cap tests need only the new field, not a redesign; the "seventeenth distinct discriminator" framing needed precision (a truncated _first_ value fires the cap too); the DTO top-key insertion point and `types` ordering needed pinning; the §3 diagram omitted `Box::new` | §1.4, §3, §10.2, §10.6                                                                                                                                  | Read both cap tests, the `UnrecognizedType` arm, and the pinned top-key list (alphabetical, so `unrecognizedRecords` sits between `quotaPressure` and `windowEndEpoch`)                                                                                                                     |
| 25  | `InsightsPane.portability.test.tsx` is a runtime break, not a cosmetic fixture update                                                                                                                                                                                                   | §10.7                                                                                                                                                   | Read the file: its `invoke.mockImplementation` returns an unannotated object literal, unlike `InsightsPane.test.tsx`'s typed `report()`                                                                                                                                                     |
| 26  | `unrecognized_type_without_role_is_dropped_but_with_role_is_kept` should be named as deliberately unchanged                                                                                                                                                                             | §10.4                                                                                                                                                   | Read the fixture: line 3 carries `message.role: assistant` and parses today                                                                                                                                                                                                                 |

### 15.3 Rejected suggestions, with evidence

| Suggestion                                                                                                                          | Verdict                                       | Evidence                                                                                                                                                                                                                                                                                                                                   |
| ----------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| "`aislop` `complexity/file-too-large` is an error and `evidence_sink.rs` at 1819 lines is at risk" (review A)                       | **Rejected as a gate risk; kept as hygiene.** | `npx aislop ci crates/antiburn-local` on `8156a6e` returns `{errors: 0, warnings: 0, files: 83}`; the same on `src/analysis` returns 0 with `evidence_sink.rs` present. The 1500-line limit is not firing on these Rust files. The new test file stays for readability, and step 0a re-measures. Review B's independent measurement agrees |
| "There are three `EvidenceObservation::UnrecognizedType` construction sites in `evidence_sink.rs` (~1366, ~1692, ~1721)" (review B) | **Rejected.**                                 | `rg 'UnrecognizedType' src/analysis/evidence_sink.rs` returns exactly three hits: the production match arm at 411 (a pattern, not a construction) and two test constructions at 1366 and 1692. Line 1721 is `fn thread_record`. §10.2 says two, correctly                                                                                  |
| "The local `report()` helper in `dto.rs` tests constructs `EfficiencyReport` literally, so the new field breaks it" (review B)      | **Rejected.**                                 | `dto.rs::report()` calls `EfficiencyReportAccumulator::new().finish(ReportContext { … })`. `EfficiencyReport { … }` has exactly one literal site in the workspace: `report.rs::finish`. `insights_ipc.rs::empty_report` also uses `finish`. Only the pinned key list needs editing                                                         |
| "§2.1 rule 3 is a doc-comment-only change; say so or reviewers will look for a test that cannot exist" (review B, E2)               | **Rejected as written.**                      | True only under the _old_ ordering. Under the accepted inversion (correction 1), rule 3 changes behavior for one real case — an allowlisted name carrying evidence-bearing shapes — and §10.4's `unrecognized_evidence_shapes.jsonl` last line is exactly the test that "cannot exist" under the rejected framing                          |
| "Filter `<missing>` out of the report-level type set" (review A, one of two offered options)                                        | **Partially rejected.**                       | Dropping it would make the engine diagnostic less auditable than the local diagnostic it summarizes, for a case that is real (`parse_record` returns `None` for any non-object line). The DTO stays faithful and the **pane** maps the string to words (D3)                                                                                |
| "Add a third suppressed-event value via `detail` for the cap dimension" (review B, C5-iii, one of two offered options)              | **Partially rejected.**                       | `detail` stays `None` (§6.1): a second dimension multiplies the cross-product of an event whose slot is already under question. The cap case is carried by a third `label` value, `inert_capped`, which answers the question §2.4 raises without widening the payload                                                                      |

### 15.4 Final decisions

1. **Predicate decides, allowlist filters noise.** Fail-closed is uniform across
   all record types, including the sixteen eventless names.
2. **Inert unknowns stay auditable and surfaced:** `records_unrecognized_inert`,
   the retained discriminator set, the report summary, and the pane note —
   including when nothing was blocked.
3. **Caps are preserved and now visible.** A capped session still degrades, and
   the pane says why.
4. **Zero-work sessions are pinned, not changed.** The pre-existing arithmetic
   is documented, tested, and filed as a follow-up.
5. **Analytics is closed-vocabulary, bucketed, consent-gated, and suppression-reset
   on withdrawal.** The standing-policy comment supersedes the draft request to
   transmit runtime type names.
6. **No golden moves and no metrics change**, by construction:
   `SessionMetricsAccumulator` ignores `Observation` and `Unusable`.
7. **Revisions bump to `PARSER_REVISION = 4` / `EVIDENCE_SCHEMA_REVISION = 3`**
   with lazy requeue, no migration, and no `serde(default)`.
8. **No material question is left open.** Every question raised in either review
   is answered in §2.4 (D1–D8), §6, §7, §10.4 (D10), or §14.

---

## 16. Implementation record

**Implemented:** 2026-08-28 on `feat/issue-229-best-effort-unrecognized-records`.
No commit or pull request was created.

### 16.1 What shipped in the working tree

- Added the structurally bounded inertness predicate beside `parse_record`, with
  fail-closed role, usage, model, effort, tool, compaction, non-object, and
  command-marker handling. The 8 MiB record frame is the scan bound.
- Demoted the eventless allowlist to an inert-diagnostic noise filter. Added
  inert audit counts, retained discriminators, and parser/evidence revisions 4
  and 3.
- Added the bounded cohort summary, desktop DTO, Insights coverage note,
  `<missing>` wording, cap visibility, and report/badge behavior tests.
- Added the closed-vocabulary, bucketed analytics event. The shared analytics
  gate checks consent before suppression state changes, and withdrawal clears
  both suppression hints. Runtime discriminator strings remain local.
- Added synthetic policy fixtures, corpus controls, parser/sink/report/DTO/UI/
  analytics tests, architecture and analytics documentation, followups, and
  both changelog entries.

### 16.2 Details resolved against the implementation

1. The report reducer determines assessment from required group states, not from
   `SessionEvidence.coverage` alone. Before this change, a discriminator cap
   made session coverage partial but could still leave detector groups complete.
   The unknown-discriminator cap now records `CapExceeded` as the group loss
   reason. A later evidence-bearing loss replaces that cap reason, preserving
   the required `UnrecognizedRecordType` precedence. This makes the planned
   capped-session denominator/status test true without changing unrelated caps.
2. Current `main` has eight capability-satisfied Claude detectors when work is
   present, not the seven stated in the planning snapshot. For a zero-work inert
   session, two absence detectors apply their denominator exclusion and the
   built-in-tools detector lacks capability, leaving six assessed detector
   denominators. The integration test pins the current matrix.
3. The implementation task explicitly required the consent-gated analytics
   contract, so step 8 was implemented. No runtime discriminator reaches the
   event payload.
4. Golden verification compared only the tracked `goldens/` directory. This
   avoided staging files and preserved unrelated untracked files.

### 16.3 Validation results

| Validation                                                   | Result                                                                                                                          |
| ------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------- |
| Baseline `pnpm run slop:all`                                 | Passed before edits: score 100, 0 errors, 0 warnings across 392 files.                                                          |
| Engine `cargo fmt --check`                                   | Passed.                                                                                                                         |
| Engine `cargo clippy --all-targets -- -D warnings`           | Passed.                                                                                                                         |
| Engine `cargo test`                                          | Passed after verification fixes: library 899 passed, 1 ignored; integration suites 63, 9, 8, 1, and 8 passed; doctest 1 passed. |
| `UPDATE_GOLDENS=1 cargo test --test claude_characterization` | Passed: 63 tests. `git diff --exit-code -- tests/fixtures/claude_characterization/goldens` passed; no golden changed.           |
| Shell `cargo fmt --check`                                    | Passed.                                                                                                                         |
| Shell `cargo clippy --all-targets -- -D warnings`            | Passed.                                                                                                                         |
| Shell `cargo test`                                           | Passed: 570 tests; no failures or ignored tests.                                                                                |
| `pnpm install --frozen-lockfile`                             | Passed: workspace dependencies were already up to date.                                                                         |
| Desktop `pnpm --filter @antiburn/desktop format`             | Passed after formatting the two changed Insights files.                                                                         |
| Desktop lint                                                 | Passed.                                                                                                                         |
| Desktop type check                                           | Passed.                                                                                                                         |
| Desktop tests                                                | Passed after verification fixes: 88 files, 904 tests.                                                                           |
| Desktop build                                                | Passed. Vite emitted the existing chunk-size advisory for the settings bundle.                                                  |
| `pnpm run slop:all`                                          | Passed after fixes: score 100, 0 errors, 0 warnings across 392 files.                                                           |
| `pnpm run slop`                                              | Passed: score 100, 0 errors, 0 warnings.                                                                                        |
| `pnpm exec aislop ci crates/antiburn-local/tests`            | Passed: score 100 across 7 supported test files, including the new untracked integration helpers.                               |
| `pnpm run secrets`                                           | Passed.                                                                                                                         |
| `node scripts/check-design-drift.mjs`                        | Passed: the design contract is in sync.                                                                                         |
| `git diff --check`                                           | Passed.                                                                                                                         |

The first post-change slop run found one trivial analytics comment. The comment
was removed, and both slop gates then passed with no findings.

---

## 17. Verification review and fix record

### 17.1 Review/fix matrix

| Verification finding                                                                                  | Verdict                                             | Resolution                                                                                                                                                                                                                                                                              |
| ----------------------------------------------------------------------------------------------------- | --------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Discriminator caps changed detector-group semantics while the plan said the behavior was unchanged    | **Valid**                                           | §2.4 and §4.1 now record the deliberate change: discriminator caps set `record_loss_reason(CapExceeded)` so supported groups become partial. Comments explain both cap sites and the weaker-cap precedence. Other session-scope caps keep their prior coverage-only behavior.           |
| `INERTNESS_MIRROR_CASES` alone did not detect parser drift                                            | **Valid**                                           | The table is now described as behavioral coverage. `parse_record_changes_require_an_inertness_review` fingerprints `parse_record` and its evidence-shape helper block. The predicate and parser comments cross-reference the table and review obligation.                               |
| The badge test mutated diagnostics that badge code never reads                                        | **Valid**                                           | Removed the tautological unit test. The inert-fixture integration test now streams Claude input and derives all three badges from the classified evidence.                                                                                                                              |
| The pane used singular pronouns for plural blocked-session counts and tested the wrong singular count | **Valid**                                           | Copy selects `it` or `them` from each blocked count. Tests cover plural blocked counts, cap-only output, and `sessionsWithTypes = 1` within a twelve-session cohort.                                                                                                                    |
| An empty report resets unknown-outcome suppression and permits a later repeat                         | **Not a defect**                                    | This is the documented `unknown → clean → unknown` transition. Remembering the clean outcome makes the later regression observable instead of suppressing it with stale state.                                                                                                          |
| Unknown-outcome suppression is not keyed by environment                                               | **Not currently applicable**                        | `get_insights_report` constructs only the native-scope request. There is no alternating environment call site. The analytics event also deliberately carries no environment dimension.                                                                                                  |
| The analytics deviation lacked a separate issue comment                                               | **Not required after the standing-policy decision** | The later standing-policy comment explicitly requires one closed-vocabulary event now and defers runtime names. This task repeats that exact requirement. §6 and §13 no longer claim another sign-off is pending.                                                                       |
| A node/depth scan budget made large or deep allowlisted housekeeping records fail closed              | **Valid**                                           | Removed the independent 512-node and depth-six limits. The iterative scan is bounded by the existing 8 MiB record frame. Unit and adapter tests cover a 500-entry snapshot and depth-eight summary with complete coverage and no unknown diagnostics. Discriminator caps are unchanged. |
| Analytics used an unnamed `(label, bucket)` tuple and accepted the whole report                       | **Valid**                                           | Added named `UnrecognizedOutcome` fields, convert them to `Facts`, assert the named `Facts` fields, and accept only `&UnrecognizedRecords`.                                                                                                                                             |
| Pane wording said a type was unrecognized even for allowlisted evidence-bearing records               | **Valid**                                           | The lead now says antiburn “could not read” the record types.                                                                                                                                                                                                                           |
| The root changelog omitted the new analytics event                                                    | **Valid**                                           | Added a reader-facing privacy entry with the event name, bucketed/fixed vocabulary, consent context, and local-only type names.                                                                                                                                                         |
| Analytics suppression state and cap precedence lacked code documentation                              | **Valid**                                           | Added the in-memory suppression comment, cap-site comments, precedence comment, and architecture text for the discriminator-cap asymmetry.                                                                                                                                              |
| The analytics catalog row was ragged and the withdrawal test depended on prior test cleanup           | **Valid**                                           | Formatting is normalized with the surrounding Markdown table, and the withdrawal test calls `reset_suppression()` before its first assertion.                                                                                                                                           |
| Unknown `<command-name>` records silently lost a synthesized late-tool signal                         | **Valid**                                           | Claude now treats an unknown record containing a command marker as evidence-bearing and fails closed. The adapter integration test includes a real synthetic skill marker and command.                                                                                                  |
| Nested role/usage/model shapes were inert and the depth policy was undocumented                       | **Valid as a conservative hardening**               | The iterative scan now rejects evidence keys at any depth. The table, unit tests, and policy text pin the conservative behavior even where `parse_record` currently reads only top-level or `message` locations.                                                                        |
| `<missing>`, non-object, and real-path cap coverage was incomplete                                    | **Valid**                                           | Adapter integration covers an inert object with no discriminator and a failed-closed non-object record. The existing real adapter cap test remains. Large/deep records now have adapter coverage rather than an artificial budget-exhaustion path.                                      |
| The cap-only pane branch had no isolated test                                                         | **Valid**                                           | Added a test with `cappedSessions > 0` and `evidenceBearingSessions = 0`.                                                                                                                                                                                                               |
| `ParseDiagnostics` said all allowlisted eventless records were excluded                               | **Valid**                                           | The comment now states that only structurally inert allowlisted records are excluded. Evidence-bearing allowlisted records count through `Unusable`.                                                                                                                                    |
| Corpus knob collision precedence was undocumented                                                     | **Valid**                                           | Both `SessionSpec` field comments state that evidence-bearing insertion runs first and wins matching indexes.                                                                                                                                                                           |
| `*nodes == MAX` was brittle                                                                           | **Obsolete**                                        | The independent node budget no longer exists. The framing bound limits the scan.                                                                                                                                                                                                        |
| Zero-work clean arithmetic and report-level type truncation caveats                                   | **Accepted design limits**                          | Existing tests and follow-ups retain these decisions. The report-level `typesTruncated` field remains best-effort, while `cappedSessions` explains session-local capping.                                                                                                               |

### 17.2 Verification validation results

| Validation                                         | Exact post-fix result                                                                                                                                                                       |
| -------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Engine `cargo fmt --check`                         | Passed.                                                                                                                                                                                     |
| Engine `cargo clippy --all-targets -- -D warnings` | Passed with no warnings.                                                                                                                                                                    |
| Engine `cargo test`                                | Passed: library 899 passed and 1 ignored; `claude_characterization` 63; `pipeline_corpus` 9; `source_validity_timing` 8; `streaming_metrics_memory` 1; `unrecognized_records` 8; doctest 1. |
| Golden regeneration                                | `UPDATE_GOLDENS=1 cargo test --test claude_characterization` passed all 63 tests. `git diff --exit-code -- tests/fixtures/claude_characterization/goldens` passed; no golden changed.       |
| Shell `cargo fmt --check`                          | Passed.                                                                                                                                                                                     |
| Shell `cargo clippy --all-targets -- -D warnings`  | Passed with no warnings.                                                                                                                                                                    |
| Shell `cargo test`                                 | Passed: 570 tests.                                                                                                                                                                          |
| Desktop format                                     | Prettier check passed.                                                                                                                                                                      |
| Desktop lint                                       | ESLint passed.                                                                                                                                                                              |
| Desktop type check                                 | `tsc --build --force` passed.                                                                                                                                                               |
| Desktop tests                                      | Passed: 88 files, 904 tests.                                                                                                                                                                |
| Desktop build                                      | Passed. Vite emitted only the existing settings chunk-size advisory.                                                                                                                        |
| Desktop unused-code check                          | `knip --no-progress` passed.                                                                                                                                                                |
| Design contract                                    | `node scripts/check-design-drift.mjs` passed: design contract is in sync.                                                                                                                   |
| Repository slop checks                             | `pnpm run slop:all` and `pnpm run slop` both scored 100 with 0 errors and 0 warnings across 392 supported files.                                                                            |
| Secret scan                                        | `pnpm run secrets` passed.                                                                                                                                                                  |
| Patch hygiene                                      | `git diff --check` passed.                                                                                                                                                                  |

No commit, issue comment, pull request, or PR description was created.

---

## 18. Final implementation gate

**Gate:** Passed on 2026-08-28 with `openai-codex/gpt-5.6-sol`.

The gate re-read the complete working-tree diff, the issue body, the fact-check,
the standing-policy comment, the later badge/report note, and the final
verification report. It found no valid defect and made no implementation or
test changes. The only gate edit is this result record.

The gate independently re-ran every relevant repository check listed in
`CONTRIBUTING.md` and `apps/desktop/README.md`. Engine formatting, Clippy,
tests, and golden regeneration passed with no golden movement. Shell formatting,
Clippy, and all 570 tests passed. Desktop formatting, lint, type checks, all 904
tests, build, and `knip` passed; the build emitted only the existing chunk-size
advisory. Both slop checks scored 100 with no findings, the test-source slop
check scored 100, and the secret scan, design-drift check, and patch-hygiene
check passed. `notices:check` was not applicable because no dependency manifest
or lockfile changed.

The working tree is ready for a separate final code-review workflow. No commit,
issue comment, pull request, or PR description was created.

---

## 19. Final pr-code-review workflow

**Role:** conservative deduplication and confidence gate over five parallel
review agents (`claudemd`, `bug`, `history`, `prior-pr`, `code-comment`).
**Scope:** `git diff HEAD` plus every implementation-related untracked file on
`feat/issue-229-best-effort-unrecognized-records`, excluding `.agents/` and
`.worktrees/`. No production code was edited, no commit, push, PR, or GitHub
comment was made.

**Gate method.** The gate read the diff itself rather than trusting the
reports. It reproduced the four behavioral claims with a temporary probe test
(`crates/antiburn-local/tests/zzz_review_probe.rs`, since deleted), compared
each claimed regression against `git show main:…` for the same input, and
re-ran `cargo fmt --check` and `cargo test` in `crates/antiburn-local`
(989 tests, all green, formatting clean).

**Score meaning.** 0 false positive or pre-existing · 25 uncertain · 50 verified
but rare or minor · 75 highly confident and practically important · 100 certain
and frequent. **Tiers:** <5 drop · 5–14 INFO · 15–39 MIGHT FIX · 40–79 SHOULD
FIX · 80–100 MUST FIX.

**Verdict:** no MUST FIX. Three SHOULD FIX findings, all in the same policy
seam: the allowlist demotion (§2.1 rule 3) turns two previously-silent record
shapes into session-wide coverage loss, and the cap counter conflates
truncation with set overflow.

### 19.1 Final finding ledger

#### 1 — SHOULD FIX · score 70 · category `bug`

**`<command-name>` veto cancels the eventless allowlist and re-creates the #222 regression**

- **Files / changed lines:** `crates/antiburn-local/src/analysis/vendors/claude.rs:163-166`
  (`let inert = is_inert_unrecognized(&value) && !has_command_name;` and the
  following `if !inert || !is_recognized_eventless(&value)`), reading
  `has_command_name` from the unchanged raw-substring test at `claude.rs:153`.
- **Proof (reproduced).** A three-line synthetic session — a real `user` turn
  carrying `<command-name>/review</command-name>`, a `last-prompt` record whose
  `prompt` echoes the same marker, and a real `assistant` turn with usage —
  yields on this branch:
  `coverage = Partial(UnrecognizedRecordType)`, `unrecognized_types = {"last-prompt"}`,
  `records_unusable = 1`. On `main` (`git show main:crates/antiburn-local/src/analysis/vendors/claude.rs:163`
  — both emissions sit inside `if !is_recognized_eventless(&value)`, and
  `last-prompt` is one of the sixteen allowlisted names) the same session is
  `Complete` with empty diagnostics.
  The marker carries no evidence on either branch: `state.pending_commands` is
  pushed only in the parsed-event path (`claude.rs:184-189`), so an unparsed
  record's marker is discarded on `main` too. The veto therefore changes nothing
  for non-allowlisted records (they already failed closed) and only ever costs
  coverage for allowlisted ones.
- **User impact.** Every session whose housekeeping record echoes prompt text
  containing a slash command loses assessment for up to seven detector
  categories, drops out of `counts.assessed`, sends all three session badges to
  `NotAssessed(IncompleteEvidence)`, and publishes the allowlisted name in the
  pane's "record types antiburn could not read" list. That is the exact field
  failure #222 / PR #231 fixed, inside the ticket that exists to remove it.
  No shipped test covers it: `tests/unrecognized_records.rs:161-179`
  (`an_unknown_command_marker_fails_closed`) uses the **non**-allowlisted
  `slash_v2`, whose verdict is identical on `main`.
- **Fix.** Let a proven-inert allowlisted record keep its silence, at
  `claude.rs:163-166`:

  ```rust
  let structurally_inert = is_inert_unrecognized(&value);
  let allowlisted = is_recognized_eventless(&value);
  // A command marker creates a late tool call only for a parsed event.
  // A known eventless name never owns one, so the marker is not evidence there.
  let inert = structurally_inert && (allowlisted || !has_command_name);
  if !inert || !allowlisted { /* observation */ }
  if !inert { /* Unusable */ }
  ```

  Add a regression test in `tests/unrecognized_records.rs`: allowlisted name +
  `<command-name>` + a real assistant turn ⇒ `EvidenceCoverage::Complete` and
  empty `unrecognized_types`. Update the §2.3 bullet and
  `docs/plans/local-insights-architecture.md:585` to state the narrowed rule.

#### 2 — SHOULD FIX · score 45 · category `bug`

**The any-depth evidence-key scan is applied to allowlisted names, so keys the parser never reads degrade whole sessions**

- **Files / changed lines:** `crates/antiburn-local/src/analysis/vendors/jsonl.rs:349-397`
  (the `["role","usage","model","speed","effort","reasoning_effort","tool_calls","compactMetadata"]`
  test at `:358-370` and the `has_split_named_tool` test at `:377-389`, both
  applied at every depth), reached through the inverted branch at
  `crates/antiburn-local/src/analysis/vendors/claude.rs:165-166`.
- **Proof (reproduced).** A two-line synthetic session — a Claude `attachment`
  record whose payload nests `attachment.config.model` (an MCP-instruction blob),
  plus a real assistant turn — yields `coverage = Partial(UnrecognizedRecordType)`
  with `unrecognized_types = {"attachment"}`. `parse_record` reads
  `role`/`usage`/`model`/`speed`/`effort`/`reasoning_effort`/`tool_calls` only at
  the top level and under `message` (`jsonl.rs:466-522`), so this record could
  never have contributed evidence. On `main` the same session is `Complete`.
  The plan's D1 verification checked only the synthetic fixture lines, whose
  payloads are scalars, so no shipped test can catch this.
- **Distinct from finding 1.** Different trigger (nested key, not a marker),
  different fix surface (`jsonl.rs`, not `claude.rs`); neither fix resolves the
  other. Both share the root cause: §2.1 rule 3 removed the allowlist's veto
  over `Unusable`, so any over-firing of the predicate now costs coverage that
  `main` kept.
- **User impact.** `attachment` is the widest structured-JSON surface Claude
  Code writes (MCP resources, IDE diagnostics, todo lists, nested memory), so an
  incidental `model` / `role` / `name`+`input` pair inside one is plausible.
  Such a session loses all detector assessment and all three badges, exactly as
  in finding 1. The over-conservatism is harmless for genuinely new vocabulary —
  there it only preserves `main`'s verdict — so the cost falls entirely on the
  sixteen known names.
- **Fix.** Choose one. (a) Match `parse_record`'s real read depth for the seven
  scalar evidence keys — top level and `message` only — and keep the any-depth
  scan for content-block types and tool shapes; update `INERTNESS_MIRROR_CASES`
  (`jsonl.rs:411-446`) and the §2.3 table rows that say "deeper locations fail
  closed conservatively". (b) Keep the any-depth scan but let
  `is_recognized_eventless` suppress the `Unusable` when the only trigger sits
  below the depths `parse_record` reads, preserving §2.1 rule 2 for the shallow
  `message.usage` case the plan actually names. Either way add a fixture line
  with a nested-but-unread key under an allowlisted name.

#### 3 — SHOULD FIX · score 45 · category `bug`

**A single over-long discriminator is counted and rendered as "more unrecognised types than antiburn records"**

- **Files / changed lines:** `crates/antiburn-local/src/insights/report.rs:395-404`
  (`capped` folds `capped_collections` and `truncated_strings` into one
  `capped_sessions` counter); rendered at
  `apps/desktop/src/views/settings/InsightsPane.tsx:333-338`; the truncation
  trigger is the new `set_record_loss_reason(CoverageReason::CapExceeded)` at
  `crates/antiburn-local/src/analysis/evidence_sink.rs:430-435`.
- **Proof (reproduced).** One unknown record whose `type` is 300 bytes plus one
  valid assistant turn yields `coverage = Partial(CapExceeded)` and
  `UnrecognizedRecords { types: {<one 256-byte string>}, types_truncated: false,
sessions_with_types: 1, inert_sessions: 1, evidence_bearing_sessions: 0,
capped_sessions: 1 }`. The session contained **one** unrecognized type, yet
  the pane states the opposite, and because `types_truncated` is `false` it also
  prints the silently truncated 256-byte string as if it were the whole name.
- **User impact.** This sentence is the only explanation the reader gets for why
  the session left the assessed set, and for the truncation path it is factually
  false. The engine-side coverage is correct; only the summary and the copy are
  wrong.
- **Fix.** Split the counter in `report.rs:395-404`: keep `capped_sessions` for
  `capped_collections` and add `truncated_sessions` for `truncated_strings`;
  carry both through `InsightsUnrecognizedRecordsPayload`
  (`apps/desktop/src-tauri/src/dto.rs:475-481`) and give each its own sentence in
  `UnrecognizedRecordsNote`. Extend
  `report.rs::unrecognized_records_summarizes_the_cohort` with a truncation-only
  session and add the pane test. If one counter must stay, neutralise the copy:
  "…contained unrecognised record types antiburn could not record in full, so
  some checks cannot report a result for {it|them}."

#### 4 — MIGHT FIX · score 35 · category `bug`

**Session-level type-set overflow is invisible in the pane's type list**

- **Files / changed lines:** `crates/antiburn-local/src/insights/report.rs:406-414`
  (`types_truncated` is set only by the report-level cap
  `MAX_REPORT_UNRECOGNIZED_TYPES`); consumed at
  `apps/desktop/src/views/settings/InsightsPane.tsx:313-316`.
- **Proof (reproduced).** The session built by
  `tests/unrecognized_records.rs:190-219` (17 distinct inert types) produces
  `records_unrecognized_inert = 17`, `unrecognized_types.len() = 16`, and
  `capped_collections = {"diagnostics.unrecognized_types"}`. Fed to
  `EfficiencyReportAccumulator`, the summary is
  `types_truncated: false, capped_sessions: 1`.
- **User impact.** The lead sentence claims to name the record types antiburn
  could not read while omitting an unbounded number of them, with no " and more"
  marker. Partly mitigated by the cap sentence, which does fire for this session.
- **Fix.** In `observe_unrecognized_records`, after computing `capped`
  (`report.rs:395-403`), also set
  `self.unrecognized_records.types_truncated |= diagnostics.capped_collections.contains(UNRECOGNIZED_TYPES_DIAGNOSTIC);`
  and extend `the_report_type_set_is_capped` with a session-cap case.

#### 5 — SHOULD FIX · score 40 · category `code-comment`

**The new `parse_record` mirror invariant states "and" where the policy is "or", and is false as written**

- **File / changed lines:** `crates/antiburn-local/src/analysis/vendors/jsonl.rs:458-460`.
- **Proof.** The comment says "Every field read here must appear in
  `is_inert_unrecognized` **and** `INERTNESS_MIRROR_CASES`." `parse_record` reads
  `isSidechain` (`:467`), `timestamp`/`ts`/`created_at`/`createdAt` (`:475-479`),
  `uuid`/`parentUuid` (`:519-520`), `message.id` (`:525`), and `content`
  (`:568-570`). None of them appears in the predicate's key list (`:352-361`);
  each is an `expected_inert = true` row in `INERTNESS_MIRROR_CASES`
  (`:411-446`). The real policy is §2.3: each read is **either** rejected by the
  predicate **or** listed as a deliberate exemption.
- **User impact.** A maintainer applying the comment literally would add
  `timestamp` / `uuid` / `content` to the predicate's reject list, which makes
  every housekeeping record fail closed and silently reverts this entire ticket.
  The comment is the primary durable guard on the predicate, so its wording is
  load-bearing.
- **Fix.** Replace with two sentences: `/// Every read here is a rejected key in`
  `/// is_inert_unrecognized or an exempt row in INERTNESS_MIRROR_CASES.` and
  `/// A new read needs one of the two before you update the fingerprint.`

#### 6 — MIGHT FIX · score 35 · category `code-comment`

**The fingerprint tripwire the mirror comment cites does not cover `parse_usage` or `parse_ts`**

- **Files / changed lines:** `crates/antiburn-local/src/analysis/vendors/jsonl.rs:459-460`
  (the "Update the parser-source fingerprint after review" clause) and the new
  test `parse_record_changes_require_an_inertness_review` at `:848-861`.
- **Proof (measured).** The window is `source.find("pub fn parse_record")` →
  `find("/// Parse usage while")`, i.e. source lines 461–765 (11 479 bytes),
  ending inside `extract_skill_name_from_command`. `parse_usage` starts at line
  768, `as_u64` at 807, `parse_ts` at 812, and `thread_identity_field` at 300 —
  all outside. Plan §2.3 names `parse_usage` as one of the readers the predicate
  must mirror (`usage`, `speed`).
- **User impact.** A future change to `parse_usage` that adds a read the
  predicate does not know about passes CI silently, which is precisely the drift
  the tripwire promises to catch.
- **Fix.** Extend `end` to a marker after `parse_ts` (for example
  `source[start..].find("#[cfg(test)]")`) and re-record the constant, or narrow
  the promise: `/// The fingerprint covers parse_record and its tool helpers.`
  `/// Review parse_usage and parse_ts by hand.`

#### 7 — MIGHT FIX · score 30 · category `code-comment`

**The cap comment is duplicated verbatim six lines apart and misdescribes its first branch**

- **File / changed lines:** `crates/antiburn-local/src/analysis/evidence_sink.rs:432-433`
  and `:439-440` (identical two-line block).
- **Proof.** At `:432` the enclosing branch is
  `if discriminator.len() != original_len` — `cap_string`
  (`analysis/evidence.rs:437-450`) truncated **one** over-long discriminator.
  Nothing about the _set_ is capped there; that is the `:439` branch
  (`unrecognized_types.len() == MAX_UNRECOGNIZED_TYPES`). The shared wording
  erases the truncation-vs-overflow distinction the plan draws in §1.4 and that
  finding 3 shows the UI already gets wrong. Both copies are visible on one
  screen, so the second states nothing the first has not shown
  (`AGENTS.md` → Comments: "Add a comment only when it states important
  information the code cannot show").
- **User impact.** Maintainer confusion in exactly the code path that produces
  the false pane sentence in finding 3.
- **Fix.** One accurate comment per branch, in the active voice:
  `:432` → `// A truncated discriminator no longer identifies the record format.`
  `// Treat the truncation as record loss so no supported group reports complete.`
  `:439` → `// A capped discriminator set means antiburn no longer understands the format.`

#### 8 — MIGHT FIX · score 30 · category `history`

**The architecture reference now contradicts itself about the invariant this change inverts**

- **Files / lines:** `docs/plans/local-insights-architecture.md:1140` (CH-001)
  and `:1143` (CH-004), left unchanged, versus `:585` which this diff rewrote and
  `crates/antiburn-local/src/analysis/tests.rs:194-198`, which this diff flipped
  from `RecordCoverage::Partial` / `{UnrecognizedRecordType}` to
  `RecordCoverage::Complete` / `{}`.
- **Proof.** CH-004's acceptance still reads "**an unrecognized record `type`
  degrades the affected evidence group to `Partial` with a reason and never
  leaves it `Complete`**, proven against CH-001's unrecognized-`type` fixture",
  and CH-001 still reads "asserting `Partial` and **not** `Complete` against this
  same fixture is CH-004's acceptance". Both clauses are now false for the exact
  fixture they name, while line 585 of the same document was updated by this
  diff — so the document is internally inconsistent rather than merely historical.
- **User impact.** A future seam auditing CH-004 reads the acceptance criterion
  and restores the old behavior.
- **Fix.** Append to both clauses, matching how `:585` was treated: "superseded
  by antiburn#229 (structural inertness); the fixture now asserts `Complete`."

#### 9 — MIGHT FIX · score 25 · category `code-comment`

**The rewritten architecture sentence presents an incomplete fail-closed list**

- **File / changed line:** `docs/plans/local-insights-architecture.md:585`
  ("A present but unrecognized role, any evidence-bearing shape, or a non-object
  record fails closed.").
- **Proof.** `claude.rs:165` adds a fourth trigger the sentence omits:
  `is_inert_unrecognized(&value) && !has_command_name`, where
  `has_command_name` is a raw substring test over the whole record
  (`claude.rs:153`). Finding 1 shows this trigger is the one that can still
  degrade a _named housekeeping_ session, and it is documented nowhere outside
  the local doc comment at `jsonl.rs:347`.
- **User impact.** The reference document that this diff deliberately rewrote is
  the durable home of the policy; a reader deriving behavior from it gets the
  wrong answer for the one case that regresses.
- **Fix.** Resolve finding 1 first, then state the surviving rule in the same
  sentence. If the veto is kept as-is: "A Claude record containing a
  `<command-name>` marker also fails closed, because a late tool call needs a
  parsed event to own it."

#### 10 — MIGHT FIX · score 25 · category `code-comment`

**The `ParseDiagnostics` header excludes exactly the records its new field counts**

- **File / changed lines:** `crates/antiburn-local/src/analysis/evidence.rs:391-394`
  (added), introducing `records_unrecognized_inert` at `:400`.
- **Proof.** The header says "Bounded diagnostics for records that contributed
  evidence **or** reduced coverage." `records_unrecognized_inert` counts records
  that by construction did neither: `evidence_sink.rs:415-423` increments it with
  no paired `Unusable`, and `an_inert_unrecognized_record_keeps_complete_coverage`
  (`evidence_sink.rs:1381-1400`) asserts `EvidenceCoverage::Complete`.
- **User impact.** The scope sentence contradicts the change it introduces, and
  it is the only place the three meanings of `records_observed` are written down.
  "contributed" and "reduced" are also past tense, against `AGENTS.md:39`.
- **Fix.** `/// Bounded diagnostics for observed records, including inert unknown`
  `/// records that keep coverage complete.`

#### 11 — MIGHT FIX · score 22 · category `code-comment`

**The DTO privacy comment says "256 characters"; the engine caps 256 bytes**

- **File / changed line:** `apps/desktop/src-tauri/src/dto.rs:472`.
- **Proof.** `cap_string` (`crates/antiburn-local/src/analysis/evidence.rs:442`)
  computes `let mut end = value.len().min(EVIDENCE_STRING_CAP);` — `str::len()`
  is bytes — then walks back to a char boundary. A non-ASCII discriminator is
  bounded at ≤256 **bytes**, i.e. fewer than 256 characters. The probe in
  finding 3 measured a truncated length of exactly 256 for an ASCII input.
- **User impact.** This comment carries the privacy argument across the IPC
  boundary, so its unit should be the one the code enforces.
- **Fix.** `/// The engine limits each value to 256 bytes and each report to 16 values.`

#### 12 — MIGHT FIX · score 15 · category `prior-pr`

**PR #242's deferral pointer is stranded once #229 closes**

- **File / lines:** `crates/antiburn-local/src/insights/badges.rs:264-268`
  (deliberately unchanged), against the decision recorded only at
  `docs/plans/issue-229-best-effort-unrecognized-records.md:261` ("Divergence
  rule **unchanged**").
- **Proof.** The comment reads "This asymmetry is intentional. Issue #229 tracks
  the report-wide coverage policy." Commit `6320ca7` deferred that question to
  #229 explicitly. This change does **not** adopt the badge's session-wide rule
  for the report; it only removes the question for inert unknowns. When #229
  closes, the pointer names a closed issue and the deferral is lost.
- **User impact.** Documentation debt in the one comment PR #242 left for the
  next reader. Scored low because the line is outside the diff and no behavior
  depends on it.
- **Fix.** Update the comment to state the decision ("the report keeps the
  detector-scope rule; antiburn#229 did not change it") **or** add a fifth entry
  to `docs/plans/local-insights-followups.md` in the existing five-field shape
  and cite it from the comment.

#### 13 — MIGHT FIX · score 18 · category `prior-pr`

**The followups ledger claims #229 shipped, and the four new entries drop the ledger's status-note convention**

- **File / changed lines:** `docs/plans/local-insights-followups.md:27` and the
  four new entries at `:29-59`.
- **Proof.** Line 27 asserts "The structural best-effort policy for genuinely
  unknown record types **shipped through antiburn#229**", but #229 is an open
  issue and no commit or PR exists yet — unlike the accurate adjacent clause
  "Filed as antiburn#222 and shipped in PR antiburn#231." All thirteen
  pre-existing entries close their `Disposition` with a status note
  (`**Filed as antiburn#221.**`, `**No issue yet:** …`, `**Folded into
antiburn#224**`, `**Resolved:** …`); the four new dispositions at `:35`,
  `:43`, `:51`, `:59` end at a bare `file-issue`. PR #235 (issue #230) closed
  this ledger specifically so it carries real issue numbers instead of
  placeholders.
- **User impact.** A reader of the ledger cannot tell whether the four new items
  are tracked anywhere.
- **Fix.** Add `**No issue yet:** …` notes to the four dispositions (or file the
  issues), and reword line 27 to name the PR once it exists.

#### 14 — MIGHT FIX · score 15 · category `history`

**The #224 pipeline baseline was not re-measured after the eventless path gained a full record walk**

- **Files / changed lines:** `crates/antiburn-local/src/analysis/vendors/jsonl.rs:349-400`,
  reached from `crates/antiburn-local/src/analysis/vendors/claude.rs:165`.
- **Proof.** On `main`, an allowlisted record cost one `type` string comparison.
  It now costs an iterative walk over every node with a `Vec` of node references.
  The bench corpus emits eventless housekeeping records at spec indexes
  `92..=99` (`tests/support/corpus.rs:348-397`), so `pipeline_baseline` covers
  the path, and `crates/antiburn-local/benches/BASELINE.md:59-65` is the record
  #224 used to dismiss further optimization. The plan's validation tables
  (§16.3, §17.2) list no bench run.
- **User impact.** Low. The walk is bounded by the already-parsed
  `serde_json::Value`, so parsing still dominates; the ask is a cheap
  confirmation, not a suspected regression.
- **Fix.** Run `cargo bench --bench pipeline_baseline` before the final commit
  and either confirm no movement or add one line to `BASELINE.md`.

#### 15 — INFO · score 12 · category `code-comment`

**The consent rationale deleted from `handle_settings_transition` survives nowhere**

- **File / changed lines:** `apps/desktop/src-tauri/src/analytics/mod.rs:407`
  (replaces three deleted lines), the new doc at `:278`, and the new early-return
  path at `:396-397`.
- **Proof.** The removed comment carried the _why_: "…so opting back in starts
  from a clean comparison rather than silently suppressing the first scan it
  would otherwise report." Plan §6.3 treats that property as a consent
  requirement and `withdrawing_consent_clears_every_suppression_hint`
  (`analytics/mod.rs:679-696`) enforces it, but no comment now states it. The
  surviving line, `/// Clears in-memory suppression hints after consent
withdrawal.`, restates what `reset_suppression()` already shows by name
  (`AGENTS.md:44`). The new `reset_suppression()` on the missing-`Store` path at
  `:396-397` — the one path that clears suppression without clearing the queue —
  is uncommented.
- **User impact.** The rationale that stops a future refactor from
  reintroducing a consent regression is now only implicit in a test name.
- **Fix.** On `reset_suppression`'s doc: add
  `/// A stale hint would suppress the first event the reader consented to.`

#### 16 — INFO · score 10 · category `claudemd`

**Passive voice and past tense in new comments**

- **Files / changed lines:**
  `crates/antiburn-local/src/analysis/vendors/jsonl.rs:347`
  ("Claude command markers **are checked** separately…");
  `crates/antiburn-local/src/analysis/evidence_sink.rs:432` and `:439`
  ("the record format **is not understood**");
  `apps/desktop/src-tauri/src/dto.rs:473` ("its diagnostic marker **is
  bounded**"); `crates/antiburn-local/src/insights/report.rs:250` ("the
  diagnostic marker set **is also bounded**");
  `crates/antiburn-local/src/analysis/evidence.rs:391` ("**contributed**",
  "**reduced**" — past tense).
- **Proof.** `AGENTS.md:38-39` → Comments: "Use the active voice and present
  tense." The rest of the new comment surface complies, so this is a localised
  slip.
- **User impact.** Repo-rule consistency only.
- **Fix.** Name the actor: "The Claude adapter checks command markers
  separately…"; "…means antiburn no longer understands the record format";
  "…because the engine bounds the diagnostic marker set". Findings 7 and 10
  already rewrite three of these sites.

#### 17 — INFO · score 10 · category `prior-pr`

**The issue's "the analytics event fires only with consent" acceptance bullet has no test**

- **File / changed lines:** `apps/desktop/src-tauri/src/analytics/mod.rs:236-247`;
  new tests at `:643-702`.
- **Proof.** `record_unrecognized_records` calls `allowed(app)` before touching
  suppression state (`:236-239`), consistent with `record_scan:193-197`, and
  `record()` re-gates — but the gate is proven structurally, not by a test.
  Plan §10.6 states the limitation and §14 files the follow-up ("End-to-end
  analytics consent test", `docs/plans/local-insights-followups.md:53-59`), and
  every pre-existing event has the same gap.
- **User impact.** None at runtime; an unmet acceptance bullet.
- **Fix.** Name it in the PR description as a known deviation, citing the
  followups entry.

### 19.2 Rejected-findings ledger

| #   | Reported by / claim                                                                                                                                                                                                                                         | Score | Reason for rejection                                                                                                                                                                                                                                                                                                                                                             |
| --- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| R1  | agent 4 `PRIOR-1` — the new `set_record_loss_reason(CapExceeded)` at `evidence_sink.rs:430-442` makes every supported group `Partial`, so a 17-distinct-type session loses all detector results                                                             | 0     | Intent-only disagreement with a documented decision. §2.4 "Discriminator-cap behavior" states the change and its justification, D5 requires the reader-visible signal that depends on it, and `a_capped_unknown_session_is_eligible_but_not_assessed` pins it. The one substantive consequence — the counter and the pane copy conflate truncation with overflow — is finding 3. |
| R2  | agent 4 `PRIOR-1` sub-claim — resulting diagnostics are "self-contradictory" (`Partial` groups with `records_unusable == 0`)                                                                                                                                | 0     | `record_loss_reason` is internal state, not a published field. `CapExceeded` already produced `Partial` session coverage on `main`; only group propagation changed, which R1 covers.                                                                                                                                                                                             |
| R3  | agent 4 `PRIOR-1` sub-claim — `session_wide_partial_coverage_diverges_from_report_by_design` "only survives because it sets `evidence.coverage` directly"                                                                                                   | 0     | The test uses `CoverageReason::MalformedRecord` (`badges.rs:272`), not a cap. The plan keeps every other session-scope cap coverage-only, so the divergence stays reachable in production. No defect.                                                                                                                                                                            |
| R4  | agent 1 advisory — `crates/antiburn-local/tests/zz_probe.rs` is untracked, unformatted, and would be swept into `git add -A`                                                                                                                                | 0     | Peer-reviewer scratch, not part of the proposed change; agent 1 scored it as advisory itself. Verified absent from the working tree at gate time (`git status --short`), as is this gate's own probe.                                                                                                                                                                            |
| R5  | agent 2 finding 1 vs agent 3 `H1` vs agent 5 `F2` — the `<command-name>` veto                                                                                                                                                                               | —     | Deduplicated into finding 1 (behavior) and finding 9 (the doc sentence). `H1`'s wider framing ("the #222 remediation path is removed") is the shared root cause and is recorded in findings 1 and 2.                                                                                                                                                                             |
| R6  | agent 2 finding 3 vs agent 3 `H1` — the any-depth scan over allowlisted names                                                                                                                                                                               | —     | Deduplicated into finding 2. Kept separate from finding 1: different trigger, different file, neither fix resolves the other.                                                                                                                                                                                                                                                    |
| R7  | agent 1 `CMD-2` vs agent 5 `F4` — the duplicated cap comment                                                                                                                                                                                                | —     | Literally identical; merged into finding 7.                                                                                                                                                                                                                                                                                                                                      |
| R8  | agent 3 `H2` vs agent 4 `PRIOR-2` — the stranded `badges.rs` deferral pointer                                                                                                                                                                               | —     | Literally identical; merged into finding 12.                                                                                                                                                                                                                                                                                                                                     |
| R9  | agent 5 "out of my category" note on `InsightsPane.tsx:325-328`                                                                                                                                                                                             | —     | Same defect as agent 2 finding 2; merged into finding 3.                                                                                                                                                                                                                                                                                                                         |
| R10 | agent 1 `CMD-1` tense/voice slips vs agent 5's ASD-STE100 remarks inside `F3` and `F4`                                                                                                                                                                      | —     | Merged into finding 16 (INFO). Style-rule compliance only; no behavior depends on it.                                                                                                                                                                                                                                                                                            |
| R11 | all agents — "checked and clean" sections (goldens unmoved, revision ladder, serde migration gating, privacy scan coverage, concurrency of `LAST_UNRECOGNIZED`, corpus tallies, `evidence_fixture_names()` growth, no `useEffect`, design-token compliance) | —     | Not findings. Independently re-confirmed by this gate where cheap: `cargo fmt --check` clean and 989 engine tests green on the working tree.                                                                                                                                                                                                                                     |

### 19.3 Gate summary

- **MUST FIX:** none.
- **SHOULD FIX:** findings 1, 2, 3, 5.
- **MIGHT FIX:** findings 4, 6, 7, 8, 9, 10, 11, 12, 13, 14.
- **INFO:** findings 15, 16, 17.

Findings 1 and 2 are the material ones: both are behavior regressions against
`main` for the sixteen allowlisted housekeeping names, produced by §2.1 rule 3
removing the allowlist's veto without narrowing the predicate that now decides
in its place. Both are reproducible in under ten lines of fixture. The gate
recommends fixing 1, 2, 3, and 5 before the signed commit, and recording the
rest in the PR description.

No GitHub comment, commit, push, or pull request was created. No production code
was edited by this gate.

### 19.4 Address-findings disposition

**Addressed:** 2026-08-28 with `openai-codex/gpt-5.6-sol`. The owner verified
all 17 surviving findings against the working tree. No rejected finding from
§19.2 changed the implementation.

| #   | Verdict                                    | Disposition                                                                                                                                                                                                                                                                 | Changed files                                                                                                                                                                    | Test or review evidence                                                                                                                                           |
| --- | ------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1   | Valid                                      | Known eventless records now tolerate echoed `<command-name>` markers. Genuinely unknown marker records still fail closed.                                                                                                                                                   | `analysis/vendors/claude.rs`, `tests/unrecognized_records.rs`, `local-insights-architecture.md`, this plan, engine changelog                                                     | `an_allowlisted_command_echo_keeps_complete_coverage` passes; `an_unknown_command_marker_fails_closed` still passes.                                              |
| 2   | Valid                                      | Added a known-eventless predicate that checks scalar evidence keys only at the root and root `message`. The strict predicate remains any-depth for new names. Tool and compaction shapes remain conservative at any depth.                                                  | `analysis/vendors/jsonl.rs`, `analysis/vendors/claude.rs`, `tests/unrecognized_records.rs`, `local-insights-architecture.md`, this plan, engine changelog                        | `an_allowlisted_nested_scalar_key_keeps_complete_coverage` passes; the allowlisted shallow `cost-state` usage fixture still fails closed.                         |
| 3   | Valid                                      | Split set overflow from string truncation. `capped_sessions` now means collection overflow, and `truncated_sessions` identifies an overlong discriminator. The DTO and pane render separate explanations. Analytics treats either limit as `inert_capped`.                  | `insights/report.rs`, `src-tauri/src/dto.rs`, `src-tauri/src/analytics/mod.rs`, `insightsIpc.ts`, `InsightsPane.tsx`, related tests, analytics docs, this plan, engine changelog | `an_overlong_type_is_truncated_without_claiming_set_overflow`, report summary tests, DTO tests, analytics tests, and the pane truncation test pass.               |
| 4   | Valid                                      | A session-local discriminator-set overflow now sets report `types_truncated`, so the pane appends “and more.”                                                                                                                                                               | `insights/report.rs`, `tests/unrecognized_records.rs`, `InsightsPane.test.tsx`, this plan                                                                                        | `the_report_type_set_is_capped` covers report and session caps; `a_capped_unknown_session_is_eligible_but_not_assessed` asserts the flag.                         |
| 5   | Valid                                      | Replaced the false “and” mirror invariant with the rejected-key **or** exempt-row rule.                                                                                                                                                                                     | `analysis/vendors/jsonl.rs`                                                                                                                                                      | The parser fingerprint test and full engine suite pass.                                                                                                           |
| 6   | Valid                                      | Extended the source fingerprint through `parse_usage`, `as_u64`, and `parse_ts`, ending before the test module.                                                                                                                                                             | `analysis/vendors/jsonl.rs`                                                                                                                                                      | `parse_record_changes_require_an_inertness_review` passes with the new fingerprint.                                                                               |
| 7   | Valid                                      | Replaced the duplicate cap comments with separate truncation and set-overflow explanations.                                                                                                                                                                                 | `analysis/evidence_sink.rs`                                                                                                                                                      | Clippy and `pnpm run slop:all` pass with no findings.                                                                                                             |
| 8   | Valid                                      | Marked the CH-001 and CH-004 unknown-type acceptance clauses as superseded by antiburn#229 structural inertness.                                                                                                                                                            | `local-insights-architecture.md`                                                                                                                                                 | Manual cross-reference review confirms both historical clauses now point to the current rule.                                                                     |
| 9   | Valid                                      | Expanded the architecture policy for marker handling and the known-eventless nested-key boundary.                                                                                                                                                                           | `local-insights-architecture.md`, this plan                                                                                                                                      | The two command-marker tests and the nested-key regression test pin the documented branches.                                                                      |
| 10  | Valid                                      | Rewrote `ParseDiagnostics` scope in active, present-tense language and included inert unknown records.                                                                                                                                                                      | `analysis/evidence.rs`                                                                                                                                                           | Clippy and both slop gates pass.                                                                                                                                  |
| 11  | Valid                                      | Corrected the DTO privacy bound from 256 characters to 256 bytes.                                                                                                                                                                                                           | `src-tauri/src/dto.rs`, this plan                                                                                                                                                | Shell Clippy and DTO tests pass.                                                                                                                                  |
| 12  | Valid                                      | Replaced the closing-issue pointer with the adopted decision: the report keeps the detector-scope rule.                                                                                                                                                                     | `insights/badges.rs`                                                                                                                                                             | `session_wide_partial_coverage_diverges_from_report_by_design` passes.                                                                                            |
| 13  | Valid                                      | Changed “shipped” to “implemented, no PR yet” and added `No issue yet` status notes to all four new followups.                                                                                                                                                              | `local-insights-followups.md`                                                                                                                                                    | Manual ledger review confirms each new disposition follows the existing status-note convention.                                                                   |
| 14  | Valid                                      | Re-ran the complete pipeline benchmark and recorded the issue #229 remeasurement.                                                                                                                                                                                           | `benches/BASELINE.md`                                                                                                                                                            | The 10 MiB full reparse median was 72.8 ms versus 72.5 ms in the baseline. Criterion found no change in the metrics-and-evidence stage against the preceding run. |
| 15  | Valid                                      | Restored the consent rationale on `reset_suppression`: stale hints can suppress the first consented event.                                                                                                                                                                  | `src-tauri/src/analytics/mod.rs`                                                                                                                                                 | `withdrawing_consent_clears_every_suppression_hint` and all shell tests pass.                                                                                     |
| 16  | Valid                                      | Rewrote the reported passive or past-tense Rust comments in active, present-tense language. Findings 7 and 10 supply the larger rewrites.                                                                                                                                   | `analysis/vendors/jsonl.rs`, `analysis/evidence_sink.rs`, `analysis/evidence.rs`, `insights/report.rs`, `src-tauri/src/dto.rs`                                                   | `pnpm run slop:all` reports 0 errors and 0 warnings across 392 supported files.                                                                                   |
| 17  | Valid limitation; no code change justified | The shared `allowed(app)` choke point still gates the event before suppression changes, but this workspace cannot construct the required Tauri `AppHandle`. Enabling the Tauri test feature remains separate dependency work. The followup now has an explicit status note. | `local-insights-followups.md`, this plan                                                                                                                                         | Structural review reconfirmed the call order in `record_unrecognized_records`; `a_clean_checkout_cannot_transmit` and shell tests pass.                           |

### 19.5 Address-findings validation

| Validation               | Final result                                                                                                                              |
| ------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------- |
| Engine format and Clippy | `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` passed.                                                               |
| Engine tests             | Library: 899 passed, 1 ignored. Integration suites: 63, 9, 8, 1, and 11 passed. Doctest: 1 passed.                                        |
| Golden regeneration      | All 63 characterization tests passed. `git diff --exit-code -- tests/fixtures/claude_characterization/goldens` passed; no golden changed. |
| Pipeline benchmark       | `cargo bench --bench pipeline_baseline` completed. The issue #229 remeasurement is recorded in `benches/BASELINE.md`.                     |
| Desktop shell            | Format and Clippy passed; all 570 tests passed.                                                                                           |
| Desktop frontend         | Prettier, ESLint, type check, all 905 tests, build, and `knip --no-progress` passed. Vite emitted only the existing chunk-size advisory.  |
| Repository gates         | `pnpm run slop:all` and `pnpm run slop` scored 100 with no findings. Secret scan, design-drift check, and `git diff --check` passed.      |

No commit, push, pull request, or GitHub comment was created.

---

## 20. Re-review remediation

**Remediated:** 2026-08-28 with `openai-codex/gpt-5.6-sol`. The pass read the
complete working-tree change, the full issue discussion, both re-review reports,
and the production call path. No commit, push, pull request, or GitHub comment
was created.

### 20.1 Finding dispositions

| Re-review finding                                                                                                                            | Verdict                   | Resolution                                                                                                                                                                                                                                    | Regression evidence                                                                                                                                                                                                                                                                                 |
| -------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Agent 9 finding 1: the allowlisted predicate had no direct test, while the existing structural test called the strict unknown-name predicate | **Valid**                 | Repointed the sixteen-name test to `is_inert_recognized_eventless`. Added direct coverage for all six scalar keys at the root and root `message`, every any-depth tool and compaction shape, an unread nested scalar key, and a command echo. | `allowlisted_names_use_the_recognized_eventless_predicate` and `recognized_eventless_records_fail_closed_on_every_parser_readable_shape` pass. The tests fail if the relaxed predicate always returns `true`, stops checking root scalar keys, or stops checking nested tool and compaction shapes. |
| Agent 9 finding 2: discriminator truncation did not prove that detector assessment follows the loss reason                                   | **Valid**                 | Extended the real-adapter truncation test to assert detector eligibility, zero assessed sessions, and `NotAssessed(IncompleteEvidence)`.                                                                                                      | `an_overlong_type_is_truncated_without_claiming_set_overflow` passes and now pins the pane sentence's assessment premise.                                                                                                                                                                           |
| Agent 8 final-pass report                                                                                                                    | **No unresolved finding** | No disposition required. Its three informational notes remain non-blocking.                                                                                                                                                                   | Re-read the cited production paths and retained their documented behavior.                                                                                                                                                                                                                          |
| Agent 9 non-blocking observations                                                                                                            | **No release defect**     | No behavior change. The one-session cohort phrase is grammatical; the import and stale-revision observations do not affect behavior; consent integration remains the documented follow-up.                                                    | Full format, lint, type, test, and repository gates pass.                                                                                                                                                                                                                                           |

The root changelog now says that either bounded type limit can block assessment.
This wording covers both discriminator truncation and discriminator-set overflow.

### 20.2 Final validation

| Validation               | Result                                                                                                                                   |
| ------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------- |
| Engine format and Clippy | `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` passed.                                                              |
| Engine tests             | Library: 900 passed, 1 ignored. Integration suites: 63, 9, 8, 1, and 11 passed. Doctest: 1 passed.                                       |
| Golden regeneration      | All 63 characterization tests passed. No tracked golden changed.                                                                         |
| Desktop shell            | Format and Clippy passed; all 570 tests passed.                                                                                          |
| Desktop frontend         | Prettier, ESLint, type check, all 905 tests, build, and `knip --no-progress` passed. Vite emitted only the existing chunk-size advisory. |
| Repository gates         | `pnpm run slop:all` and `pnpm run slop` scored 100 with no findings. Secret scan, design-drift check, and `git diff --check` passed.     |

**Final disposition:** both valid unresolved findings are fixed with regression
coverage. No invalid defect claim required a plan-only disposition, and no
MUST FIX, SHOULD FIX, or MIGHT FIX finding remains from the supplied re-reviews.

### 20.3 Final Opus review disposition

**Observation 1 — addressed.** The inert-only note now says those records do not
themselves block results. It no longer implies results exist when an unrelated
malformed record leaves the same session incomplete. The pane test covers that
combined state.
