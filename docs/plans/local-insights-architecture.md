# Local Insights: staged architecture and delivery plan

**Status:** Proposal for discussion
**Goal:** Put a useful local Hygiene and Efficiency experience in front of users quickly, while leaving a credible path to a more correct and scalable implementation.

## Executive position

The system should be treated as four separable stages:

```text
session finder → session parser → session analyzer → efficiency report
      │                │                 │                  │
   sources        normalized         derived facts       findings +
  on disk/DB        sessions          and detectors       aggregates
```

The first version should not attempt to reproduce the entire Cadence backend or the full architecture of `ctx`. It should reuse Antiburn's existing local engine, make conservative claims, and optimize for learning whether users value the recommendations.

The design should establish a few durable seams from the beginning:

- parsers return a vendor-neutral session plus provenance and diagnostics;
- analyzers consume normalized facts rather than raw JSON;
- detectors are pure functions over derived evidence;
- reports carry coverage, freshness, and version metadata;
- missing or unsupported evidence is represented as “not assessed,” never as zero.

Everything else can be deliberately simple in the first pass.

## Canonical Hygiene and Efficiency scope

The local report should preserve the complete set of findings and remediation ideas currently represented by Cadence `main` at `/{org-id}/my-work/insights`, under **Hygiene and Efficiency**. The current web implementation has nine detector sections—not just the original six:

| Cadence section | What it finds | Current solution/remediation |
|---|---|---|
| **Sessions Over Depth** | Individual Claude Code requests whose context exceeds the evidence-driven autocompact cap. This is request depth, not merely a long-running session. | Set `CLAUDE_CODE_AUTO_COMPACT_WINDOW` to the recommended cap, compact at natural task boundaries, or move open-ended exploration into a subagent. |
| **Model Overthinking** | Claude Code or Codex sessions using reasoning/thinking tiers above the reviewed recommended cap, currently principally `xhigh`/`max`/`ultra` versus `high`. | Lower the tier with `/effort` in Claude Code or `/model` in Codex. `ultrathink` and `ultracode` prompt keywords are separate from this setting and are not inferred as reasoning-tier evidence. |
| **Overpowered Subagents** | Premium main-loop models silently spawning subagents on the same premium tier, such as Fable/Opus or Codex/Sol subagents. | Configure a cheaper subagent model: `CLAUDE_CODE_SUBAGENT_MODEL` or per-agent Claude configuration; Codex `default_subagent_model` in `~/.codex/config.toml`. Keep premium subagents when deliberately justified. |
| **Unused MCP Servers** | MCP definitions loaded into eligible sessions but never directly invoked. | Remove the server or scope it to projects that use it. Claude supports `claude mcp remove`, project `.mcp.json`, connector denial, or disabling auto-fetched connectors; Codex supports `codex mcp remove` or project-scoped `~/.codex/config.toml`. |
| **Unused Built-In Tools** | Native harness tools whose definitions consume context but are not used. | Disable only when there is curated knowledge of what the tool does, what capability is lost, and a safe disable mechanism. Claude deny/settings controls and Codex toggles are supported where known; un-disableable tools remain audit-only rather than receiving unsafe advice. |
| **Unused Skills** | Skills loaded repeatedly but never invoked, grouped by installed, project, plugin, or bundled origin. | Remove or narrow installed/project skills; use Claude `skillOverrides` or `disable-model-invocation: true`; use Codex `[[skills.config]] ... enabled = false`; uninstall unused plugins. Bundled skills without a documented disable mechanism remain visible as checks, not recommendations. |
| **Old Model Usage** | Usage of a curated deprecated model after its replacement became available. Pricing informs the opportunity but does not gate a capability recommendation. | Select the replacement with `/model`, or change the default model in `~/.claude/settings.json` or `~/.codex/config.toml`. |
| **Overuse of Fast Mode** | Fast-tier usage either inside delegated/subagent work or left on as a standing default. Claude impact is a documented price premium; Codex impact is plan quota, not an invented dollar price. | Toggle `/fast` deliberately for latency-critical interactive work; switch it off before delegation-heavy work or when left on by default. |
| **Cache Churn** | Tokens paid or re-sent more than once because of idle expiry, compaction, or model switching. Claude cache writes carry a premium and roughly an hour expiry; Codex has uncached-input/provider-eviction behavior. | Finish or hand off before cache expiry, compact after returning to a large context, start a fresh session when accumulated context is unnecessary, and avoid switching models mid-session. Treat provider-side eviction estimates as an upper bound on user-controlled savings. |

