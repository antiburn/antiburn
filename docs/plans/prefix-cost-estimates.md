---
title: "Dollar estimates for the three prefix burn checks"
created_at: "2026-09-13"
status: planned
---

# Dollar estimates for the three prefix burn checks

- **Date:** 2026-09-13
- **Issue:** none.
- **Status:** planned. Two PRs, one optional follow-on, built in stacked worktrees.

## Problem

Unused built-in tools (B), Unused MCP servers (M), and Unused skills (K) each
name a definition that a session loaded and never used. That definition is
re-sent, and re-read from cache, on every request in the session:

```
definition_tokens x cache_read_rate[model], summed over every assistant request
```

Today the Burn Checks report and the per-session Hygiene badges name the
resource but never price it. A session with 777 Fable requests and 4,219
Sonnet requests carrying an unused 7,900-token Workflow tool definition is
burning about $8.20 on that tool alone, and nothing in the product says so.

`definition_tokens` is a single per-source, per-session number today (see the
correction below), not itself indexed by model; only the cache-read rate that
prices it varies by which model made the request.

## Decisions

1. **Count every assistant request, including sub-agent (delegated-scope)
   turns, and say so in the user-facing copy.** A delegated worker re-reads
   the same definitions its parent loaded; undercounting understates the
   waste by however much of the session ran as sub-agents.
2. **Price with the live pricing table (`lookup_pricing`,
   `crates/antiburn-local/src/analysis/pricing.rs:141`), installed from
   models.dev by `apps/desktop/src-tauri/src/runtime_pricing.rs`, and stamp
   the pricing revision on the stored figure.** This is the same table the
   session cost card reprices against, so the two numbers reconcile. The
   reviewed model catalogue (`reviewed_pricing`,
   `apps/desktop/src-tauri/src/remediation/target.rs:301`) is hand-maintained
   for the model/reasoning-editor watches and does not cover every model this
   feature must price; it must not be reused here.
3. **PR 1** prices the aggregate Burn Checks report at evidence time and
   renders per-resource dollars in the check detail and a total on the list
   row. **PR 2** adds the three checks to the per-session Hygiene badge set
   with a per-resource dollar line. **PR 3** (optional, cheap) gives the
   cache-churn estimator its two rates from the same lookup instead of `None`.

## Correction to the starting brief

The brief assumed all three checks already carry a `definition_tokens` /
`compatible_requests` shape in `FindingCause` and that
`display_estimate_input` just forgets to fill it in. That is true only for
**B**:

- `FindingCause::UnusedBuiltInTool { tool, tokens: BuiltInToolTokens }`
  (`crates/antiburn-local/src/remediation/findings.rs:52`) carries a token
  count, and `unused_built_in_tools.rs` has a second path —
  `source_assessable` (line 70), `evaluate_with_source_evidence` (line 83),
  `finding_causes_with_source_evidence` (line 139) — that reads the report's
  per-turn `TokenBurnSourceEvidence.replicated_tokens`
  (`crates/antiburn-local/src/insights/report.rs:435`) instead of the flat
  catalogue size. The dispatcher
  (`crates/antiburn-local/src/insights/detectors/mod.rs:444,460`) only
  special-cases `DetectorId::UnusedBuiltInTools` for this path.
- `FindingCause::UnusedMcpServer { server: String }` and
  `FindingCause::UnusedSkill { skill: String }`
  (`crates/antiburn-local/src/remediation/findings.rs:47,55`) carry **no**
  token field. `unused_mcp_servers.rs` and `unused_skills.rs` have only a flat
  `finding_causes(evidence)` — no `_with_source_evidence` variant, no
  `source_assessable`. That is why `display_estimate_input`
  (`apps/desktop/src-tauri/src/remediation/display.rs:257,267`) always passes
  `None` for M and K: there is nothing in the cause to pass.

PR 1 for M and K is therefore "add the field and the source-evidence path
`unused_built_in_tools.rs` already has," generalized rather than copied twice.

## PR 1: aggregate Burn Checks report

### Price the replication loop

