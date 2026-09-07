# Burn check coverage and remediation

Status: Phase 1 is in progress. Phase 2 is backend-only. Phase 3 is deferred.

## Naming and architecture

Rename `VendorAdapter` to `SessionReader`. Name the model-provider abstraction
`ModelCatalog`, not `ProviderAdapter`.

| Name | Responsibility |
| --- | --- |
| `SessionReader` | Read native session files and databases into normalized observations with explicit coverage |
| `ModelCatalog` | Resolve provider/model capabilities, option mappings, accounting rules, prices, and reviewed check policies |
| `AgentConfigEditor` | Inspect effective agent configuration and prepare supported, minimal edits |

These names separate coding agents from model providers. Use traits at these
boundaries, typed enums for shared operations, and data for provider/model
differences. Do not introduce a plugin framework or an agent-by-provider
implementation matrix.

Illustrative interfaces:

```rust
trait SessionReader: Send + Sync {
    fn read(
        &self,
        input: &SessionInput,
        sink: &mut dyn RecordSink,
    ) -> Result<ReadOutcome>;
}

trait ModelCatalog: Send + Sync {
    fn resolve(&self, target: &ModelTarget) -> Support<ModelDefinition>;
}

trait AgentConfigEditor: Send + Sync {
    fn inspect(&self, context: &ConfigContext) -> Result<ConfigSnapshot>;

    fn prepare(
        &self,
        snapshot: &ConfigSnapshot,
        change: &ConfigChange,
    ) -> Result<EditSupport>;
}
```

Preserve the existing reader's streaming, snapshot, and resume methods during
the rename. These signatures describe boundaries, not a replacement API.
Keep checks, prompt generation, fix verification, and savings calculations
provider-neutral. Use trait-object registries rather than cross-product match
tables.

## Scope

- Read existing local files and databases only.
- Do not install hooks, plugins, collectors, or runtime subscriptions.
- Do not launch agents to discover missing evidence.
- Keep checks independent of agent identity unless intentionally exclusive.
- Recommend an automatic edit only when applicability and safety are established.
- Otherwise provide a bounded prompt with the reason automatic remediation is unavailable.
- Apply edits only through explicit invocation, never during scanning.
- Track verified fixes, external fixes, recurrence, and potential token-equivalent savings.
- Keep Phases 1 and 2 backend-only.

Missing evidence remains unavailable. Enabling a check for every agent must not
mean returning a misleading clean result.

## Phase 1: Fix agent and provider coverage

Phase 1 targets full passive-analysis support for Claude Code, Codex, OpenCode,
Pi, Cursor, and Antigravity. Full support means that each native source has a
dedicated bounded reader and every fact it saves reaches the applicable check.
It does not mean that all nine checks must return a result when an agent does
not save the required fact.

Do not remove a working finding while coverage expands. A correction from a
false clean result to partial evidence is not a regression. Before a check is
left unavailable for one of the six target agents, inspect all relevant passive
native sources and ask the maintainer to approve that exact agent/check pair.

The living coverage contracts are:

- [`docs/session-coverage.md`](../session-coverage.md) for discovery, framing,
  parsing, companions, versions, and provider-route extraction.
- [`docs/check-coverage.md`](../check-coverage.md) for the nine checks and their
  evidence limits by exact source format.

### 1.1 Establish capability contracts

Replace coarse agent-wide assumptions with capabilities tied to the source
format, version, and observed session evidence.

| Concept | Meaning |
| --- | --- |
| Source support | The reader understands this fact in this source format |
| Evidence coverage | This session contains sufficient, complete observations |
| Model policy | The catalog has reviewed semantics for evaluating these observations |

A reader must not claim complete coverage merely because its agent usually
supports a feature.

Split the current coupled evidence groups:

- Model identity, occurrence counts, and timing from token accounting.
- Loaded skills from MCP servers and built-in tool definitions.
- Reasoning effort from model identity, while preserving request-level association.
- Service tier from reasoning effort and observed latency.
- Delegation relationships from ordinary event-parent or fork relationships.
- Cache accounting from agent identity.

```rust
enum Support<T> {
    Supported(T),
    Unsupported { reason: CapabilityReason },
    Unknown { reason: CapabilityReason },
}
```