Every section also has an explicit detector status: findings, clean, or not assessed with a reason such as insufficient sessions, missing pricing, missing harness coverage, or incomplete evidence. A local implementation should preserve that honesty model rather than treating an empty findings list as proof that the check ran cleanly.

The exact wording and settings syntax can evolve, but the nine-category scope and the distinction between actionable recommendations and audit-only checks are part of the compatibility target. The first local report may support fewer harnesses or mark some categories unavailable, but it should not silently omit a category that it did not assess.

## Existing Antiburn capabilities to reuse

Antiburn already has most of the basic pipeline:

- `crates/antiburn-local/src/discovery/mod.rs`
  - `Explorers::discover_recent_sessions` fans out across supported agents;
  - `SessionLog` and `SessionSource` represent file, inline, and provider-database sources;
  - `AgentExplorer` provides provider-specific discovery and optional database fingerprints.
- `crates/antiburn-local/src/analysis/interface.rs`
  - `SessionInput`, `RawSource`, and `VendorAdapter` define the discovery-to-parser seam.
- `crates/antiburn-local/src/analysis/model.rs`
  - `NormalizedSession`, `NormalizedEvent`, `Usage`, and `ToolCall` are already a useful internal carrier model.
- `crates/antiburn-local/src/analysis/engine.rs`
  - derives `SessionMetrics`, token totals, context pressure, tool mix, compaction markers, phases, and costs.
- `crates/antiburn-local/src/analysis/initial_context.rs`
  - already attributes initial context to skills, MCP, and other sources where the transcript supports it.
- `apps/desktop/src-tauri/src/analytics.rs`
  - runs CPU-heavy analysis on the blocking pool and already owns source fingerprints and analysis orchestration.
- `apps/desktop/src-tauri/src/store/`
  - already stores derived per-session analysis and has explicit clear/delete behavior.
- `apps/desktop/src/views/SettingsView.tsx` and `apps/desktop/src/lib/settingsPanes.ts`
  - provide the natural first UI surface and pane-registration seam.

The first implementation should extend these seams rather than importing Cadence's `cadence-analysis` dependency graph or introducing a parallel engine. This proposal is specifically for the live 30-day cross-session report; session-level diagnostic cards are a separate feature documented in [`docs/plans/session-level-insights-cards.md`](session-level-insights-cards.md).

## 1. Session finder

### Responsibility

Find candidate session sources and describe them. The finder should not parse provider payloads or make claims about their schema.

### Make It Work

Use the existing `Explorers::discover_recent_sessions` with an explicit 30-day lookback for the Insights request. Return the existing `SessionLog`/`SessionSource` values and filter them by the source timestamp or the best available discovery recency.

For the prototype:

- use existing file and provider-database discovery;
- use the existing stable source identity where available;
- honor existing folder-consent and WSL boundaries;
- skip missing or unreadable sources individually;
- do not add a new database table or discovery index;
- report the number of discovered, parsed, and usable sessions separately.

The normal activity scan currently has a shorter configurable window. Insights should not silently inherit that limit if the product promise is “last 30 days”; it should request its own bounded lookback or explicitly label the narrower scope.

### Make It Good

Introduce a small `SourceDescriptor` at the discovery/parser boundary:

```text
provider                 claude / codex / opencode / ...
source_kind              jsonl_file / sqlite / inline
source_identity          stable local identity
location                 path or provider-local identifier
observed_harness_version optional version from metadata
observed_schema_variant  optional provider schema identifier
source_fingerprint       mtime+size, DB fingerprint, or equivalent
```

The descriptor should be diagnostic and internal at first. It gives later caching and support reports a stable place to record why a source was selected without making the finder understand provider schemas.

### Make It Fast

Measure discovery separately from parsing. Keep the existing parallel agent fan-out and add bounded concurrency for source reads. Use provider-specific fingerprints where available; do not walk or open unchanged sources unnecessarily.

## 2. Session parser

### Responsibility

Turn one source into a vendor-neutral session while preserving enough provenance to explain what was and was not understood.

### Make It Work

Keep the current `VendorAdapter` interface and existing dedicated adapters. Parse incrementally:

- JSONL should be read line by line, not loaded conceptually as one giant JSON array;
- each line should be independently decoded or skipped with a diagnostic;
- malformed records should not discard an otherwise useful session;
- SQLite sources should use the existing adapter as a fallback and be labeled lower fidelity where schema meaning is uncertain;
- provider-specific parsing should remain in provider modules, not in the generic analyzer.

