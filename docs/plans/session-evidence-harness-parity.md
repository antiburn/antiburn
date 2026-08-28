# Draft plan: session evidence harness parity

**Status:** Draft for review  
**Scope:** The six session hygiene checks across Claude, Codex, OpenCode, and Pi  
**Primary risk:** A false clean result caused by incomplete source evidence

## Summary

Antiburn already has the correct high-level pipeline:

```text
raw transcript or provider database
    -> source adapter
    -> NormalizedRecord / NormalizedEvent
    -> CompositeSink
    -> SessionMetricsAccumulator + SessionEvidenceAccumulator
    -> persisted metrics and evidence
    -> session badges and the Insights report
```

The accumulators are shared across harnesses. This work does not replace them. It extends the canonical record contract, makes evidence consume the same logical parent-and-child session as metrics, and moves capability truth to the adapter boundary.

This is a medium-to-large analysis refactor, not an application rewrite. The UI, durable worker, evidence table, report reducer, and six stable badge identifiers remain in place.

The plan has two goals:

1. Stop current false-clean and false-finding outcomes immediately.
2. Support every check that each source can prove, while returning `NotAssessed` when a source or individual session cannot prove a verdict.

## Problem statement

The current implementation has four structural gaps.

### Static source capabilities decide too much

`apps/desktop/src-tauri/src/analysis.rs` selects a static `SourceCapabilities` value by vendor before the adapter reads the source. Most capabilities therefore describe a harness family rather than the facts available in one specific session.

A harness can support an optional signal while an older or incomplete session does not contain it. The current contract can turn that missing signal into a clean result.

### Metrics merge child streams, but evidence keeps only the parent

`stream_vendor_with_hooks` creates one accumulator per parent or child source. It merges metrics from all accumulators, but publishes evidence from only the first accumulator:

```rust
evidence: evidence.first().map(SessionEvidenceAccumulator::evidence)
```

This loses separately stored child facts before session badges run. It blocks or weakens:

- parent-versus-child model comparisons;
- delegated fast-mode detection;
- logical-session evidence completeness;
- thread-safe cache analysis across independently stored streams.

### The canonical event lacks a complete logical-thread contract

`NormalizedEvent` carries timestamps, token classes, model, effort, speed, source, and Claude-style record links. It does not yet express a complete provider-neutral contract for:

- logical thread identity;
- stable per-thread turn order;
- main-loop versus delegated scope;
- explicit parent-child relationships;
- completeness of optional effort and speed signals.

Adapters already know some of these facts, but they often use them only to filter inherited records or discover children.

### Detector prerequisites conflate finding proof with clean proof

Some detectors reject all evaluation when a conservative prerequisite is absent. This can hide a directly observed finding that needs fewer facts than a clean absence claim.

The new contract must distinguish:

- facts sufficient to prove a finding;
- facts sufficient to prove a clean result;
- facts that are unsupported by the source;
- facts that the source supports but this session records only partially.

## Immediate correctness defects

These changes should land before broader source support.

### Empty obsolete-model registry produces a clean result

The production `model_replacements` registry is empty. The detector sees no matching rule and can report clean on complete model evidence.

An empty reviewed registry must produce `NotAssessed(RegistryEmpty)` or the existing equivalent contract-incomplete reason. It must never produce clean.

### Fast-mode detection counts non-fast labels

The current detector sums delegated counts across every key in `models.fast_modes`. A delegated `standard` turn can therefore count as fast-mode overuse.

Only normalized `fast` values may enter the finding numerator. Recognized `standard` values belong only in the assessed denominator.

### Missing optional signals can read clean

A source-level effort or speed capability does not prove that every eligible turn in a session recorded that signal.

The evidence must retain:

- eligible turn count;
- signal-bearing turn count;
- missing-signal turn count;
- observed normalized values.

A missing signal blocks a clean result for checks that need it. An observed finding can still win when partial evidence proves its presence.

### Session-list not-assessed copy uses finding wording

The session-row tooltip uses finding text as the check name for `NotAssessed` rows. It can show “detected” next to a not-assessed mark.

The presentation definition must carry a verdict-free name separately from clean, finding, and not-assessed titles.

## Design principles

### Evolve the existing canonical seam

Do not build a parallel ingestion architecture. Extend `NormalizedRecord`, `NormalizedEvent`, and `EvidenceObservation` where needed.