Preserve the existing complete/partial evidence distinction. Unsupported
capabilities and incomplete session data are different conditions.

### 1.2 Add a data-driven model catalog

Resolve capabilities for a provider connection and model, not just a model
family. A resolved definition contains the relevant subset of:

- Canonical model identity and aliases.
- API/accounting format.
- Supported reasoning controls and native value mappings.
- Supported service tiers.
- Context limits.
- Reviewed premium-model and replacement policies.
- Applicable pricing and its provenance.
- Capability and policy revisions.

Use local agent catalogs where their format is understood, plus reviewed
catalog data. Declared availability does not prove that an account can use a
model successfully. Reuse existing pricing and policy infrastructure rather
than creating a second pricing store.

Important rules:

- An OpenAI-compatible endpoint does not automatically support every OpenAI option.
- Claude on Anthropic, Bedrock, or another route can expose different features.
- Resolve OpenCode variant names to options; arbitrary names are not effort levels.
- Resolve Pi thinking levels through the model's supported mappings.
- A model's highest supported effort is not automatically excessive.
- Unknown models must not silently become non-premium or current.

Adding a provider that uses an existing protocol should usually require catalog
entries and tests, not another configuration editor. New protocol semantics
require a focused implementation behind the catalog boundary.

### 1.3 Repair shared evidence bugs

Land these before widening check eligibility:

| Gap | Change |
| --- | --- |
| Model checks depend on token accounting | Allow model-only assessment when identity and timing are sufficient |
| Context sources share one skill/MCP gate | Give each resource kind independent coverage |
| Codex context extraction does not reach check evidence | Connect existing skill listings, harness version, and tool-catalog observations |
| MCP attribution uses substring matching | Preserve exact server identity and native tool-to-server mappings |
| Namespaced skill identities are truncated or matched ambiguously | Preserve full identity and resolve aliases only when unambiguous |
| Cursor synthesis discards structured tool data | Preserve supported calls, attribution arguments, record identities, and timestamps |
| Cursor advertises cache-write support without sufficient usage evidence | Correct its source-specific capability |
| Cline analysis reads metadata without the transcript companion | Load relevant sources together and fingerprint companion changes |
| Cache accounting uses agent exceptions and session-wide flags | Normalize per request or compatible request segment |
| Effort labels lose their model association | Evaluate effort against the model and provider that produced the request |
| Partial effort/speed signals can appear clean | Require coverage of eligible post-boundary activity before verifying absence |

Preserve bounded reads, streaming behavior, and resume/full-read equivalence.

### 1.4 Normalize resource evidence

Use one shared representation for skills, MCP servers, and tools:

```text
Identity and kind
Source and scope
Observation boundary
Configured / available / injected / invoked
Coverage and provenance
Optional token measurement
```

Injected means present in model-facing context. Do not equate:

- Installed with loaded.
- Connected with injected.
- Permission-denied with removed from the request.
- An unused skill listing with an unused full skill document.
- Current configuration with a historical session inventory.

A versioned catalog supports reconstruction only when the session preserves
the inputs needed to select the correct definitions and enabled surface.
Otherwise return partial or unavailable evidence.

### 1.5 Expand all nine checks

| Check | Required coverage work | Remaining boundary |
| --- | --- | --- |
| Session overdepth | Preserve actual request context for Claude, Codex, OpenCode, and Pi; improve Antigravity coverage; add native formats with request usage | Lifetime session totals cannot establish request depth |
| Model overthinking | Resolve OpenCode variants and Pi mappings; add explicit effort from characterized persisted records; add reviewed provider policies | Thinking text, mode names, and latency are not effort settings |
| Overpowered subagents | Preserve actual parent/worker model relationships; connect Antigravity sidecars; add Copilot and Cursor delegation evidence where saved | A fork or parent event ID does not establish delegation |
| Unused MCP servers | Generalize loaded-inventory observations and exact call attribution | Current config and observed calls alone cannot prove historical non-use |
| Unused built-in tools | Connect Codex's version/catalog path; characterize other tool surfaces and deferred loading | A generic catalog is insufficient without the effective enabled surface |
| Unused skills | Connect Codex listings; normalize native invocation/injection and reliable origins | Loaded lists and invocation coverage are both required |
| Old model usage | Remove token-accounting coupling; add identities from dedicated readers; expand reviewed replacement data | Unlisted models are unknown to the policy, not automatically current |
| Fast mode overuse | Preserve explicit delegated service-tier evidence; add agents where effective speed settings are saved | Do not infer fast mode from a fast model, latency, or routing preset |
| Cache churn | Correct per-API tokens, ordering, and mixed-provider handling; extend reviewed accounting/policy | Cache misses alone do not establish a user-fixable cause |