The initial normalized model can remain close to the existing `NormalizedSession` and `NormalizedEvent`. Add only the provenance needed by the report:

```text
provider
source_format
parser_revision
observed_version / schema_variant
capabilities or fidelity flags
diagnostics: unknown fields, skipped records, unsupported features
```

The parser should be able to return one of:

- usable session;
- usable session with partial coverage;
- unsupported/invalid source with a reason.

### Make It Good

Move toward provider/version-specific parser modules without prematurely building a full framework:

```text
vendors/
  claude/
    v_current.rs
  codex/
    v_current.rs
  opencode/
    sqlite_v_current.rs
  generic_jsonl.rs
```

Version detection should be explicit. A parser should record both the observed harness/schema version and the parser revision that interpreted it. Unknown fields can be tolerated and reported; breaking structural changes should produce a partial or unsupported result instead of silently producing false values.

A normalized event should eventually carry, where available:

- native event identity and sequence;
- timestamp;
- role and usage;
- model;
- tool name and useful input summary;
- compaction boundary;
- sidechain/thread identity;
- source provenance.

Do not add every field speculatively. Add a field when a detector or user-facing claim requires it.

### Make It Fast

Use streaming and bounded reads. Avoid reparsing the same raw payload once for every detector. The parser should produce one normalized session that all analyzers consume.

## 3. Session analyzer

### Responsibility

Derive analytical facts from normalized sessions and run detectors over those facts. The analyzer is where “a Skill tool call occurred” or “context was repeatedly rehydrated” belongs—not in the finder.

Separate the analyzer into three conceptual layers:

1. **Normalized session:** provider-neutral events and provenance.
2. **Derived evidence:** token turns, model usage, context sources, skill/MCP calls, compactions, eligibility, costs, and coverage.
3. **Detectors:** pure functions that turn evidence into findings and detector statuses.

`EfficiencyEvidence` belongs at layer 2/3. It should not become the parser's carrier type because it is shaped around one product feature and will change as new insights are added.

### Make It Work

Use the existing engine outputs wherever possible:

- `SessionMetrics` for token, context, duration, tool, and cost facts;
- `initial_context` for skill/MCP attribution;
- normalized `ToolCall` and `SkillUse` for invocation detection;
- `NormalizedEvent::is_compaction_boundary` for compaction signals;
- existing pricing for approximate savings.

For the first user experiment, preserve the nine-section report contract above even if some sections are initially not assessed. Implement the detectors with the strongest local evidence first—context depth, cache churn, unused skills/MCP, model usage, and reasoning/fast-mode signals—and render explicit unavailable statuses for built-in-tool fleet validation, unsupported harness versions, missing pricing, premium-subagent joins, or partial initial-context attribution.

Do not make claims requiring unavailable fields. A detector that cannot yet run should appear as “not assessed” with a reason, not disappear and not report “clean.” The full set of Cadence findings and solutions remains the target behavior; staged implementation controls evidence coverage and confidence rather than silently narrowing the product definition.

Each finding should include:

```text
stable finding kind
human-readable title and explanation
evidence values used
estimated impact, if defensible
confidence/coverage status
recommended action, if any
```

### Make It Good

Create a cross-agent evidence matrix and golden fixtures. For each detector, document:

- required fields;
- optional fields;
- behavior when fields are missing;
- eligibility thresholds;
- whether the result is exact, estimated, or not assessed.

Keep detector logic pure and deterministic. This permits the same detector to run over live sessions, fixtures, cached metrics, or a future report cache.

Do not claim parity with Cadence until there is an approved rule/spec fixture set. The Cadence implementation can provide useful behavioral examples, but its server evidence contract and private dependencies should not become Antiburn's architecture by accident.

### Make It Fast

Avoid one raw transcript pass per detector. Build the derived facts once, then run all detectors over them. Reuse existing `SessionAnalysis` and `metrics_json` cache entries where their evidence is sufficient; only reread raw sources for fields not already cached.

## 4. Efficiency report

### Responsibility

Aggregate per-session evidence and findings over a defined window, and describe the quality and freshness of the result.

A report should contain at least:

```text
computed_at
window_start / window_end
source_count
parsed_count
usable_count
eligible_session_count
findings
detector statuses
pricing/catalog revision
parser/normalization revision
coverage and diagnostics
```

### Make It Work

Do not persist reports initially. Compute on opening the Insights pane, keep the result in memory for that request, and discard it. This avoids a migration and lets the product team change detector rules quickly while validating value.