`token_burn_evidence` (`apps/desktop/src-tauri/src/insights_report.rs:373`)
reads assistant turns (`TOKEN_BURN_TURNS_SQL`, line 100) and, for each turn
where `scope == "main"` (line 461), calls `observe_main_context` (line 465,
defined line 552) to add each source's `definition_tokens` to a running
`replicated_tokens` once its context reaches the definition size — without
knowing the turn's model.

- Widen the `scope == "main"` guard to include `"delegated"` (Decision 1).
  `TokenBurnTurnAccumulator::observe` already branches on `turn.scope ==
  "delegated"` for other checks, so this is an already-used scope value.
- Pass the turn's `model` (and `speed`) into `observe_main_context`. When
  `definition_tokens` applies, look up pricing the way `report_turn_pricing`
  already does (report.rs:816) and add
  `definition_tokens as f64 * cache_read_cost_per_token` to a new
  `replicated_cost_usd: Option<f64>` on `SourceTokenCounter` (report.rs:366)
  and `TokenBurnSourceEvidence` (report.rs:435). Add
  `pricing_revision: Option<String>`, stamped once per report run as
  `format!("pricing-generation-{}", pricing_generation())` (same shape as
  `remediation/target.rs:78-81`). A turn whose model has no pricing row still
  adds to `replicated_tokens`; only `replicated_cost_usd` stays unaffected —
  token and dollar evidence fail independently.
- `finish_source_counters` (report.rs:582) carries both fields through
  unchanged.

**Amendment (2026-09-13):** verified against `evidence.rs` that a deferred
`ToolDefinition` never adds to `replicated_tokens` or `replicated_cost_usd`.
`source_token_counters`
(`apps/desktop/src-tauri/src/insights_report.rs`) already returns `None` for
an entire source group — the whole `mcp_instructions`/`builtin_tool`/
`skill_instructions` group for that scope, not just the deferred row — when
any source in it has `deferred: true` (`evidence.rs:142`) or an "Other ..."
rollup name. `observe_main_context` then skips that `None` group entirely, so
a deferred tool contributes no tokens and no cost, and it cannot mask a
sibling non-deferred tool's figures either. This guarantee predates PR 1 and
already had a passing test
(`source_estimates_reject_ambiguous_or_deferred_rows`); PR 1 adds
`a_deferred_source_never_adds_tokens_or_cost_for_its_whole_group` to cover it
explicitly for the new cost field.

### Extend M and K to the source-evidence path

- `FindingCause::UnusedMcpServer` and `FindingCause::UnusedSkill` each gain
  `tokens: Option<u128>, cost_usd: Option<f64>`. Neither needs
  `BuiltInToolTokens`'s `Definition`/`Replicated` split — a server or skill is
  always evaluated from `context_sources`, so a plain optional pair covers a
  finding with and without source evidence.
- Add `source_assessable` / `finding_causes_with_source_evidence` to
  `unused_mcp_servers.rs` and `unused_skills.rs`, built the way
  `unused_built_in_tools.rs` does it: read `source_evidence.mcp_sources` /
  `.skill_sources` (populated at report.rs:393-403), filter to
  `replicated_tokens > 0 && !invoked`, emit one cause per source.
- Widen the two `match detector { DetectorId::UnusedBuiltInTools => ..., _ =>
  ... }` dispatches in `detectors/mod.rs` (lines 444, 460) to also route
  `UnusedMcpServers` and `UnusedSkills`.
- The flat `finding_causes` for M and K (used when no source evidence exists)
  keeps returning `tokens: None, cost_usd: None` — the finding still fires,
  just without a dollar figure, like `BuiltInToolTokens::Definition` today.

### Estimator input and `estimate_savings`

`SavingsEstimateInput` (`crates/antiburn-local/src/remediation/estimates.rs:82`)
gains fields on the three prefix variants. Unlike
`CacheRehydrationPriceDifference` (line 105), which prices a *difference*
between two rates, these price one rate against zero — the tokens should
never have been sent. PR 1's evidence already sums per-turn, per-model dollars
into one number (`replicated_cost_usd`), so the estimator input should carry
that pre-summed figure rather than a single `cache_read_rate`: re-deriving one
rate at estimate time would be wrong for a multi-model session, or would need
the same per-model breakdown duplicated a second time.