Keep detector IDs stable. Coverage work must not silently change check meaning.
Threshold or policy changes need separate tests and must not create a false win.

### 1.6 Agent work packages

Every supported agent now has a dedicated bounded reader and explicit source
format. Copilot, Cline, Kiro, Amp, and Windsurf remain fail-closed because the
repository has no fixtures that prove detector-grade semantics. Their unproved
capabilities stay unavailable until reviewed contracts and fixtures exist.

The six target agents must reach the maximum coverage their passive sources can
support. Research another native file, database, or companion before proposing
that a missing check remain unavailable. Do not use mutable current
configuration as historical evidence.

| Agent | Work |
| --- | --- |
| Claude Code | Preserve request-level provider, effort, and speed. Join characterized `Task` calls to child sidecars. Parse MCP, built-in, and skill exposure changes. Resolve the effective tool surface by version and model. Segment cache evidence by request and route. |
| Codex | Preserve provider, effort, and service-tier inheritance for parent and child requests. Characterize thread-scoped dynamic tools for MCP and built-ins. Connect skill listing, injection, and invocation evidence. Reject incomplete accounting shapes from clean coverage. |
| OpenCode | Parse native `subtask` records instead of treating all ancestry as delegation. Resolve variants by provider and model. Preserve session version, explicit tool settings, skill origin, and exact tool attribution. Characterize resource inventories and SQLite accounting. |
| Pi | Resolve thinking levels by provider, API, and model. Preserve bounded skill invocation identity. Research passive delegation, resource inventory, effective tool surface, speed, and repeated-context evidence. Keep continuation and fork ancestry separate from delegation. |
| Cursor | Characterize CLI agent JSONL, CLI store DB, IDE composer, and legacy chat separately. Preserve structured calls, arguments, record identities, usage, settings, model changes, and worker relations where each native source saves them. |
| Antigravity | Characterize brain JSONL, cascade JSON, workspace chat, and native SQLite separately. Preserve request identities, retries, token classes, model enums, and timestamps. Connect a relationship companion only after its passive pairing and provenance are proved. |
| Copilot CLI | Add a dedicated reader for persisted model/effort changes, subagent configuration, skill invocation, and MCP attribution |
| Copilot IDE | Characterize separately from CLI; do not reuse CLI coverage declarations |
| Cline | Repair metadata/transcript pairing and change detection; support characterized JSON and database variants independently |
| Kiro | Characterize canonical/chat evidence, add CLI discovery where supported, and account for versioned tool/resource semantics |
| Amp | Add whole-thread parsing; distinguish routing modes from models, effort, and speed; keep file-change fallback coverage narrow |
| Windsurf/Cascade | Characterize native files or existing exports; path recognition must not claim protobuf analysis support |

Copilot's official schema marks per-call usage and loaded skill/MCP inventories
as transient. These events cannot be recovered from the persisted event log.
Other existing files may provide equivalent evidence, but support remains
unavailable until demonstrated.

The same rule applies where other agents expose facts only through runtime
hooks or extensions. Those integrations are outside this plan.

### 1.7 Acceptance

- Every check has an explicit coverage result for every supported source format.
- Every source format has an explicit discovery and parsing result in
  `docs/session-coverage.md`.
- Every newly enabled combination has synthetic positive, negative, and incomplete-evidence fixtures.
- Full and resumed reads produce equivalent evidence.
- Tests cover model changes, mixed providers, duplicates, forks, deferred tools, malformed records, and missing fields.
- Unsupported versions never trigger a generic interpretation that produces clean results.
- A checked-in coverage matrix records supported source versions and remaining gaps.
- Partial source coverage can support safe findings but cannot produce a clean result.
- A target-agent check remains unavailable only after passive-source research
  and explicit maintainer approval for that agent/check pair.