A useful canonical turn shape is:

```rust
struct CanonicalTurn {
    role: Role,
    ts_ms: Option<i64>,
    logical_thread_id: Option<String>,
    turn_index: Option<u64>,
    scope: WorkScope,
    model: Option<String>,
    effort: Option<String>,
    speed: Option<String>,
    usage: Usage,
    tools: Vec<ToolCall>,
}

enum WorkScope {
    MainLoop,
    Delegated { child_id: String },
}
```

This is a conceptual shape. The implementation can extend `NormalizedEvent` rather than introduce a second turn type.

Compactions, context sources, and subagent relationships should remain explicit observations when they are not model turns.

### Keep source-specific logic at the adapter edge

Each adapter remains responsible for:

- parsing raw source shapes;
- filtering inherited or replayed records;
- normalizing model, effort, speed, and token classes;
- assigning logical-thread and work-scope facts it can prove;
- declaring structural support;
- reporting per-session completeness at end of stream.

Shared accumulators must not branch on harness names to interpret raw fields.

Detector policy can still differ by harness when the economic meaning differs. For example, repeated context is represented by cache-write tokens for one provider and uncached input for another.

### Structural support and observed coverage are separate

The adapter contract states what the source format can express. The stream summary states what this session actually expressed.

The final evidence state is their intersection:

- `Unsupported`: the source cannot express the fact;
- `Partial`: the source can express it, but this session has gaps;
- `Complete`: the source expressed every fact needed to prove absence.

`SourceCapabilities` may remain as serialized diagnostics, but static booleans must not be the sole detector eligibility authority. Add `NotAssessedReason::SignalMissing` for a source-supported setting that this session did not record. Keep it distinct from `CapabilityMissing`, `IncompleteEvidence`, and `EvidenceContractIncomplete`; serialize it as `signalMissing` with reader wording “Not assessed — this session did not record the setting this check needs.”

### Accumulate one logical session

Parent and child sources must feed one logical evidence accumulator. Each source gets a scoped wrapper that supplies its logical thread and main/delegated identity without changing source parsing.

Per-child metrics remain available for the current cost split and roster UI.

If discovery proves a child exists but the child cannot be read, relevant logical-session facts become partial. The child must not silently disappear and permit clean.

### Keep evidence rule-neutral

Persist observed facts and coverage, not final hygiene conclusions. Detector thresholds and reviewed registries remain report-time policy.

### Preserve bounded memory and local privacy

New thread, child, and signal collections require explicit caps. Overflow becomes `Partial(CapExceeded)`.

Do not persist prompts, reasoning text, tool input, private paths, raw provider IDs, or transcript fragments. Persist only bounded technical identifiers where a detector requires stable grouping.

## Target support matrix

Legend:

- **Supported:** the current source provides the needed facts after canonicalization.
- **Conditional:** supported only when the individual session records the optional signal or complete relationship.
- **Unsupported:** no trustworthy current source contract exists.
- **Validation required:** the raw relationship exists, but its product meaning needs characterization before it can support a verdict.

| Check | Claude | Codex | OpenCode | Pi |
| --- | --- | --- | --- | --- |
| Session overdepth | Supported | Supported | Conditional on root/child tagging | Supported |
| Model overthinking | Conditional on explicit effort | Conditional on explicit effort | Conditional on mapped variant policy | Conditional on thinking-level evidence |
| Overpowered subagents | Supported | Conditional on child rollout attribution | Conditional on covered `parent_id` subagent ancestry | Unsupported |
| Obsolete model | Conditional on reviewed registry | Conditional on reviewed registry | Conditional on reviewed registry | Conditional on reviewed registry |
| Fast-mode overuse | Conditional on speed coverage | Conditional on service-tier coverage | Unsupported | Unsupported |
| Excess cache rehydration | Supported with complete thread order | Conditional using uncached-input accounting | Conditional on session/thread identity | Conditional on cache-write support |

Expected maximum after this plan:

- Claude: 6/6.
- Codex: up to 6/6 when service-tier and child evidence are present.
- OpenCode: up to 5/6 for covered schemas; fast mode remains unsupported.
- Pi: 4/6.

Unsupported checks must still appear in the six-check UI with a settled `NotAssessed` reason.

Cursor and Antigravity are outside this plan. They remain outside the durable evidence cohort until their own source contracts and characterization suites exist.

## Detector semantics

### Session overdepth