```rust
McpDefinitionExposure {
    definition_tokens: Option<u64>,
    compatible_requests: Option<u64>,
    replicated_cost_usd: Option<f64>,
    pricing_revision: Option<String>,
},
BuiltInDefinitionReplication {
    replicated_tokens: Option<u64>,
    replicated_cost_usd: Option<f64>,
    pricing_revision: Option<String>,
},
InjectedSkillDocument {
    document_tokens: Option<u64>,
    compatible_requests: Option<u64>,
    replicated_cost_usd: Option<f64>,
    pricing_revision: Option<String>,
},
```

In `estimate_savings` (line 147), each arm keeps its existing
`LiteralInputTokens` result when the rate or revision is missing, and returns
`ApiEquivalentUsd` from `replicated_cost_usd` when both are present, gated by
`valid_revision` (line 250) the same way `WorkerModelPriceDifference` is.

### Wiring and revision

`display_estimate_input` (`apps/desktop/src-tauri/src/remediation/display.rs:252`)
passes the new fields straight through at lines 257, 261-267, 268.
`display_opportunity` (display.rs:170) needs no change — it already sums
`estimate_savings(...).value` and refuses to mix units, so a priced target
reports `ApiEquivalentUsd` and an unpriced one keeps `LiteralInputTokens`.

There is no `FINDINGS_REVISION` / `INSIGHTS_REVISION` constant.
`list_current_findings` recomputes `CurrentFinding` on every call from stored
evidence, gated by `PARSER_REVISION` / `ANALYZER_REVISION` /
`EVIDENCE_SCHEMA_REVISION` / `METRICS_SCHEMA_REVISION`
(`CURRENT_FINDINGS_SQL`, insights_report.rs:110) — none of which change here,
since `SessionEvidence`'s shape is untouched. The in-process `CachedTarget`
cache (`apps/desktop/src-tauri/src/remediation/mod.rs:109`) has its own
10-minute TTL and needs no revision.

What is durable and revision-gated is a watched remediation's stored
estimate: `definition.savings_method_revision == SAVINGS_METHOD_REVISION`
(`remediation/mod.rs:1059`). Since this PR changes what three methods return,
bump `SAVINGS_METHOD_REVISION` from `1` to `2`
(`crates/antiburn-local/src/remediation.rs:18`).

### Frontend

`BurnCheckDisplayFactsPayload` (`apps/desktop/src/lib/insightsIpc.ts:195`)
already carries `estimatedOpportunity` and `quantity`; no payload shape
change, only consumers that currently ignore the fields.

- `BurnCheckTargetDetail.tsx` (line 12) never reads `estimatedOpportunity`.
  Add one line under the recommendation, shown only when
  `target.display.estimatedOpportunity?.unit === "apiEquivalentUsd"`.
  **Amendment (2026-09-13):** the original draft copy, "About $X.XX a session
  in wasted cache reads," overstates precision. `estimatedOpportunity` is
  `display_opportunity` summed over every currently published finding for
  this target (`target.findings`, `remediation/display.rs:163`), one finding
  per session where the resource showed up unused; the sum is all-or-nothing
  across those findings, never a partial-session figure, and
  `occurrenceCount`/`display.observationCount` already reports how many
  sessions it covers (`target.rs`/`dto.rs`, `occurrences: target.findings.len()`).
  There is no day-based rolling window here — `SavingsInterval` in
  `display_cause_opportunity` only gates validity, it does not scale the
  value. Use instead: `~$X.XX in cache reads of this unused definition across
  N sessions, sub-agent requests included.` (reuse `costTotal`'s `~$X.XX`
  formatting from `BurnChecksSavings.tsx:60`; `N` is
  `target.occurrenceCount`, with correct singular/plural "session").