- No new runtime capture or UI is introduced.

### 1.8 Implementation order

1. Make source capabilities represent complete, partial, and unavailable facts.
2. Add contract tests that bind every `SourceFormat` to parsing and check coverage.
3. Repair shared resource identity, request-level model controls, and cache segments.
4. Complete Claude Code and Codex without removing valid existing findings.
5. Complete OpenCode native delegation, model controls, resources, and SQLite accounting.
6. Complete Pi model controls and investigate each missing passive fact.
7. Rework Cursor synthesis and add characterization suites for every native surface.
8. Complete Antigravity protobuf, companion, model, delegation, and cache contracts.
9. Ask for individual disable approval only when passive research proves a fact is absent.
10. Update both living coverage documents from the implemented and tested result.

## Phase 2: Implement remediation for all checks

### 2.1 Define shared operations

Use typed operations rather than provider-specific setting paths in detectors:

```text
SetModel
SetReasoning
SetWorkerModel
SetWorkerServiceTier
SetCompactionPolicy
DisableMcpServer
SetSkillVisibility
RestrictToolSurface
```

The check selects an intent and target. `ModelCatalog` validates model-related
semantics. `AgentConfigEditor` translates the validated change into native
configuration. Agent-only changes do not need provider feature validation.

Keep native option encoding inside the implementation boundary. API callers
must not supply arbitrary JSON pointers, paths, commands, or replacement bytes.

### 2.2 Select automatic remediation first

Return one primary recommendation:

```rust
enum Recommendation {
    Automatic {
        preview: EditPreview,
        fallback_prompt: AgentPrompt,
    },
    Prompt {
        prompt: AgentPrompt,
        automatic_unavailable: CapabilityReason,
    },
}
```

Select automatic remediation only when all gates pass:

- The finding identifies a specific target.
- The proposed change addresses the observed issue.
- The native setting and applicable version are supported.
- The effective configuration scope is known.
- The replacement is valid for the selected provider/model.
- No unresolved override makes the edit ineffective.
- The change is narrow and preserves unrelated settings.
- The activation boundary and verification method are defined.
- The recommendation does not depend on an unsupported quality or workload assumption.

Otherwise return the prompt and the precise limiting reason. Do not add a
numeric confidence score.

Unused in one session does not establish that disabling a resource is safe.
Use a prompt unless stronger scoped evidence or explicit user selection
establishes the intended change.

Automatic means callable after explicit approval, not background mutation.
An apply conflict requires a fresh preview. Never execute the fallback prompt
or choose another edit automatically.

### 2.3 Provide a strategy for every check

| Check | Automatic option when eligible | Prompt fallback | Verification |
| --- | --- | --- | --- |
| Session overdepth | Supported compaction/context policy | Bounded handoff or context-policy review | New relevant requests have appropriate context; retain the historical maximum |
| Model overthinking | Lower effective effort for the selected scope | Review effort or bound the task | Explicit subsequent effort on eligible requests |
| Overpowered subagents | Validated lower-cost model for a named worker | Review worker selection and bounded delegation | Actual worker model and parent relationship |
| Unused MCP servers | Disable a selected optional server in scope | Named-server keep/disable audit | Complete subsequent evidence shows model-facing definitions absent |
| Unused built-in tools | Restrict the injected tool surface | Task-specific tool audit | Definitions absent, not merely permission-denied |
| Unused skills | Supported visibility/enablement change | Named-skill audit and narrow proposal | Complete context evidence reflects intended visibility |
| Old model usage | Validated replacement for an explicit model | Replacement and compatibility review | Actual subsequent replacement use |
| Fast mode overuse | Standard service for the relevant worker | Effective worker-speed review | Explicit standard-tier delegated activity |
| Cache churn | Specific supported change after diagnosis | Bounded cache/input diagnosis | Sufficient comparable post-fix request evidence |

All nine checks receive a strategy. This does not promise an automatic edit or
numeric savings for every finding. Do not broaden a worker-specific fix into a
global edit merely because the agent lacks a worker-specific setting.

