# Burn check coverage and remediation

Status (2026-09-08): Phase 1 implementation and full-suite validation are complete
for the scoped baseline. Merged-worktree validation remains pending. Phase 2 is planned as full backend
remediation in four bounded substeps; no Phase 2 code is implemented. Start it
after Phase 1 acceptance. Phase 3 UI remains deferred.

## Naming and architecture

The reader API is `SessionReader`; the model-provider abstraction is
`ModelCatalog`, not `ProviderAdapter`. Only the new reader API is supported.
The maintainer approved removal of `VendorAdapter` and related aliases; the
[engine changelog](../../crates/antiburn-local/CHANGELOG.md) records the breaking
change. Do not restore compatibility aliases.

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

The reader retains streaming, snapshot, and resume methods. These illustrative
signatures describe boundaries, not a replacement for the implemented API.
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
- Keep remediation backend-only until the separate UI phase.

Missing evidence remains unavailable. Enabling a check for every agent must not
mean returning a misleading clean result.

## Phase 1: Fix agent and provider coverage

The only Phase 1 targets are OpenCode, Pi, Codex, Claude Code, and Antigravity.
The baseline covers accepted persisted shapes and approved source-scoped limits,
not all historical versions or every native schema. Cursor and other agents are
deferred and retain their basic current support. All 26 source keys remain in
the living inventories.

Do not remove a working finding while coverage expands. A correction from a
false clean result to partial evidence is not a regression. Before a check is
left unsupported for a target agent, record the passive alternatives reviewed
and maintainer confirmation for that named check. The dated ledger below is the
current decision record, not a claim of future impossibility.

The living coverage contracts are:

- [`docs/session-coverage.md`](../session-coverage.md) for discovery, framing,
  parsing, companions, versions, and provider-route extraction.
- [`docs/check-coverage.md`](../check-coverage.md) for the nine checks and their
  evidence limits by exact source format.

### Implemented baseline

| Area | Current result |
| --- | --- |
| Source contracts | Exact source formats separate reader support, complete/partial session evidence, and reviewed policy. The manual inventories contain all 26 keys; machine inventory and behavior tests are available. |
| Model catalog | Provider/API/model resolution retains unknown states and reuses reviewed pricing/replacement data. Compatible protocol names alone do not authorize options. |
| Claude and Codex core | D/T/S/O/F/C can assess complete accepted sessions on reviewed routes. Possible incomplete sessions do not make the entire source Partial. Request controls and delegated scope remain associated with their actual models. |
| Claude delegation | Exact Task/Agent call IDs pair with unique sidecar `toolUseId` and actual child models. Sidecar presence/content affects scan cursors, analysis fingerprints, resume joining, and publication guards. |
| Resources | All M/B/K checks deny session-wide clean without full historical inventories. Scoped findings require complete observed subsets and calls. Claude M/K and Codex exact `tool_search_output` MCP exposure follow this rule. |
| Skills and built-ins | Full injected skill documents are separate from listings. OpenCode selected skills and Codex selected documents remain observed/invoked, not unused listings. Claude/Codex catalog-backed built-in findings stay scoped; the catalog is not whole-inventory proof. |
| OpenCode | Native task metadata plus matching ancestry and actual child model prove delegation. Both accepted exports and SQLite use compatible-request cache accounting and validated order, not `parentID` as predecessor. |
| Pi | T explicitly uses `AgentSelectedPolicy`, reviewed native provider/API names, and branch/fork state. Official example-extension nested `toolResult` output provides actual worker-model findings only; arbitrary extensions cannot prove clean. |
| Cache persistence | Pi and OpenCode provider/API fields survive durable turn rows and feed the shared compatible-request query. Unknown routes, incompatible history, and compactions prevent clean. |
| Partial sources | Cursor and Antigravity preserve direct D/O findings where evidence exists and deny clean by source gate. Missing model/time is not borrowed from earlier records. Cursor's explicit synthesized header model remains supported. |

### Confirmed limits