- `CheckRow` (`BurnChecksReport.tsx:56`) renders `presentation.metric` (a
  token-burn percent). Add a smaller line under it summing
  `estimatedOpportunity` across `targets.data.targets`, the way
  `display_opportunity` sums per target, computed in `checkRowPresentation`
  (`apps/desktop/src/views/checks/checkUi.ts:93`) so the main window and
  popover share it, matching the existing shared-formatting contract in
  `docs/plans/burn-check-remediation.md`.
- `CHECK_UI[...].recommendation` copy (checkUi.ts:41-51) is unchanged; the
  dollar figure is a fact line, not a new recommendation.

### Tests

- `report.rs`: two models at different rates on one source summing
  `replicated_cost_usd`; a delegated-scope turn now contributing; a model
  with no pricing row leaving `replicated_tokens` intact.
- `unused_mcp_servers.rs` / `unused_skills.rs`: port
  `unused_built_in_tools.rs`'s source-evidence tests (assessable, not
  assessable, invoked source excluded, zero-token excluded).
- `estimates/tests.rs`: one test per new field combination (both present →
  `ApiEquivalentUsd`; revision missing → falls back to `LiteralInputTokens`).
  `remediation/display.rs`: a cause with `replicated_cost_usd` yields
  `estimated_opportunity` with `ApiEquivalentUsd`.
- Vitest: `BurnCheckTargetDetail` renders the dollar line only when the unit
  is `apiEquivalentUsd`; `checkRowPresentation` sums correctly.
- Golden updates wherever a fixture pins `SAVINGS_METHOD_REVISION`, a
  `SavingsEstimateInput` variant, or a `BurnCheckDisplayFactsPayload` literal
  (`remediation/tests.rs`, `insights_report.rs` fixtures, `dto.rs:2663`-style
  `pricing_revision` literals). `docs/check-coverage.md`: under the M/B/K rows
  (lines 34-36), note that a priced finding needs a resolvable pricing-table
  entry for the turn's model, and that an unpriced session still reports the
  finding without a dollar figure.

## PR 2: per-session Hygiene badges

### Evidence

No per-turn loop is needed. `SessionEvidence` already carries what this needs:

- `ContextSourceEvidence.mcp_servers` / `.skills`:
  `LoadedSource.token_count: Option<u64>`
  (`crates/antiburn-local/src/analysis/evidence.rs:121-133`).
- `ContextSourceEvidence.tool_definitions`: `ToolDefinition.tokens: u32`
  (evidence.rs:139-143).
- `SessionEvidence.models.by_model: BTreeMap<String, ModelTokens>`,
  `ModelTokens.turns: u64` (evidence.rs:163-171) — already inclusive of
  delegated turns per Decision 1.

Same simplification as PR 1: `tool_definitions` resolves once per session for
one model (`builtin_tool_rows`,
`crates/antiburn-local/src/analysis/initial_context.rs:887-902`, takes a
single `model: Option<&str>`), not per model in `by_model`. So a resource's
cost is `token_count * sum over by_model of (turns * cache_read_rate(model))`
— the token count fixed, the rate summed per model.

### Badge and finding-detail shape

`BadgeId` (`crates/antiburn-local/src/insights/badges.rs:9`) gains three
variants — suggested `UnusedMcpServer`, `UnusedBuiltInTool`, `UnusedSkill`,
matching the existing singular style — added to `ALL` (line 19, becomes
`[Self; 9]`) and `detector()` (line 27). `session_badges`'s `[SessionBadge; 6]`
(line 53) and every typed call site become `[SessionBadge; 9]`.

Reuse the existing per-badge detail mechanism instead of inventing one:
`SessionHygieneFindingEvidencePayload`
(`apps/desktop/src-tauri/src/dto.rs:1187`) already carries typed per-badge
evidence built in `finding_evidence()` (dto.rs:1329), the way `ObsoleteModel {
models: Vec<...> }` (dto.rs:1199-1201, 1399-1413) reports one entry per
obsolete model. Add three arms, each a small `Vec` (a session can carry more
than one unused resource):