### 2.4 Implement safe native editors

Register only implemented editors. Missing support selects the prompt.

Each editor must:

- Resolve user, project, local, managed, and supported override layers.
- Detect the native schema/version.
- Preserve comments, formatting where practical, unrelated keys, and permissions.
- Validate regular-file targets, ownership, and the chosen symlink policy.
- Keep reads and previews bounded.
- Redact secrets from previews, errors, and logs.
- Reject stale previews and unsupported formats.
- Apply a minimal atomic update with tested concurrency behavior.
- Read back the result and distinguish written configuration from effective behavior.

A pre-write comparison alone does not prevent every concurrent external write.
Refuse automatic application where the implementation cannot meet its
documented safety contract.

Start with single-file edits. Do not build multi-file transactions or automatic
undo infrastructure. Retain only non-secret action metadata needed for
verification. Full configuration snapshots remain in memory.

Implement verified operations across Claude, Codex, OpenCode, and Pi first.
Add Cursor, Kiro, and other editors as native contracts are established.
Provider additions should reuse these editors.

### 2.5 Generate bounded prompts

Use deterministic templates over typed evidence. Do not call another model to
generate remediation prompts.

Each prompt contains:

- The observation and its scope.
- A small set of relevant facts or resource identities.
- One bounded objective.
- Necessary quality and permission constraints.
- A request to inspect native support before proposing or applying a change.
- A short verification request.

Target approximately 200-350 words with a hard size cap. Exclude full
transcripts, configuration files, credentials, and unrelated project history.
Treat extracted content as quoted data.

State limitations honestly. Asking an agent to use less reasoning does not
prove that its configured effort changed.

### 2.6 Persist fix episodes

Persist fix history separately from cumulative reports:

```text
open -> awaiting_verification -> fixed
                                  |
                                  v
                               recurred
```

Evidence freshness and availability remain separate from lifecycle state.

Store:

- Detector and target identity.
- Full agent/environment identity and applicable scope.
- Baseline facts and relevant revisions.
- Applied action metadata, if any.
- Effective behavior boundary.
- Verification time and evidence.
- Recurrence boundary.
- Savings method and supported aggregates.

Track external fixes through the same verifier. Label them as observed
improvements rather than attributing them to antiburn. When application time
is unknown, use a conservative supported observation boundary.

A successful write is not a verified fix. Require fresh, relevant post-boundary
work. Never infer a fix from inactivity, deletion, report aging, source failure,
changed policy, or corrected historical accounting.

### 2.7 Reuse background processing

Use the existing evidence worker and startup reconciliation:

- Mark affected assessments dirty inside the successful publication transaction.
- Evaluate outside the database write lock.
- Guard persistence against stale results.
- Reconcile unfinished work after restart.
- Use assessment/source revisions for idempotency, not the published fence as a unique behavioral event.
- Generate prompts and previews on demand.

Do not add a separate remediation queue, worker, lease system, scheduler, or
service. Tracking must work without any report window being open.

### 2.8 Calculate potential savings

Support potential token-equivalent savings with an explicit underlying method:

```text
Potential tokens avoided
Potential API-equivalent cost avoided
Potential token-equivalent savings
```

For price-only changes, compare observed post-fix token classes under the
previous and fixed settings. Convert the supported cost difference into a
clearly labeled token equivalent. This is a counterfactual estimate, not proof
of fewer physical tokens, equivalent output quality, or subscription savings.

For resource removal, estimate avoided injected tokens only when definition
size and subsequent exposure are known. For behavioral changes such as lower
effort or handoffs, return unknown unless a defensible comparison exists.

Rules:

- Accrue against relevant activity, never idle time.
- Preserve measured zero separately from unknown.
- Use no arbitrary percentage fallback or positive minimum.
- Pin the rates and method used for each estimate.
- Retain cost increases rather than claiming positive savings.
- Close the interval at recurrence.
- Prevent overlap between model/speed changes and parent/child usage.
- Recompute idempotently so retries and rereads cannot duplicate savings.
- Permit a verified fix without a numeric estimate.

### 2.9 Expose backend operations

Keep the API small:

```text
Get recommendation
Apply approved preview
Record externally applied change
Get fix episodes and savings
```