Calculate request context occupancy from disjoint input classes on main-loop turns only.

A finding requires one observed main-loop request above the reviewed cap. A clean result requires complete main-loop ownership, request-context coverage, model identity, and order where the adapter needs order to prove ownership.

Do not require Claude-style UUID links when an adapter has already proven root ownership through another mechanism.

Lock the overdepth cap at 400,000 tokens for this work. Keep it in the report-time catalog so a later evidence-backed policy change does not require reparsing. Support `ANTIBURN_SESSION_OVERDEPTH_CAP_TOKENS` as a positive-integer startup override with precedence over the compiled default. Parse it once in the shell and inject the same effective catalog into session badges and the 30-day report. Invalid values fall back to 400,000 tokens. Unit tests inject catalogs directly; a debug-launch integration test sets the environment variable against synthetic data and verifies the visible finding.

### Model overthinking

Only explicit effort values count. Normalize case and whitespace before policy lookup.

Store signal coverage separately from observed tier counts. Unknown values do not trigger, but they prevent a clean result until policy classifies them.

Use a reviewed per-family tier policy. Do not assume that equal strings have equal meaning across harnesses.

### Overpowered subagents

Compare the dominant main-loop model tier with observed delegated model tiers in the same logical session.

A finding requires:

- a proven delegated turn or child relationship;
- an observed child model;
- an observed main-loop model;
- reviewed premium-tier classification for both.

A clean result additionally requires complete child enumeration and child-model attribution.

### Obsolete model

Use a non-empty reviewed replacement registry with:

- stable source model identifiers and aliases;
- replacement model identifier;
- effective date;
- rationale;
- registry revision.

Normalize observed model keys before matching. Usage before the replacement effective date is not a finding. An empty registry is not assessed.

### Fast-mode overuse

Recognize exact normalized `fast` and `standard` values. Missing or unknown speed values are not standard.

Support two settled patterns when the evidence permits them:

1. any recognized fast turn in delegated or sidechain work;
2. standing-default use when fast turns are at least 30% of recognized fast-or-standard turns across at least three fast-containing sessions and at least five eligible sessions in the report window.

The per-session badge reports the delegated pattern. The 30-day report evaluates both the delegated and cross-session standing-default patterns. Null or unknown speed values enter neither numerator nor denominator and block clean when required coverage is absent. Claude findings use dollar impact; Codex findings use plan-quota impact.

For Codex, parse a trustworthy thread-settings service tier and normalize reviewed values such as priority/default into fast/standard.

### Excess cache rehydration

Group non-sidechain turns by logical session and thread. Sort by a stable turn index with timestamps as a secondary check. Skip pairs with incomplete or overlapping order.

For adjacent turns, calculate repeated paid context beyond positive context growth:

- cache-write accounting where the provider reports paid cache creation;
- uncached-input accounting where the provider reports full-price repeated input but no cache writes.

Attribute observed causes separately:

- compaction;
- model switch;
- idle gap beyond the reviewed cache TTL;
- other.

Cause attribution explains a finding but does not replace the repeated-context calculation.

The detector must never compare a parent turn with a child turn or two unrelated child threads. Preserve the stable `excessCacheRehydration` identifier, but use the provider-neutral shared check name “Excess context reprocessing.” Detailed report copy uses “Reduce repeated cache writes” for cache-write accounting and “Reduce full-price context re-reads” for uncached-input accounting.

## Implementation phases

### Phase 0: Freeze baselines and decisions

**Risk:** Low

1. Record the current source capability and badge status matrix in characterization tests.
2. Add failing tests for the four immediate correctness defects.
3. Lock the reviewed overdepth cap at 400,000 tokens and its `ANTIBURN_SESSION_OVERDEPTH_CAP_TOKENS` startup override.
4. Lock both fast-mode patterns: delegated fast use and the 30-day standing-default threshold.
5. Adopt the locked initial replacement registry listed in this plan. Keep `RegistryEmpty -> NotAssessed` as a permanent honesty rule.

**Acceptance:** Tests reproduce every current false-clean or false-finding path before implementation changes.

### Phase 1: Land honesty fixes

**Risk:** Low

Likely files:

- `crates/antiburn-local/src/insights/detectors/mod.rs`
- `crates/antiburn-local/src/insights/detectors/old_model_usage.rs`
- `crates/antiburn-local/src/insights/detectors/model_overthinking.rs`
- `crates/antiburn-local/src/insights/detectors/overuse_of_fast_mode.rs`
- `crates/antiburn-local/src/insights/badges.rs`
- `apps/desktop/src/lib/presentation/sessionHygiene.ts`

Changes:

- Load the nine predefined replacement rules from a compiled registry; an empty registry returns not assessed.
- Fast detection counts only exact fast values.
- Missing or unknown effort/speed coverage blocks clean and uses the structured `signalMissing` reason when applicable.
- Session-list not-assessed names become verdict-free.
- Increment the report catalog revision when policy changes.

**Acceptance:** No empty policy registry or absent optional signal can produce clean. A delegated standard turn cannot produce a fast finding.

### Phase 2: Strengthen the canonical adapter contract

**Risk:** Medium

Likely files:

- `crates/antiburn-local/src/analysis/interface.rs`
- `crates/antiburn-local/src/analysis/model.rs`
- `crates/antiburn-local/src/analysis/evidence.rs`
- `crates/antiburn-local/src/analysis/evidence_sink.rs`
- `crates/antiburn-local/src/analysis/metrics_sink/`

Changes:

- Add provider-neutral logical thread and work scope.
- Add per-signal eligible/present/missing coverage.
- Add adapter-owned structural capabilities.
- Add end-of-stream session coverage to `SessionSummary` or a dedicated canonical summary.
- Remove the desktop’s duplicate vendor capability authority after all adapters migrate.
- Preserve the current serialized `SessionMetrics` output.

Use a compatibility bridge while adapters migrate. Do not switch every source in one unverified step.

**Acceptance:** Existing Claude metrics and evidence remain equivalent on all characterization fixtures. The new canonical fields serialize no private source content.

### Phase 3: Build logical-session evidence accumulation

**Risk:** High

Likely files:

- `apps/desktop/src-tauri/src/analysis.rs`
- `crates/antiburn-local/src/analysis/evidence_sink.rs`
- `crates/antiburn-local/src/analysis/merge.rs`

Changes:

- Replace `evidence.first()` with one logical evidence result.
- Feed parent and child sources through a scoped sink into the logical evidence accumulator.
- Keep independent child metrics accumulators for roster and cost output.
- Tag child records with logical thread and delegated scope.
- Degrade child-dependent evidence when a discovered child is unreadable or incomplete.
- Keep parent and child context/cache sequences separate.

**Acceptance:** A child-only model or speed signal reaches parent session hygiene. Child model switches never create parent cache churn. An unreadable known child prevents a false clean.

### Phase 4: Enrich source adapters

**Risk:** Medium

#### Claude

- Preserve UUID linkage and inline sidechain observations.
- Reconcile inline sidechains with separately discovered child sources without double counting.
- Report effort and speed completeness per eligible turn.

#### Codex

- Preserve root versus child rollout ownership.
- Emit child scope when the desktop supplies a discovered child rollout.
- Parse reviewed service-tier records into canonical fast/standard values.
- Preserve request occupancy and effort attribution.
- Report uncached-input support for cache analysis.

#### OpenCode

- Include source session ID and parent ID while querying the recursive cluster.
- Stop flattening every descendant anonymously into the root stream.
- Treat `session.parent_id` as delegated ancestry for covered schemas. Distinguish it from forks, which have a null `parent_id` and a copied-prefix root session.
- Preserve per-message variant, model, token classes, and compaction boundaries.

#### Pi

- Preserve root/fork filtering guarantees.
- Preserve thinking-level changes as explicit effort state.
- Retain per-session cache-write availability based on the selected API.
- Do not infer subagents or speed from parent-session metadata.

**Acceptance:** Each adapter’s characterization suite freezes its structural support, per-session partial states, and final six-check status matrix.

### Phase 5: Rework detector fact requirements

**Risk:** Medium to high

Likely files:

- `crates/antiburn-local/src/insights/report.rs`
- `crates/antiburn-local/src/insights/badges.rs`
- the six detector modules under `crates/antiburn-local/src/insights/detectors/`

Changes:

- Evaluate fact coverage rather than vendor capability masks.
- Separate finding requirements from clean requirements.
- Remove the universal thread-identity gate from overdepth when main-loop ownership is otherwise proven.
- Implement dominant parent versus delegated child model evaluation.
- Implement reviewed effort and speed policies.
- Implement normalized replacement matching.
- Replace event-presence cache churn with per-thread repeated-context accounting.