For this proposal, the MVP is the 30-day report: invoke the existing discovery pipeline with a 30-day lookback, normalize the discovered sources, aggregate in memory, and return the report from one new command. This is the smallest version that tests the intended cross-session experience.

Use one settings pane. Run CPU-heavy work on the blocking pool and expose loading/progress/partial-result states. Do not add dismissals, history, notifications, or nudges until users demonstrate that the recommendations are useful. Do not add session-level cards to this implementation; they have a different user moment and product purpose.

The first report can be labeled explicitly:

> Computed live from local sessions on this machine. Some detectors may be unavailable when the source does not provide enough evidence.

### Make It Good

Add a report cache only after the report contract is stable. Cache identity should include more than time:

```text
window definition
source fingerprints
parser revision
normalization/analyzer revision
detector revision
pricing generation
```

A report may be reused inside a freshness interval only when those inputs still match. Findings should have stable identities so later dismissals or history do not depend on list ordering.

If persistence is needed, first reuse the existing `session_analysis` storage for per-session derived facts. Add a report table only when there is a demonstrated need for cross-launch history, dismissals, or expensive incremental aggregation.

### Make It Fast

Measure before adding storage. Potential optimizations, in order:

1. reuse existing per-session metrics;
2. add parser/analyzer-aware invalidation to the existing fingerprint cache;
3. parse sources concurrently with a bounded limit;
4. cache normalized or derived evidence on disk if cold reads are measurably slow;
5. add incremental report aggregation or a dedicated report store only if profiling proves it necessary.

A time-window cache alone is insufficient: a new session, changed transcript, parser update, or pricing update must invalidate the relevant result.

## Kent Beck delivery sequence

### Make It Work

**Objective:** prove that users find at least one recommendation useful.

- Use current discovery, adapters, normalized sessions, and metrics.
- Support only the harnesses and detectors with reliable evidence.
- Compute on demand; no new SQLite schema.
- Add a Settings → Insights surface with top findings and honest unavailable states.
- Use synthetic fixtures and a small set of real-transcript manual checks.

**Exit evidence:** users can understand a finding, identify the evidence behind it, and report that it changes a setting or behavior. The feature must not produce false zeros for unsupported data.

### Make It Good / Right

**Objective:** make the validated concept maintainable and trustworthy.

- Add `SourceDescriptor`, parser/schema revisions, capabilities, and diagnostics.
- Split provider/version-specific parsing from generic parsing.
- Stabilize normalized-session and derived-evidence contracts.
- Add golden fixtures per provider/version and detector.
- Define exact/estimated/not-assessed semantics.
- Integrate clear/delete behavior and stable finding identities.
- Review source provenance and open-source boundary before importing any external or Cadence-derived code.

**Exit evidence:** parser and detector behavior is deterministic, version-aware, explainable, and covered by fixtures; changing a parser invalidates affected derived data.

### Make It Fast

**Objective:** meet a measured user-facing performance budget.

- Profile discovery, file I/O, parsing, normalization, analysis, aggregation, and IPC separately.
- Reuse `session_analysis` and current fingerprints where possible.
- Add bounded concurrency and incremental invalidation.
- Persist only the layer whose cost justifies persistence.
- Keep the background app lazy; Insights should not cause continuous rescans while closed.

**Exit evidence:** a representative large local history opens within an agreed budget, cache hits avoid raw parsing, and profiling shows no material CPU/memory regression in the normal tray app.

## What not to build in the first pass

- A full `ctx`-style immutable history/index system.
- A new SQLite schema dedicated to reports.
- Server parity or backend-specific Cadence dependencies.
- Fleet-wide validation for built-in tools.
- Automatic edits to agent configuration files.
- Long-term history charts, dismissal synchronization, or background nudges.
- Parser support for every harness before the first user test.

The [`ctxrs/ctx` history crates](https://github.com/ctxrs/ctx/tree/main/crates) are a useful architectural reference: its provider-specific modules, source descriptors, schema/version evidence, bounded JSONL readers, SQLite snapshot handling, and normalized core records demonstrate the direction for Make It Good. Its full layering is not a prerequisite for Make It Work.

## Decisions needed before implementation

1. Which two or three detectors are most likely to produce an actionable cross-session decision?
2. Which harnesses are in the first supported cohort—Claude and Codex only, or all currently discovered agents with reduced coverage?
3. What is the minimum evidence required to call a finding actionable rather than merely interesting?
4. What cold-open latency is acceptable before a progress state becomes a product problem?
5. Are findings allowed to recommend copy/paste actions, or only describe opportunities in the first experiment?