```rust
UnusedMcpServer { servers: Vec<HygieneUnusedResourcePayload> },
UnusedBuiltInTool { tools: Vec<HygieneUnusedResourcePayload> },
UnusedSkill { skills: Vec<HygieneUnusedResourcePayload> },
```
```rust
pub struct HygieneUnusedResourcePayload {
    pub name: String,
    pub cost_usd: Option<f64>,
}
```

Each arm mirrors `ObsoleteModel`'s builder: filter the relevant
`context_sources` map to `injected && !invoked` (matching each detector's own
`evaluate`, e.g. `unused_mcp_servers.rs:40`), and for each survivor sum
`token_count * cache_read_cost_per_token` across `models.by_model` via
`lookup_pricing`, stamping the same `pricing-generation-<n>` string as PR 1.
`cost_usd` is `None` when nothing in `by_model` prices.

### Frontend

`sessionHygiene.ts` already threads `findingEvidence` through to its
consumer. Add the three new IDs to the definition table (reusing the copy
already in `apps/desktop/src/lib/presentation/checks.ts:12-14`) and one
presentation branch per new payload variant, rendering `resource — ~$X.XX`
per entry in the existing `detail` slot (`sessionHygiene.ts`'s
`SessionHygieneCheck.detail`) — broaden that field's doc comment since the
slot now also carries prefix-check resource/dollar copy, not only
cache-accounting copy.

### Tests

- `badges.rs`: three synthetic-evidence tests (mirroring the existing
  per-`BadgeId` table) asserting Finding/Clean/NotAssessed.
- `dto.rs`: one test per new `finding_evidence` arm with two models at
  different rates, asserting summed `cost_usd`; one with no resolvable
  pricing asserting `cost_usd: None` while `name` still reports.
- Vitest: `sessionHygiene.ts` renders the resource/dollar line for each new
  badge ID.
- `docs/check-coverage.md`: note next to each of M/B/K's entry that the
  per-session badge additionally reports a per-resource dollar figure under
  the same eligibility as the finding.

## PR 3 sketch (optional)