**Acceptance:** Every target matrix cell has a fixture showing finding, clean, and not-assessed behavior where those states are valid.

### Phase 6: Revision rollout and operational hardening

**Risk:** Medium

Expected revision changes:

- `PARSER_REVISION`: 5 -> 6 when adapter meaning changes.
- `ANALYZER_REVISION`: 6 -> 7 when evidence reduction changes.
- `EVIDENCE_SCHEMA_REVISION`: 3 -> 4 when persisted evidence fields change.
- `METRICS_SCHEMA_REVISION`: remain 1 unless the serialized metrics contract changes.

A new SQLite table is not expected. The existing revisioned evidence JSON and reconciliation path should requeue stale rows.

Changes:

- Verify stale rows become pending and recompute.
- Verify the UI never reads stale evidence as clean.
- Re-run memory baselines with parent plus many children.
- Audit every new field for bounds and privacy.
- Verify active-source and source-changed behavior still publishes metrics and evidence atomically.

**Acceptance:** Upgrade tests prove old evidence is reprocessed without a manual database migration or false clean transition.

## Test strategy

### Characterization

Extend:

- `crates/antiburn-local/tests/claude_characterization.rs`
- `crates/antiburn-local/tests/codex_characterization.rs`
- `crates/antiburn-local/tests/opencode_characterization.rs`
- `crates/antiburn-local/tests/pi_characterization.rs`

Each source needs fixtures for:

- complete root session;
- missing optional effort/speed signal;
- unknown signal value;
- child-only model usage where supported;
- unreadable or incomplete child;
- compaction and model switching;
- malformed and unknown records;
- active incomplete tail;
- obsolete model before and after effective date.

### Canonical parity

Prove that the richer canonical stream preserves current metrics:

- token totals and classes;
- context occupancy;
- model runs;
- tools and skills;
- compaction counts;
- cost split;
- parent and child roster.

### Logical-session attribution

Prove:

- a child premium model reaches the parent’s evidence;
- a child fast turn reaches the parent’s evidence;
- a child context window never increases parent overdepth;
- a child model switch never creates parent cache churn;
- two child threads never form an adjacent cache pair;
- a missing known child prevents clean.

### Detector honesty

For every check:

- observed findings survive partial evidence when presence is conclusive;
- incomplete absence never becomes clean;
- unsupported facts return capability missing;
- source-supported but missing session facts return incomplete or contract-incomplete;
- an empty policy registry returns not assessed;
- unknown policy values return not assessed rather than clean.

### UI and IPC

Preserve the six badge IDs and their order. The stable `excessCacheRehydration` ID remains unchanged while its shared reader label becomes “Excess context reprocessing”:

- `sessionOverdepth`
- `modelOverthinking`
- `overpoweredSubagents`
- `obsoleteModel`
- `fastModeOveruse`
- `excessCacheRehydration`

Verify all six appear in:

- the session-row aggregate and tooltip;
- session detail;
- the corresponding Insights report categories.

Verify unsupported sources settle as not assessed instead of remaining in a permanent pending state.

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

A source can expose inline sidechain turns and separate child files. The logical sink needs a stable deduplication rule before both paths are enabled.

**Mitigation:** Characterize identity overlap first. Enable one authoritative child path per source until equivalence is proven.

### OpenCode descendant meaning

The covered OpenCode schema uses `session.parent_id` for child subagent sessions. Forks instead create a new root with null `parent_id`, a copied history prefix, and a generated fork title. Unknown or changed schemas can invalidate that distinction.

**Mitigation:** Add hand-authored synthetic fixtures for a `parent_id` subagent, a copied-prefix fork with null `parent_id`, and a similar-title non-fork. Enable the subagent check only for the covered relationship shape; unknown shapes remain not assessed.

### Optional signals produce false clean

Old sessions can lack fields that newer harness versions emit.

**Mitigation:** Track signal-bearing and missing eligible turns. Never interpret `None` as a negative observation.

### Cache calculations mix threads

Global timestamp order is insufficient when child work overlaps the parent.

**Mitigation:** Require complete per-thread identity and order for clean. Skip incomparable pairs and retain partial coverage.

### Policy changes alter old verdicts

Replacement, effort, speed, premium-tier, and cache-TTL policies can change without parser changes.