The [2026-09-08 confirmation ledger](../check-coverage.md#confirmation-ledger)
records named checks, alternatives reviewed, and pinned citations:

- Pi M/B/K/F are unsupported; no alternative local inventory or speed proof was found.
- OpenCode M/B/F/T are unsupported; historical inventories, tier, and effort maps are absent from the reviewed sources.
- Antigravity T/S/M/B/K/F/C are unsupported; reviewed native sources and runtime descriptors provide no alternative persisted proof.
- These are source-scoped decisions, not future impossibility claims. Workspace and unknown shapes remain uncharacterized.

Open gaps remain explicit: CoreV2 `session_message` is not the existing
`OpenCodeSqliteV2` `session`/`message`/`part` reader; OpenCode WSL discovery still
uses an executable export path; Cursor native synthesis and Antigravity private
identity/model/linkage coverage remain incomplete. Other agents' broader
parsing and companion work is deferred, not completed by reader registration.

Known accepted shapes do not require an unobtainable universal version range.
Schema/header or pinned producer evidence plus synthetic fixtures defines the
boundary. Unknown changed evidence fails partial or unavailable.

### Validation record

Pre-merge validation passed on 2026-09-08:

- Engine formatting, strict Clippy, 1,336 unit tests, all integration suites,
  and doctests. Two existing ignored tests did not run.
- Desktop Rust formatting, strict Clippy, and 937 tests.
- Frontend lint, type checks, 1,144 tests, and production build.
- Coverage contract, resume/replay, provider changes, forks, scoped resources,
  and sidecar scan/publication regressions.
- Changed-code and full-project quality scans, secrets scan, and whitespace checks.

Repeat acceptance against the merged worktree before pushing. Documentation
inventory tests do not prove all matrix cells or all installed versions. The
existing frontend chunk-size warning does not fail the build.

## Delivery Order

| Phase | Scope | Status |
| --- | --- | --- |
| 2.1 | Typed current findings, exact selectors, deterministic bounded prompts | Planned; gated on Phase 1 acceptance |
| 2.2 | Minimal native editors: inspection, preview, explicit apply, readback | Planned; builds on 2.1 |
| 2.3 | Durable episodes, post-boundary verification, external fixes, recurrence | Planned; builds on 2.1 and 2.2 |
| 2.4 | Potential savings and stable typed backend IPC | Planned; builds on 2.3 |
| 3 | Remediation UI/UX after backend acceptance | Deferred |

Phase 2 must not turn partial coverage, a selected resource, or a catalog guess
into a new finding. It consumes only verified existing findings and preserves
their exact scope. All four substeps belong to Phase 2. Deliver and validate
them separately; prompts alone do not complete backend remediation.

## Phase 2: Backend Remediation

The following contracts are planned, not implemented capabilities. Use the five
Phase 1 targets and their accepted shapes. Do not expand reader coverage, add
collection, or build UI to make remediation appear complete. Unknown and
confirmed unsupported checks return a typed unavailable reason, not an action
based on missing facts. A prompt fallback applies to an actual finding, not to
an absent finding.

### 2.1: Typed findings and prompts

Build the input from the current published assessment, not report prose or a
cumulative badge. Retain typed check facts, exact finding scope, coverage and
policy semantics, and the evidence references needed to revalidate them.

- Identify the agent, environment/profile, workspace, exact `SourceFormat`,
  session/thread, request or resource, and delegated parent/call/child where applicable.
- Retain provider/API/model and controls on the actual request. Never substitute
  a requested alias, parent default, fork relation, or current setting for actual child evidence.
- Bind the finding to source generation, published fence, projection/schema
  revisions, detector/catalog policy revisions, and assessment revision.
- Require analyzed generation to match current source generation and all
  relevant revisions to match before returning an actionable recommendation.
- Read evidence and turn rows from the same published fence. A pending claim
  or an older ready row cannot authorize an action against a newer source.
- Include contributing companion fingerprints. Claude sidecar additions,
  removals, or changed claims invalidate prior parent-child recommendations.
- Recheck the binding before preview, apply, and episode-result persistence.
  Stale or unavailable evidence requires reassessment, not a fallback edit.

Use an opaque finding ID and a server-resolved typed target selector. Display
names and model aliases alone are not selectors. Automatic proposals require
both a current verified finding and an exact selector; user selection can
narrow an established finding but cannot supply missing historical proof.
"Verified finding" means a currently supported detector result, not a fixed
episode. A historical finding alone does not prove the same setting is active now.

Local configuration inspection establishes only the current edit target,
precedence, and activation requirements. It cannot backfill historical effort,
resource exposure, service tier, delegation, or cache linkage. CoreV2
`session_message` remains outside the accepted OpenCode reader. Do not call the
OpenCode WSL executable export path from remediation; its passive-access gap
remains unresolved. Unsupported environments receive an explicit reason.

### Shared operations

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

### Recommendation selection

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
- The assessment is current under the generation, fence, and revision rules above.
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

### Per-check strategies

The observation column covers all five targets. CC = Claude Code, CX = Codex,
OC = OpenCode, PI = Pi, AG = Antigravity. D/T/S/M/B/K/O/F/C use the coverage
document's check codes. Every editor below is conditional on a pinned native
contract, exact current target, and all safety gates; none is promised today.

| Check | Available observations across five targets | Safe prompt for a current finding | Conditional native editor | Verification feasibility | Savings constraints |
| --- | --- | --- | --- | --- | --- |
| D: Session overdepth | CC/CX/OC/PI: accepted request depth. AG: direct positive depth only where present, never clean. | Propose a bounded handoff or review context policy; retain necessary task state. | Supported scoped compaction/context policy; no destructive session reset. | Relevant new requests below the reviewed threshold; retain historical maximum. AG cannot establish a clean fix with its present source gate. | Unknown without comparable workload/context evidence; a smaller request alone does not prove avoided tokens. |
| T: Model overthinking | CC/CX: request model, effort, route, inherited controls. PI: branch-local `AgentSelectedPolicy`. OC/AG: unsupported. | Review the observed level for this task and scope, without asserting provider-translated Pi effort. | CC/CX effective scoped effort; PI native thinking-policy setting only under its reviewed semantics. | Explicit post-boundary control on eligible requests. PI verifies saved agent policy, not final provider effort or quality equivalence. | Unknown without defensible comparable activity; no fixed percentage reduction. |
| S: Overpowered subagents | CC: exact Task/Agent ID and unique sidecar claim. CX: owned spawn and child rollout. OC: task metadata, ancestry, actual child model agree. PI: official example-extension nested results, positive-only. AG: unsupported. | Review a named worker's model and bounded task; preserve quality and required capabilities. | Exact persistent worker model only; no global substitution when worker scope is unavailable. PI needs a reviewed existing extension config contract, not extension installation. | New exact parent/call/child activity with actual model. PI positive-only evidence cannot prove a clean fix under current coverage. | Price counterfactual only with owned child usage and reviewed rates; no parent/child duplication or assumed equal quality. |
| M: Unused MCP servers | CC: observed injection and exact calls. CX: completed exact `tool_search_output` namespace exposure and calls. OC/PI/AG: unsupported. | Audit only the named observed optional server; ask whether it is needed beyond this task. | Disable an explicitly selected optional server in the proven scope; inactivity alone does not authorize disabling. | Complete later evidence must prove the selected definitions absent. No full inventory exists; missing observation is not removal or session-wide clean. | Unknown unless definition token size and subsequent avoided exposure are supported. |
| B: Unused built-in tools | CC/CX: harness/model catalog-backed scoped definitions and complete calls; exclude deferred, situational, zero-cost definitions. OC/PI/AG: unsupported. | Audit only the supported observed tool surface for this task. | Native definition/exposure restriction, not a permission-denial setting masquerading as removal. | Complete targeted exposure proof is required; permission denial or no calls does not verify absence. No session-wide clean. | Unknown without measured definition size and comparable exposure; catalog membership alone is insufficient. |
| K: Unused skills | CC: full document injection plus invocation identity can support scoped findings. CX/OC: selected documents are injected and invoked, not unused listings. PI/AG: unsupported. | Audit a named full injected document only when an unused-skill finding exists; do not propose removal from a listing. | Supported visibility/enablement edit for an explicitly selected document and scope. | Complete targeted context evidence must prove intended visibility. Selected-only evidence and missing documents cannot prove absence; no session-wide clean. | Unknown without document token size and proved avoided injection; never retain private document bodies for estimates. |
| O: Old model usage | CC/CX/OC/PI: actual timed model use and reviewed replacement policy. AG: direct positive timed model use only; no borrowed model/time or clean. | Review the exact replacement's compatibility and task requirements. | Exact scoped model setting with reviewed provider/API support; worker findings stay worker-scoped. | Actual subsequent replacement use in the same applicable scope. AG may show an improvement but cannot establish clean with current gaps. | Compare supported post-fix token classes at pinned old/new rates; increases remain negative and unknown rates stay unknown. |
| F: Fast mode overuse | CC/CX: actual delegated model, speed/tier, route, inherited controls. OC/PI/AG: unsupported. | Review speed needs for the identified worker; do not change global service by default. | Explicit standard-tier control for that worker, if native precedence and activation are known. | Explicit post-boundary standard-tier delegated requests, not missing tier or a parent default. | Known tier-price difference on owned usage only; overlap with model changes must be accounted once. |
| C: Cache churn | CC/CX/OC/PI: durable route, token classes, compatible ordered main-thread history and compaction boundaries. AG: unsupported. | Diagnose bounded input/cache behavior without claiming a cause from token totals alone. | Only a diagnosed cause with a supported scoped setting; otherwise prompt-only. | Sufficient comparable requests on a reviewed route; exclude unknown/mixed routes, unresolved forks, compactions, late or malformed ordering. OC `parentID` is not a predecessor; Google cache policy is unreviewed. | Unknown without comparable cache accounting and a supported causal comparison; no generic cache-hit savings ratio. |

All nine checks receive a strategy. This does not promise an automatic edit or
numeric savings for every finding. Do not broaden a worker-specific fix into a
global edit merely because the agent lacks a worker-specific setting.

Resource scope is the complete observed subset plus complete calls and eligible
activity, not a full historical inventory. Current config absence cannot fill
that gap. When accepted post-boundary evidence cannot prove targeted absence,
keep the episode awaiting verification with an unavailable reason. Likewise,
positive-only AG D/O and PI S observations can show improvement but do not
override their clean-result limits to mark a fix verified.

### 2.2: Minimal native editors

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

For CC/CX/OC/PI, first inspect native current config contracts without writes.
Record each supported operation's schema/version, precedence, exact target,
activation boundary, verification evidence, and fixture. Enable one single-file
operation at a time only when this record and its tests pass. Unsupported layers,
managed policy, environment/CLI overrides, ambiguous worker routing, or unknown
Pi extension settings remain prompt-only with specific reasons. AG is
prompt-only unless a safe current native edit contract is separately established;
this does not expand its D/O-only finding support. Other agents remain deferred.

Bind previews to the finding revision, opaque target ID, config fingerprint,
typed change, and expiry. Reinspect effective scope at apply; reject changed
content, identity, overrides, expired approval, or stale evidence. Do not retry
a conflicting write with a new target. Record explicit approval and distinguish
no-op, conflict, failed write, successful readback, and effective activation.

### Bounded prompts

Use deterministic templates over typed evidence. Do not call another model to
generate remediation prompts.

Each prompt contains:

- The observation and its scope.
- A small set of relevant facts or resource identities.
- One bounded objective.
- Necessary quality and permission constraints.
- A request to inspect native support before proposing or applying a change.
- A short verification request.

Target approximately 200-350 words with a hard cap of 8 KiB UTF-8 for the entire
prompt, including quoted data. Include at most eight resource identities, each
at most 256 UTF-8 bytes. Omit excess examples with a count; never truncate an
exact action selector into a different identity. If essential facts cannot fit,
return a typed size-limit reason instead of an unsafe prompt. Exclude full
transcripts, configuration files, credentials, and unrelated project history.
Treat extracted content as quoted untrusted data, never instructions. Strip
control characters, exclude secrets and raw private paths, and use sanitized
display labels separate from exact backend selectors. Deterministic ordering
must give identical output for identical typed inputs.

State limitations honestly. Asking an agent to use less reasoning does not
prove that its configured effort changed.

### 2.3: Episodes and verification

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
- Source generation/fence and exact selector binding, separate from the episode ID.
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

Define each verifier's minimum eligible post-boundary activity and completeness
requirements before enabling the operation. Capture the effective boundary
(next request, next session, or restart), not merely the config write time.
Unknown timing cannot assign old requests to a fix. Insufficient evidence keeps
the episode awaiting verification with a reason; positive recurrence requires a
fresh supported finding for the same target. Do not merge sibling workers,
forks, environments, or distinct resource scopes into one episode.

### Background processing

Use the existing evidence worker and startup reconciliation:

- Mark affected assessments dirty inside the successful publication transaction.
- Evaluate outside the database write lock.
- Guard persistence against stale results.
- Compare current generation, published fence, policy/schema revisions, and
  assessment revision at result commit; discard and reevaluate changed inputs.
- Reconcile unfinished work after restart.
- Use assessment/source revisions for idempotency, not the published fence as a unique behavioral event.
- Generate prompts and previews on demand.

Do not add a separate remediation queue, worker, lease system, scheduler, or
service. Tracking must work without any report window being open.

### 2.4: Savings and backend IPC

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

### Backend operations

Keep the API small:

```text
Get recommendation
Apply approved preview
Record externally applied change
Get fix episodes and savings
```

Follow repository conventions for IPC names. No operation accepts arbitrary
filesystem targets or shell commands. Do not connect these operations to
existing UI surfaces before the UI phase.

Requests use opaque finding, preview, and episode IDs plus typed operation and
scope enums. The backend resolves targets from trusted discovery/config context;
reject forged, cross-environment, expired, and stale IDs. Responses distinguish
automatic, prompt-only, unavailable, stale/conflict, applied-awaiting-verification,
fixed, and recurred outcomes with typed reason codes. Savings carry units,
method/rate revision, interval, and known/unknown status. Keep serialized Rust
types and desktop mirrors aligned; do not expose internal config snapshots or
raw evidence bodies. An external-change request records a claim, never an
immediate verified fix. No endpoint executes prompts or fallback commands.

### Tests and acceptance

Use shared contract suites plus implementation-specific synthetic fixtures.

| Area | Required coverage |
| --- | --- |
| Recommendation selection | Automatic eligibility, prompt fallback, precise reasons, all nine checks |
| Phase 1 boundaries | Five-target per-check table; AG D/O positive-only; PI M/B/K/F and OC T/M/B/F unavailable; selected CX/OC skills do not create unused findings |
| Exact identity | CC unique sidecar claim and actual child; CX owned spawn/fork controls; OC metadata/ancestry/model agreement; PI native policy and official extension only; aliases and sibling workers rejected |
| Freshness | New source generation with old ready evidence, in-flight fence, changed sidecars, schema/catalog/detector revisions, stale preview/apply/result; no mixed-fence rows |
| Provider extensibility | New catalog provider works through existing editors; unknown capabilities block edits |
| Config editors | Precedence, versions, comments, unrelated keys, permissions, secrets, malformed files, unsafe paths, conflicts, failed writes, readback |
| Verification | Historical failure remains; eligible new behavior fixes only supported scopes; application alone does not; missing inventory, no activity, positive-only sources, and missing tier cannot prove clean |
| External fixes | Unknown application time, conservative boundaries, correct scope |
| Persistence | Restart recovery, stale results, duplicate publications, migrations, deletion, retention |
| Accounting | Missing timestamps, late imports, mixed providers, pinned prices, recurrence, zero/unknown, no double counting |
| Prompt bounds/privacy | Deterministic output, 8 KiB boundary and overflow, multibyte labels, eight-resource cap, long identities, injection text, controls, secrets; no transcripts/config bodies in outputs or logs |
| IPC | Rust/mirror serialization, typed unknown states, forged IDs, cross-scope approval, expiry, bounded pagination, no arbitrary path/command or automatic fallback execution |
| Known discovery gaps | CoreV2 is not accepted SQLite; WSL executable export is not invoked; unknown shapes/environments stay unavailable |
| Performance | Bounded queries and retained evidence; no dependency on an open window |

### Execution and acceptance

1. Close Phase 1 acceptance on main. Review the two coverage baselines against
   characterization, replay/resume, durable provider/control rows, and desktop
   sidecar/publication tests. Record the revision and actual command results.
   Do not treat inventory tests or this plan review as full acceptance.
2. Implement 2.1 in the engine's typed finding/strategy boundary with desktop
   publication freshness checks. Reuse existing IDs and evidence types where
   valid. Complete deterministic prompt and unavailable-result tests for every
   check/target before introducing writes.
3. Implement 2.2 behind `AgentConfigEditor`. Pin the first native operation's
   accepted config shape and safety contract, then add other proven operations
   across CC/CX/OC/PI. Test preview/apply/readback and conflicts per operation.
   Record unsupported combinations rather than inventing generic writers.
4. Implement 2.3 using existing store migrations and evidence-worker publication
   and reconciliation paths. Add bounded episode records and per-check pure
   verifiers. Test restart, external changes, missing proof, and recurrence
   before counting any savings.
5. Implement 2.4 with pinned counterfactual methods, idempotent usage allocation,
   unknown reasons, and bounded episode queries. Expose typed IPC and mirrors
   only after internal contracts pass; do not wire report or settings UI.
6. Run `aislop scan --changes` after supported code changes, then the relevant
   formatter, lint, type checks, tests, and builds below. Review the combined
   diff for widened evidence claims, current-config-as-history, secret retention,
   and accidental UI work. Record results and remaining limitations per substep.

For implementation validation, use `cargo fmt --check`,
`cargo clippy --all-targets -- -D warnings`, and `cargo test` in both
`crates/antiburn-local` and `apps/desktop/src-tauri`. Run the engine's
`cargo test --test check_coverage_contract` as an explicit coverage guard.
From the root, run `pnpm --filter @antiburn/desktop lint`, `type-check`, `test`,
and `build` as separate commands with the same filter when IPC mirrors change.
Follow `CONTRIBUTING.md` and `apps/desktop/README.md` for current prerequisites.
Any future evidence or coverage change requires the corresponding living
coverage-document update and characterization tests; Phase 2 does not assume
such a change is needed to enable an editor.

Phase 2 is accepted only when all of these outcomes are demonstrated:

- Every current supported finding has a deterministic bounded prompt, or an
  explicit privacy/size reason if essential facts cannot safely be rendered.
  Missing facts and confirmed unsupported checks never produce actions.
- Automatic previews exist only for proven exact current targets. Every enabled
  native operation passes its safety fixtures and requires explicit approval;
  unsupported operations retain a precise prompt-only reason.
- Durable episodes distinguish applied from behavior-verified, track external
  improvements and recurrence, survive restart, and reject stale results.
  Unverifiable scopes remain visibly unverified rather than counted as fixes.
- Savings are evidence-based, pinned, idempotent, and non-overlapping. Unknown,
  measured zero, and negative estimates remain distinct.
- Stable typed IPC has contract tests and no arbitrary file/command escape.
  Tracking works without an open window. No remediation UI is implemented.

Current completion record for this plan revision:

- Documented: scoped Phase 1 implementation baseline, confirmed limits, and
  remaining discovery/source gaps, checked against the living coverage documents.
- Planned: all four Phase 2 substeps, strategy table, safety gates, and acceptance matrix.
- Pending: Phase 1 final full-suite and merged-main validation by the integration owner.
- Pending: all Phase 2 implementation, native operation fixtures, migrations,
  IPC contracts, and implementation test results. This documentation update
  does not claim those tests ran or Phase 1 was finally accepted.

## Phase 3: Remediation UI/UX (deferred)

After backend contracts stabilize, design the automatic-fix preview/apply flow,
prompt fallback, awaiting-verification state, verified wins, recurrence, and
potential token-equivalent savings presentation.

Preserve the distinction between configuration applied, behavior verified
fixed, and estimated savings. No remediation UI implementation belongs in the
backend phase.

## Research and implementation references

Research baseline: 2026-09-08. The
[confirmation ledger](../check-coverage.md#confirmation-ledger) pins the reviewed
OpenCode, Pi, Codex, Antigravity descriptor/adapter, and SDK references. Runtime
SDK schema is not persisted proof. Upstream references below describe future
options, not guarantees about every installed version. Pin accepted shapes and
test behavior before enabling readers or writers.

### Repository

- [Session reader interface](../../crates/antiburn-local/src/analysis/interface.rs)
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