`CacheChurn`'s `display_estimate_input` arm passes `paid_input_rate: None,
cache_read_rate: None`. Fill both from `lookup_pricing(model)` the same way
PR 1 does, stamp the same `pricing_revision`, drop the two `None`s.
`CacheRehydrationPriceDifference` (estimates.rs:203) needs no change — it
already expects exactly these two rates and a revision.

## Open questions

None block starting PR 1. The one judgment call worth a maintainer nod — using
evidence-time summed dollars (`replicated_cost_usd`) rather than a single rate
in `SavingsEstimateInput`'s three prefix variants, a smaller deviation from
the `CacheRehydrationPriceDifference` template than the brief assumed — is
recorded as a decision above, not a question.

## Verification

To check the Workflow-tool figure ($8.20 on 777 Fable + 4,219 Sonnet requests)
against a real session after PR 1 ships:

1. Find a session with an unused Workflow-tool finding in the Unused
   built-in tools target list.
2. Read `quantity`/`quantityUnit` on `BurnCheckDisplayFactsPayload` — this is
   `replicated_tokens` (unchanged by this plan) divided by the tool's
   definition size, i.e. the compatible-request count.
3. Multiply the tool's known definition size (7,900 tokens for Workflow) by
   each model's live `cache_read_cost_per_token` (`lookup_pricing`) and by
   that model's request count from `models.by_model[model].turns`; sum across
   models.
4. Compare against `estimatedOpportunity.value` on the same payload — they
   should match to floating-point rounding. A mismatch means either the
   per-turn loop counted a different request set (check the `scope` change)
   or priced against a different table revision (compare `pricing_revision`
   on the stored cause with the currently installed `pricing_generation()`).

## Deviations during PR 1

Recorded during implementation, smallest faithful choice in each case:

1. **`pricing_revision` added as a third field on `FindingCause::UnusedMcpServer`
   and `FindingCause::UnusedSkill`, not only `tokens`/`cost_usd`.** The plan's
   "Correction to the starting brief" and "Extend M and K to the
   source-evidence path" sections describe only two new fields, but the
   corresponding `SavingsEstimateInput::McpDefinitionExposure` /
   `InjectedSkillDocument` variants (this same PR) require a
   `pricing_revision: Option<String>` to gate `ApiEquivalentUsd` through
   `valid_revision`, the same way `UnusedBuiltInTool` needs it. Carrying the
   revision on the cause (stamped from
   `SessionTokenBurnEvidence.pricing_revision`) is the only way
   `display_estimate_input` can populate that field without a second lookup.
   `FindingCause::UnusedBuiltInTool` already needed the same third field for
   the same reason, so this generalizes rather than special-cases M and K.
2. **`compatible_requests: Some(1)` is how M and K's pre-summed
   `tokens: Option<u128>` maps onto `McpDefinitionExposure`'s and
   `InjectedSkillDocument`'s existing two-factor
   `(definition_tokens, compatible_requests)` shape.** Those two estimator
   variants multiply the two factors for their `LiteralInputTokens` fallback.
   M and K's source evidence already sums replicated tokens once per source
   (mirroring B), so one "compatible request" of that exact size reproduces
   the total unchanged, without changing the shape of a struct
   `CacheRehydrationPriceDifference` and others also use.
3. **The desktop-side dispatch gate needed a second widening the plan does
   not cite.** The plan's anchors for "widen the dispatch to also route
   `UnusedMcpServers` and `UnusedSkills`" point only at
   `crates/antiburn-local/src/insights/detectors/mod.rs` (lines 444, 460).
   That dispatcher is reachable only when the desktop shell's
   `assess_current_detector`
   (`apps/desktop/src-tauri/src/insights_report/findings.rs`) decides to
   fetch and pass `token_burn_evidence` at all; before this PR that gate
   special-cased only `DetectorId::UnusedBuiltInTools`. It now matches
   `UnusedBuiltInTools | UnusedMcpServers | UnusedSkills`, or M and K would
   never see source evidence in the running desktop app despite the engine
   crate supporting it.
4. **Amendment 2's verification found no code change was needed** — the
   deferred-tool exclusion already existed and was already tested; PR 1 adds
   one explicit test for it (`a_deferred_source_never_adds_tokens_or_cost_for_its_whole_group`)
   rather than new production code.
5. **File paths in "### Frontend" were one directory level stale.**
   `BurnCheckTargetDetail.tsx`, `BurnChecksReport.tsx`, and
   `BurnChecksSavings.tsx` live under
   `apps/desktop/src/views/main-window/burn-checks/`, not
   `apps/desktop/src/views/checks/` (only `checkUi.ts` lives there). No
   design change, just corrected paths.
6. **`checkRowPresentation` takes an added optional `targets` parameter
   instead of being fed a pre-aggregated sum.** The plan says to sum
   `estimatedOpportunity` across `targets.data.targets` "computed in
   `checkRowPresentation` ... so the main window and popover share it." In
   practice the popover (`views/popover/ChecksView.tsx`) never fetches
   `listBurnCheckTargets` — it only ever has `ChecksCategoryPayload`, which
   carries no target list and no pre-aggregated dollar figure. Making
   `checkRowPresentation(category, targets?)` accept the target list as an
   optional second argument keeps one shared implementation: the main
   window's `CheckRow` passes its already-loaded `state.targets[detector]?.data?.targets`
   and gets the summed cost line, and the popover's call site simply omits
   the argument and gets `costLine: null`, with no new fetch added to the
   popover's lighter surface.

## Deviations during PR 2

**PR 2 outcome (2026-09-14).** The per-session Hygiene badges this section
planned (PR #516, `feat/prefix-cost-session-badges`) were closed: a
per-session "unused" verdict is not evidence for a config change, since one
session's idle resource says nothing about the fleet, and the three checks
could never read clean, because no reader proves a full historical resource
inventory for one session (see `docs/check-coverage.md`). The recommendation
stays where it can act on it, in the aggregate Burn Checks report from PR 1. In its place, the session's Cost
tab gained an informational "Loaded but not used" section: it names each
resource this one session paid to carry idle and what that replay cost, with
no badge, no check, and no effect on any pass/fail count.