Follow repository conventions for IPC names. No operation accepts arbitrary
filesystem targets or shell commands. Do not connect these operations to
existing UI surfaces during this phase.

### 2.10 Full test suite and acceptance

Use shared contract suites plus implementation-specific synthetic fixtures.

| Area | Required coverage |
| --- | --- |
| Recommendation selection | Automatic eligibility, prompt fallback, precise reasons, all nine checks |
| Provider extensibility | New catalog provider works through existing editors; unknown capabilities block edits |
| Config editors | Precedence, versions, comments, unrelated keys, permissions, secrets, malformed files, unsafe paths, conflicts, failed writes, readback |
| Verification | Historical failure remains; new behavior fixes it; application alone does not |
| External fixes | Unknown application time, conservative boundaries, correct scope |
| Persistence | Restart recovery, stale results, duplicate publications, migrations, deletion, retention |
| Accounting | Missing timestamps, late imports, mixed providers, pinned prices, recurrence, zero/unknown, no double counting |
| Privacy | No transcripts or credentials in previews, prompts, action records, or logs |
| Performance | Bounded queries and retained evidence; no dependency on an open window |

Run `aislop scan --changes`, then formatting, Clippy, and tests in the engine
and desktop Rust workspaces. Run desktop lint, type checks, tests, and build
when IPC mirrors change. Follow `CONTRIBUTING.md` and the desktop README.

Acceptance: every surfaced finding has a recommendation path; automatic edits
are explicit and safe; fixes survive restarts; savings are evidence-based and
idempotent; no UI changes are present.

## Phase 3: Remediation UI/UX (deferred)

After backend contracts stabilize, design the automatic-fix preview/apply flow,
prompt fallback, awaiting-verification state, verified wins, recurrence, and
potential token-equivalent savings presentation.

Preserve the distinction between configuration applied, behavior verified
fixed, and estimated savings. No UI/UX implementation belongs in Phases 1 or 2.

## Research and implementation references

Research date: 2026-09-07. Upstream references establish opportunities, not
guarantees about every installed version. Pin supported schema versions and
test exact behavior before enabling readers or writers.

### Repository

- [Existing session adapter](../../crates/antiburn-local/src/analysis/interface.rs)
- [Reader registration](../../crates/antiburn-local/src/analysis/vendors/mod.rs)
- [Evidence capabilities](../../crates/antiburn-local/src/analysis/evidence.rs)
- [Evidence normalization and attribution](../../crates/antiburn-local/src/analysis/evidence_sink.rs)
- [Existing context extraction](../../crates/antiburn-local/src/analysis/initial_context.rs)
- [Check identifiers](../../crates/antiburn-local/src/insights/status.rs)
- [Check policies](../../crates/antiburn-local/src/insights/detectors/mod.rs)
- [Current burn estimator](../../crates/antiburn-local/src/insights/report.rs)
- [Publication and persistence](../../apps/desktop/src-tauri/src/store/mod.rs)
- [Evidence worker](../../apps/desktop/src-tauri/src/insights_worker.rs)

### Upstream

- [Claude settings reference](https://code.claude.com/docs/en/settings-reference)
- [Codex configuration](https://developers.openai.com/codex/config-reference)
- [Codex skills](https://developers.openai.com/codex/skills)
- [OpenCode models](https://opencode.ai/docs/models/)
- [OpenCode V2 agents](https://opencode.ai/v2/docs/agents)
- [Pi session format](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/session-format.md)
- [Pi model mappings](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/models.md)
- [Cursor subagents](https://cursor.com/docs/subagents.md)
- [Antigravity MCP](https://antigravity.google/docs/mcp/)
- [Copilot event schema](https://github.com/github/copilot-sdk/blob/main/nodejs/src/generated/session-events.ts)
- [Cline CLI](https://docs.cline.bot/cli/cli-reference)
- [Kiro agent configuration](https://kiro.dev/docs/custom-agents/configuration-reference/)
- [Amp modes and models](https://ampcode.com/docs/models-and-subagents)
- [Cascade skills](https://docs.devin.ai/desktop/cascade/skills)
- [Google cache accounting](https://ai.google.dev/gemini-api/docs/generate-content/caching)