**Mitigation:** Keep policy in versioned report-time catalogs and increment catalog revision independently from parser revisions.

### Reprocessing load

An evidence-schema revision requeues every current cohort session.

**Mitigation:** Use the existing bounded durable worker, preserve last completed evidence as stale, and measure the queue on representative parent-plus-child fixtures.

## Non-goals

- Adding a seventh session hygiene check.
- Redesigning the session row or Insights pane.
- Enabling Cursor or Antigravity evidence in the same change.
- Inferring unsupported signals from prompts, model names, pricing, or generic ancestry.
- Persisting transcript text or tool input.
- Replacing the durable worker, report reducer, or evidence table.
- Combining native and WSL report scopes.

## Delivery recommendation

Ship each seam as its own production-safe, independently shippable pull request. No pull request may leave existing evidence unreadable, stop the durable worker from processing current rows, or permit a false clean result. If a prerequisite pull request has not merged when the next pull request is ready, stack the next branch onto the prerequisite branch and target that branch. After the prerequisite merges, rebase or retarget the stacked pull request onto the updated base without combining the seams.

Recommended seams:

1. honesty fixes;
2. canonical scope and signal coverage;
3. logical parent-child evidence;
4. one adapter enrichment per source;
5. detector fact requirements and cache accounting;
6. revision rollout and UI verification.

Each seam must leave unsupported or incomplete states as `NotAssessed`. Temporary loss of a clean badge is acceptable. A false clean result is not.

## Review questions and settled decisions

1. **Settled:** Fast-mode policy includes delegated fast use and the 30-day standing-default threshold.
2. **Settled:** The overdepth cap is 400,000 tokens with an `ANTIBURN_SESSION_OVERDEPTH_CAP_TOKENS` startup override.
3. **Settled:** The first production model replacement registry uses the nine reviewed rules below.
4. **Settled:** Covered OpenCode `session.parent_id` relationships are subagent ancestry; copied-prefix forks have null `parent_id`.
5. **Settled:** Keep the stable cache check ID, use the shared name “Excess context reprocessing,” and use accounting-specific detail copy.
6. **Settled:** Source-supported but signal-free sessions use the structured `signalMissing` not-assessed reason.

## Locked initial model replacement registry

These report-time policy rules are the initial production registry. Ship them as a hardcoded, compiled list that works without network access. Model matching must normalize case and aliases before evaluation. The effective date is the earliest date on which post-release usage can produce a finding. A future remotely published registry may override this list only after complete schema and revision validation; missing, invalid, stale, or unreachable remote data must always fall back to the compiled values. Remote registry retrieval is outside this plan, but the catalog interface and revision must preserve that extension seam.

| Stable rule key | Source model IDs | Replacement | Effective date |
| --- | --- | --- | --- |
| `claude-opus-4-5-to-opus-5` | `claude-opus-4-5`, `claude-opus-4-5-20251101` | `claude-opus-5` | 2026-07-24 |
| `claude-opus-4-6-to-opus-5` | `claude-opus-4-6`, `claude-opus-4.6`, `antigravity-claude-opus-4-6-thinking` | `claude-opus-5` | 2026-07-24 |
| `claude-opus-4-7-to-opus-5` | `claude-opus-4-7`, `claude-opus-4.7` | `claude-opus-5` | 2026-07-24 |
| `claude-opus-4-8-to-opus-5` | `claude-opus-4-8`, `claude-opus-4.8`, `anthropic/claude-opus-4.8`, `claude-opus-4-8-thinking-high` | `claude-opus-5` | 2026-07-24 |
| `claude-sonnet-4-5-to-sonnet-5` | `claude-sonnet-4-5`, `claude-sonnet-4-5-20250929` | `claude-sonnet-5` | 2026-06-30 |
| `claude-sonnet-4-6-to-sonnet-5` | `claude-sonnet-4-6`, `claude-sonnet-4.6` | `claude-sonnet-5` | 2026-06-30 |
| `gpt-5-4-mini-to-gpt-5-6-luna` | `gpt-5.4-mini` | `gpt-5.6-luna` | 2026-07-09 |
| `gpt-5-4-to-gpt-5-6-terra` | `gpt-5.4` | `gpt-5.6-terra` | 2026-07-09 |
| `gpt-5-5-to-gpt-5-6-sol` | `gpt-5.5`, `gpt-5.5-fast` | `gpt-5.6-sol` | 2026-07-09 |
