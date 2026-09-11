# Burn Check Detection and Config Remediation Parity

Status: in progress. Implementation began after maintainer approval.

Research date: 2026-09-11.

Continuation entry point: Section 22 records the follow-up research, maintainer
decisions, concrete implementation owners, and remaining acceptance gates.
Its explicitly recorded decisions supersede the earlier stricter planning
requirements. Research completion is not implementation completion.

## 1. Outcome

Make nearly every applicable burn recommendation actionable through a reviewed,
precise config change. Improve passive detection across OpenCode, Pi, Codex,
Claude Code, Cursor, and Antigravity without inventing historical evidence.
Keep processing fast and memory-bounded across the entire pipeline.

This plan extends the delivered [remediation plan](burn-check-remediation.md).
It does not replace the current [check coverage](../check-coverage.md),
[session coverage](../session-coverage.md), or [remediation guide](../remediation.md).
Those documents remain implementation baselines until tested changes ship.

The maintainer approved these planning decisions on 2026-09-11:

- Include preventive config recommendations separately from detected burn and
  verified savings.
- Permit missing-setting and missing-file creation after explicit review.
- Write the plan before implementation. Implementation began after a later maintainer request.

The target is not nine green checkmarks for every agent. The target is consistent
evidence standards, safe actions wherever documented controls exist, and clear
limits where an agent does not persist the necessary facts.

## 2. Non-Negotiable Boundaries

- Read existing local files and databases only for check evidence.
- Do not install hooks, extensions, plugins, collectors, or runtime subscriptions.
- Do not invoke an agent, MCP server, credential helper, or language-server RPC
  to discover evidence or validate an edit.
- Existing persisted output from a reviewed extension can be accepted. Installing
  or running that extension to manufacture coverage cannot.
- Current config describes current policy, not historical request behavior.
- Keep requested, configured, producer-reported, and actual served values distinct.
- Keep findings, preventive advice, file-write success, behavioral verification,
  and savings separate.
- Do not expand permissions, change trust, weaken sandboxing, or alter account
  routing or credentials to reduce cost or make another edit effective.
- Permit an explicitly reviewed, narrow permission restriction only when it
  removes the intended optional resource and preserves required capabilities.
- Do not edit managed, generated, installed-plugin, or bundled assets by default.
- Do not replace missing evidence with names, timing guesses, text-size estimates,
  mutable catalogs, or defaults from another agent.
- Preserve unknown configuration fields and observed unrelated user changes.
  Concurrent external writes can still race with replacement; use the reviewed
  best-effort write and backup contract in Sections 7 and 22.
- Keep resource absence checks scoped to the inventory actually proved.
- Use synthetic fixtures. Do not commit captured or redacted private transcripts.

OpenCode executable-based WSL discovery is an existing exception in the baseline,
not a precedent. Replace it with passive file/database access in a separate work
item before claiming passive WSL parity.

## 3. Research Method and Confidence

The review inspected local detectors, evidence production, discovery, reports,
config resolvers, writes, prompts, watches, UI, tests, and coverage documents.
Online research examined official documentation, pinned open-source producers,
release notes, and public native-format investigations.

The research passes used no agent execution, real-session capture, runtime
benchmark, or new behavior test. Earlier implementation work has separate focused
tests; research findings still need regressions. Upstream source establishes
behavior at a commit, not every historical release. The earlier session's final
summary claimed full Cargo validation, but its retained progress record did not
establish the final command status. Rerun relevant checks before claiming the
current worktree validated.

| Evidence class             | Permitted conclusion                                                                |
| -------------------------- | ----------------------------------------------------------------------------------- |
| Implemented baseline       | Existing reader/editor behavior, subject to the recorded limitations                |
| Pinned producer source     | Candidate semantics at one immutable revision                                       |
| Official rolling docs      | Documented control; minimum version and exact loader behavior may remain unresolved |
| Release note               | Feature or fix exists in a named release; not a complete persistence schema         |
| Community decoder or issue | Research lead requiring independent characterization                                |
| Runtime SDK type           | Runtime contract only; not proof of native on-disk persistence                      |

### Research anchors

| Agent           | Anchor                                                             | Release caveat                                                                  |
| --------------- | ------------------------------------------------------------------ | ------------------------------------------------------------------------------- |
| OpenCode        | `193de13a88d62a6409c6d385831180f1def527dc`, dev, 2026-09-10        | Latest release found was `v1.18.30`; do not assume all dev behavior ships there |
| Pi              | `d12cd92e45e308d4af000554292165ef1984253b`, main, 2026-09-10       | Latest release found was `v0.85.1`; per-model compaction was unreleased         |
| Codex           | `fc948f8c473e5d11e780ffcf1fd7f812a2020932`, 2026-09-11             | Gate features by release containment or accepted producer shape                 |
| Claude Code     | Official docs accessed 2026-09-11                                  | Version gates are recorded below; no immutable docs revision was established    |
| Cursor          | Official docs and dated CLI changelog; public readers pinned below | Private storage needs separate CLI/IDE contracts                                |
| Antigravity CLI | Official release notes through `1.1.28`, 2026-09-09                | CLI evidence does not establish desktop 2.11.0 support                          |
| Antigravity SDK | `52ea99480960ed02be1561f6fe57b99e7186962a`                         | Package/runtime configuration is not persisted session evidence                 |

Existing baseline pins remain valid historical boundaries. In particular, this
research does not reverse the dated no-proof decisions for accepted OpenCode,
Pi, or installed Antigravity 2.0 2.11.0 sources. New shapes need new acceptance.

## 4. Current Coverage

### Detection

`A` means accepted sources can yield findings and clean results when session
evidence is complete. `S` means scoped observed-resource findings, never
session-wide clean. `L` means finding-only. `No` means no implemented finding
path for the reviewed agent sources, whether unsupported or uncharacterized.

| Check                    | OpenCode       | Pi                  | Codex     | Claude Code | Cursor | Antigravity |
| ------------------------ | -------------- | ------------------- | --------- | ----------- | ------ | ----------- |
| D: Session overdepth     | A              | A                   | A         | A           | No     | L           |
| T: Model overthinking    | No             | A, selected policy  | A         | A           | No     | No          |
| S: Overpowered subagents | A              | L, reviewed example | A         | A           | No     | No          |
| M: Unused MCP servers    | No             | No                  | S         | S           | No     | No          |
| B: Unused built-in tools | No             | No                  | S         | S           | No     | No          |
| K: Unused skills         | No unused path | No                  | S, narrow | S           | No     | No          |
| O: Old model usage       | A              | A                   | A         | A           | L      | L           |
| F: Fast mode overuse     | No             | No                  | A         | A           | No     | No          |
| C: Cache churn           | A              | A                   | A         | A           | No     | No          |

OpenCode selected-skill evidence is partial but marks a complete selected document
as both injected and invoked. It does not produce an unused-skill finding.
Codex selected documents have a similar constraint; inherited exposure provides
a narrower possible unused path. Pi T describes agent-selected policy, not final
provider effort. Pi S covers the reviewed official example extension only.

Cursor partial matrix cells for tools and workers must not be advertised as
implemented findings. Antigravity compatibility, brain, cascade, and SQLite
profiles remain source-limited. Unknown workspace formats do not inherit them.

### Actions and verification

`A` means current automatic config edit and prompt support. `P` means exact-target
prompt template support only. A template still needs an eligible finding.

| Check | OpenCode         | Pi  | Codex | Claude Code | Cursor | Antigravity |
| ----- | ---------------- | --- | ----- | ----------- | ------ | ----------- |
| D     | P                | P   | P     | P           | No     | P           |
| T     | No               | A   | A     | A           | No     | No          |
| S     | P                | P   | P     | P           | No     | No          |
| M     | No               | No  | P     | P           | No     | No          |
| B     | No               | No  | P     | P           | No     | No          |
| K     | P, template only | No  | P     | P           | No     | No          |
| O     | A                | A   | A     | A           | No     | P           |
| F     | No               | No  | P     | P           | No     | No          |
| C     | P                | P   | P     | P           | No     | No          |

Current automatic writes require an existing supported setting and file, accepted
attribution, and native macOS/Linux. Windows is attribution-only. WSL writes are
unavailable. A generic check-level prompt fallback is not exact-agent parity.

Positive verification currently supports T for Claude/Codex/Pi, O for
Claude/Codex/OpenCode/Pi, and F for Claude/Codex. D/S/M/B/K/C do not have positive
remediation verification. Confirmed monetary contributions currently center on
attributed old-model replacement activity.

## 5. Correctness and Safety Backlog

Resolve these before broadening automatic writes or clean-result eligibility.
Each item needs a focused failing test, the smallest correct fix, and relevant
revision invalidation. Line numbers can move; the linked files identify owners.

Section 22 narrows the write-safety exit to best-effort conflict protection and
backups, and retains existing cache thresholds/sample eligibility. The original
static findings below still explain the risks; universal race freedom and cache
calibration are no longer prerequisites for config-editing delivery.

| Priority | Static-review finding                                                                  | Required change and regression                                                                                                                |
| -------- | -------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------- |
| P0       | Incomplete JSONL tails can be consumed before a resume snapshot                        | Snapshot at a complete boundary with matching state, or decline resume; complete a line by appending only its newline                         |
| P0       | Saved tail-window validation does not prove unchanged parsed prefix                    | Define supported append semantics and verify prefix identity; rewrite a middle record outside the saved window, then append                   |
| P0       | Config checks and replacement have path/content/metadata race intervals                | Pin validated descriptors where supported; reject changed identity, bytes, mode, owner, or trust; deterministic races at every write boundary |
| P0       | Apply reuses cached repository trust                                                   | Re-resolve enabled and accessible scope before prepare, apply, and recovery; disable a repository while review is open                        |
| P0       | Recovery can permanently block a target whose original value remains                   | Distinguish expected original, replacement, and conflicting state; provide explicit bounded recheck/resolution                                |
| P1       | Claude early-return resolution can miss environment controls in other applicable files | Resolve all accepted layers before selecting the effective leaf; test local model plus global environment override                            |
| P1       | Matching observed/current values is described too strongly as physical attribution     | Record matching current default separately from proven launch/control provenance                                                              |
| P1       | Old-model verification omits exact source-format matching                              | Filter exact source and physical target; prevent two attempts from owning the same replacement activity                                       |
| P1       | Report estimates contain hidden assumptions, minimums, and incomplete denominators     | Share qualifying evidence with detectors; unknown remains unknown; preserve known zero/negative price differences                             |
| P1       | Tool catalog falls back to older or unrelated model definitions                        | Separate approximate display estimates from detector-grade exposure; exact entries or reviewed compatibility ranges only                      |
| P1       | Premium classification uses family/name heuristics for unreviewed models               | Require reviewed tier identity; unknown family members cannot produce clean                                                                   |
| P1       | Mixed cache accounting evaluates only one family                                       | Stream independent compatible segments/accounting families and evaluate each                                                                  |
| P1       | Cache ratio excludes initial paid baseline and has no sample floor                     | Review metric definition and calibrate thresholds; test two-request and low-volume cases                                                      |
| P1       | Explicit OpenCode API aliases can reduce cache coverage                                | Characterize valid explicit/omitted route equivalence without broad fallback                                                                  |
| P1       | Old-model cause counts include pre-replacement turns                                   | Count only turns meeting the dated lifecycle boundary                                                                                         |
| P1       | Cursor synthesis deduplicates equal text from distinct observations                    | Use native identity; keep equal text with different IDs, times, and models                                                                    |
| P1       | Unverifiable prompt watches consume active capacity                                    | Store terminal action history separately from useful verification work                                                                        |
| P1       | Batch prompts overflow and can persist partial watches                                 | Shared bounded template, explicit selection/omission counts, transactional enrollment                                                         |
| P2       | Shared action cache can evict visible handles                                          | Reuse stable target entries or partition capacity; expose expiry and refresh                                                                  |
| P2       | UI omits unsupported/truncated targets and review side effects                         | Show action coverage, limits, activation timing, and material effects                                                                         |
| P2       | Copy analytics can count repeated success for one preparation                          | Deduplicate per prepared text; measure review-visible after stale-result rejection                                                            |
| P2       | Local path display contradicts privacy documentation                                   | Decide local-only reviewed path display versus semantic labels; keep paths out of telemetry                                                   |

Primary implementation owners:

- [Framing](../../crates/antiburn-local/src/analysis/framing.rs),
  [source validity](../../crates/antiburn-local/src/analysis/source_validity.rs),
  and [resume parity tests](../../crates/antiburn-local/tests/resume_parity.rs).
- [Evidence sink](../../crates/antiburn-local/src/analysis/evidence_sink.rs),
  [evidence query](../../crates/antiburn-local/src/analysis/evidence_query.rs),
  [tool catalog](../../crates/antiburn-local/src/analysis/tool_catalog.rs),
  [detectors](../../crates/antiburn-local/src/insights/detectors), and
  [report estimation](../../crates/antiburn-local/src/insights/report.rs).
- [Config filesystem](../../apps/desktop/src-tauri/src/agent_config/filesystem.rs),
  [editor](../../apps/desktop/src-tauri/src/agent_config/editor.rs),
  [controller](../../apps/desktop/src-tauri/src/remediation/mod.rs),
  [recovery](../../apps/desktop/src-tauri/src/remediation/recovery.rs), and
  [watch store](../../apps/desktop/src-tauri/src/store/remediation.rs).

## 6. Shared Check Semantics

Do not change the meaning of a check merely to obtain another supported cell.
Any policy change needs a separate decision, revision, and characterization set.

| Check | Existing policy or evidence                                                   | Required design improvement                                                                                                               |
| ----- | ----------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------- |
| D     | Maximum observed input context above 400,000 tokens                           | Review model window and main/worker scope; a current gauge, step count, or text estimate is not request context                           |
| T     | Reviewed above-cap effort, including xhigh/max/ultra where applicable         | Say above the reviewed cap, not more reasoning than the task needed; distinguish selected policy from native/final effort                 |
| S     | Premium actual parent and actual worker joined through native delegation      | Use exact reviewed models; do not substitute worker count, nesting, concurrency, requested aliases, or configured inheritance             |
| M     | Exposed server uninvoked over complete observed calls and eligible activity   | Require exact ownership; account for non-tool instructions/resources and short-session uncertainty                                        |
| B     | Positive-cost exposed definition, unused, not deferred or situational         | Require actual schema exposure/removal semantics; permission denial alone is not universal proof                                          |
| K     | Full injected document, not merely a listing, unused under accepted semantics | Separate selected, preloaded, inherited, reattached, truncated, and listed content; instructions can affect behavior without another call |
| O     | Timed actual use after a reviewed replacement becomes available               | Check compatibility, account availability, route, quality purpose, and dated model policy; newer is not inherently cheaper                |
| F     | Explicit fast-tier delegated activity                                         | Preserve premium service-tier meaning; one request does not prove latency was unnecessary                                                 |
| C     | Compatible ordered request accounting above reviewed ratios                   | Segment routes, models, compactions, and accounting; retain thresholds and label heuristic/sample limits under Section 22                 |

For M/B/K, investigate a minimum eligible interval and material exposure threshold.
Do not silently require many sessions to prove a positive observation, but do not
recommend global removal from one short task without explaining uncertainty.
Preserve situational tools and required dependencies.

For D/T/S/F, show policy observations with scope and user-selectable tradeoffs.
Lower context, effort, model capability, or speed can increase retries and lost
time. A config action must not promise improved task quality or guaranteed cost.

For C, assess cache-write accounting and uncached-input accounting independently.
Review the first-request baseline explicitly. No universal TTL, cache key, or
compaction change is an acceptable fallback for unknown churn cause.

## 7. Configuration Architecture

### Keep the existing boundaries

The engine owns evidence, check policy, typed recommendation inputs, prompt
construction, verification, and estimates. The shell owns current config
resolution, physical targets, writes, persistence, IPC, UI, and analytics.

Extend `VendorConfig` and `VendorRemediationPolicy`; do not add a second writer
or a generic executable configuration engine. Use small vendor-specific typed
operations with shared lifecycle and filesystem code.

### Separate identities and outcomes

Retain the existing finding, attempt, action-handle, and prepared-operation
identities. Add a durable control identity only where a concrete operation needs
one: agent, product surface, environment, scope, physical file, selector, and
versioned resolver contract. A worker definition is not a historical worker call.

Store these concepts independently:

- Historical observed value and evidence provenance.
- Current configured value and its winning source.
- Prior effective behavior when a setting is absent or inherited.
- Proposed value, scope, and expected activation boundary.
- Config write outcome and current typed readback.
- Positive later behavior and savings eligibility, if supported.

Preventive advice must not create a fabricated historical finding. A matching
current default can justify a reviewed future-policy change without claiming it
caused the earlier request.

### Operation types

Start with only types needed by scheduled controls:

- Replace an existing scalar.
- Add a missing scalar or nested leaf.
- Add/remove one exact list entry while preserving order and unrelated entries.
- Edit one existing named-agent frontmatter field.
- Create one minimal documented config file.

Defer arbitrary source-code rewrites, config migrations, whole-worker creation,
and multi-file transactions until a specific operation requires them. SDK Python
configuration is prompt-assisted source work, not a universal JSON editor.

Each operation contract must define:

| Contract field | Required content                                                                          |
| -------------- | ----------------------------------------------------------------------------------------- |
| Applicability  | Agent, product, source shape, config syntax, version evidence, model/provider constraints |
| Resolution     | Paths, precedence, trust, inheritance, managed sources, runtime override exclusions       |
| Selector       | Exact key, array item, canonical skill document, or named-worker field                    |
| Mutation       | Existing-key replacement, insertion, list edit, or exclusive file creation                |
| Validation     | Syntactic validity plus the post-edit effective value under all accepted layers           |
| Effects        | Scope, reload/restart, inheritance changes, capability loss, cache effects, cost tradeoff |
| Recovery       | Expected original, proposed state, conflict, interrupted creation/replacement             |
| Verification   | File readback, positive behavior if available, and explicitly unavailable savings         |

### Missing-setting and file creation

Creation is approved for planning, not unrestricted write authorization.

1. Ask the user to confirm the target scope and show whether it is shared.
2. Resolve existing files and higher-priority sources first.
3. Treat missing as a distinct expected state; a concurrent file/key creation
   conflicts rather than being overwritten.
4. Create the smallest valid leaf. Do not serialize the merged effective config
   into a new file and freeze unrelated inherited defaults.
5. Do not copy MCP credentials, commands, remote config, or plugin definitions
   just to create a disable override.
6. Allow partial MCP overlays only where the pinned loader proves their semantics.
7. Validate newly introduced directories and trust requirements before review.
8. Never create trust records or weaken managed policy to activate the setting.
9. For shared project config, explain teammate impact. Do not silently edit global
   Git excludes; separately review any needed local ignore rule.
10. Resolve the complete post-edit configuration again before declaring success.

Keep the current 256 KiB config bound unless measured requirements justify a
reviewed increase. Preserve JSONC comments, TOML layout, unknown fields, and
Markdown bodies. Reject duplicate semantic definitions and malformed documents.

### Filesystem and recovery requirements

The maintainer chose best-effort safety on 2026-09-11, not guaranteed exclusion of
external writers or guaranteed power-loss durability. Do not require proof that
the agent stopped. Show a non-blocking apply notice recommending that the user
close the editor. Prefer no-follow descriptor operations and same-directory
atomic replacement where supported. Keep these as practical protections, not a
requirement to solve every platform race before delivering config editing.

Validate the current target, bytes, permissions, and trusted scope before writing.
Serialize Antiburn writes and recovery for the same physical file. External
writers can still win a race or overwrite the result later; do not claim otherwise.
Do not use a failed atomic operation as a reason to silently truncate the live
config. Prefer a clear retryable error over a known corruption risk.

Before replacing an existing config, refresh one local `<config>.backup` companion
with the complete pre-edit contents. Keep only the latest backup, as approved by
the maintainer. Use no broader access than the original; never follow a backup
symlink or overwrite an unsafe target. A backup failure prevents the config write.
New-file creation has no original backup. See Section 22 for ordering and recovery.

Use exclusive temporary files, best-effort supported file/directory sync, typed
readback, and persisted uncertain-write state. Missing-file publication must not
overwrite a file that appeared after review. Recheck changed permissions rather
than restoring stale metadata over them. Do not promise power-loss durability
from a successful process-crash test or from the existence of a backup.

Recovery must distinguish exact original state, exact replacement state, missing
created target, changed target, and unknown effective precedence. An original
value after a crash before replacement should release the reservation safely.
Never overwrite a third-party change during recovery or rollback.

Native Windows requires ACL, reparse-point, sharing, identity, replacement, and
crash-recovery tests before write support. WSL requires a separate Linux-side
path/environment contract; never edit native host config for WSL evidence.

## 8. OpenCode Plan

Sources: [config][oc-config], [agents][oc-agents], [skills][oc-skills],
[pinned loader][oc-loader], [paths][oc-paths], [request preparation][oc-request],
and [provider translation][oc-transform].

### Controls by check

| Check | Candidate action                                                                                              | Gate or limit                                                                                             |
| ----- | ------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------- |
| D     | `compaction.auto`, `compaction.reserved`, `compaction.prune`; later `tail_turns` and `preserve_recent_tokens` | Compaction is not a hard request-depth ceiling; characterize first released versions of newer fields      |
| T     | Route-specific model `options`, agent options, or selected variant options                                    | Preventive only under current evidence; no universal reasoning field                                      |
| S     | Existing `agent.<name>.model` and reviewed `variant`, or equivalent Markdown frontmatter                      | Bind the winning worker definition and actual delegation; `small_model` is not a subagent default         |
| M     | `mcp.<server>.enabled = false`; scoped tool exclusion where proved                                            | Historical exposure remains unavailable; enablement-only overlay is supported by the pinned V1 schema     |
| B     | Exact whole-tool `permission` denial that removes its schema                                                  | Later granular exceptions and grouped permissions change the result                                       |
| K     | Exact skill-name permission; worker-scoped skill exclusion                                                    | Removes future availability, not an already selected document; current unused finding path remains absent |
| O     | Top-level, agent, or command model selector                                                                   | Resolve the correct selector and provider route; preserve runtime/resume distinctions                     |
| F     | Reviewed OpenAI `serviceTier` option where supported                                                          | Preventive only; fast-named variants are not evidence of paid priority                                    |
| C     | Diagnose `setCacheKey`, supported `cacheControl`, and prefix changes                                          | Automatic session-derived keys often already exist; do not create a static shared key                     |

Provider controls such as `reasoningEffort`, `thinking`, `effort`, and
`thinkingConfig` are model/API-specific alternatives. Translate only reviewed
routes. Do not copy one provider's option into another provider's config.

### Resolution requirements

The pinned loader includes more than the simplified docs order: remote defaults,
global legacy `config.json`, JSON/JSONC, custom config, project ancestry,
configuration directories and Markdown agents, inline config, active account
organization config, managed files, MDM, and post-merge environment overrides.
Instructions and plugins have special merge/deduplication rules.

Request options resolve through model-derived defaults, model options, agent
options, selected variant, plugin mutation, and provider translation. A selected
variant can defeat an agent-level effort edit. Explicit request model/variant
can defeat an agent default. Reject unknown plugin and remote control rather
than execute it to resolve the value.

Top-level unknown agent fields can normalize into `options`; conflicting
representations are semantic duplicates. Legacy `mode` conversion and Markdown
collisions need fixtures. Environment substitutions and file substitutions remain
dynamic unless the exact bounded resolver contract can prove them without code
execution or secret exposure.

`OPENCODE_PERMISSION`, `OPENCODE_DISABLE_AUTOCOMPACT`, and
`OPENCODE_DISABLE_PRUNE` can affect the final result. Inspecting Antiburn's
environment is not proof of the historical agent's environment.

### Real schema removal

The pinned permission implementation excludes a tool definition only when the
last applicable rule has pattern `*` and action `deny`. An `ask`, command/path
deny, or later exception is not equivalent. `edit` groups edit/write/apply_patch;
some MCP resource operations share `read`. Test actual request construction with
synthetic tools before pricing removed definitions. Prefer permissions over
deprecated `tools` writes for versions where that mapping is reviewed.

### CoreV2 source work

Add a separate accepted source contract for `session_message`; do not silently
expand `OpenCodeSqliteV2`. Research [SQL storage][oc-sql], [message types][oc-message],
[context epochs][oc-epochs], and [skill guidance][oc-guidance].

Use session/sequence ordering, explicit type, model/time/tokens, compactions, and
complete tool results. Handle mutable projections, duplicate sequences, unknown
types, forks, and legacy/CoreV2 coexistence without double counting.

`session_context_epoch` is mutable current state, not a full historical archive.
Skill guidance snapshots contain listings, not full skill documents. Neither
proves historical M/B/K exposure. Runtime tools and response events do not prove
that their complete controls are saved.

Initial target: D/O where request facts are proved, then C with reviewed linkage
and accounting, and selected-skill facts without an unused claim. T/M/B/F remain
unavailable unless a new persisted proof is found. Config CoreV2 `request.body`,
`permissions`, `disabled`, and variant arrays require their own editor contract;
they are not V1 `options` and variant maps.

## 9. Pi Plan

Sources: [settings][pi-settings], [settings manager][pi-settings-source],
[trust][pi-trust], [startup][pi-sdk], [changelog][pi-changelog], and
[official worker example][pi-workers].

### Controls by check

| Check | Candidate action                                                                     | Gate or limit                                                                                                   |
| ----- | ------------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------- |
| D     | `compaction.enabled`, `reserveTokens`, `keepRecentTokens`; per-model overrides later | Increasing reserve compacts earlier; do not use zero reserve; per-model fields were unreleased at research time |
| T     | Exact `modelThinkingLevels[provider/modelId]`, otherwise `defaultThinkingLevel`      | Recursive merge precedes model lookup; saved policy is not final provider effort                                |
| S     | Existing reviewed example agent Markdown `model`                                     | No universal Pi worker config; user/project scope and extension revision must be known                          |
| M     | Extension-specific guidance only                                                     | No core MCP config contract; do not create `mcpServers`                                                         |
| B     | `defaultTools`                                                                       | Version-gated preventive tool selection; extensions/custom tools remain enabled                                 |
| K     | Exact `skills` exclusions or package resource filter                                 | Availability change only; `enableSkillCommands` does not disable all loading                                    |
| O     | Paired `defaultProvider` and `defaultModel`                                          | Scoped models, CLI, SDK, and resumed state can override defaults                                                |
| F     | SDK/provider-specific `serviceTier` or supported request parameter                   | No core settings fast control; source editing is prompt-assisted, not a generic config action                   |
| C     | Supported `PI_CACHE_RETENTION` or explicit SDK cache retention review                | No established `settings.json.cacheRetention`; display notices are not a cache policy                           |

### Merge, trust, and creation

Global settings live under the effective agent directory. Project settings are
`<cwd>/.pi/settings.json`, not arbitrary ancestor settings files. Nested objects
merge recursively; arrays replace. A global per-model thinking or compaction
entry can beat a project-wide fallback after merge. Resolve each leaf, not the
first file containing any candidate key.

Current documentation names `PI_CODING_AGENT_DIR` and
`PI_CODING_AGENT_SESSION_DIR`; the baseline includes `PI_AGENT_DIR`. Preserve
historical accepted behavior and add a versioned discovery test, not a global
rename based on current docs.

Trust can come from a run override, extension, nearest saved ancestor decision,
global fallback, or interactive decision. Temporary approval is not durable
trust. Noninteractive modes may ignore project resources. Creating the first
project settings file can introduce a trust requirement that did not exist.
Do not write `trust.json` or `defaultProjectTrust` to activate a burn fix.

SDK settings and `applyOverrides()` differ from CLI trust. Resume restores model
and thinking state independently from edited defaults. `/model` and `/thinking`
changes are session-scoped unless explicitly saved in current versions.

### Version gates

| Feature                                 | Research gate                                                                                                                                        |
| --------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------- |
| Project trust                           | v0.79.0; fallback setting in v0.79.1                                                                                                                 |
| Recursive merge                         | `v0.84.0`; introducing commit `97f0ccdd96cc207b6ad3630c56eea4d32dbdcf53` is contained in this tag, not `v0.83.0`                                     |
| `defaultTools` preserving custom tools  | v0.84.2, including the preservation fix                                                                                                              |
| Session-scoped model/thinking selection | v0.84.3 behavior                                                                                                                                     |
| Producer-native thinking metadata       | `v0.85.0` contains introducing commit `4e69b0c28060f0f02fbe38bfa7c21a2e2eb25057`; `v0.84.3` and `v0.84.4` do not; presence remains provider-specific |
| New OpenAI cache TTL translation        | v0.85.1, model/API-specific                                                                                                                          |
| Per-model compaction                    | `46bde88a1cd752966aa2a357d292e83aff98b132`, unreleased on 2026-09-11                                                                                 |

### Evidence additions

Investigate persisted `responseModel`, `responseId`, `providerThinkingLevel`,
`usage.cacheWrite1h`, diagnostics, and compaction/branch-summary usage.

Keep requested and response model separate, especially router `auto` requests.
`providerThinkingLevel` is assigned before a payload callback can mutate the
request in reviewed code; label it producer-reported rather than final-wire proof.
Preserve existing `AgentSelectedPolicy` semantics until a separate policy is
accepted. Missing metadata stays missing.

`cacheWrite1h` is a subset of cache writes; reasoning is a subset of output.
Do not add either twice. Summary requests use different routing/cache behavior
and must not become compatible main-loop pairs. Diagnostics can explain a
transformation without proving the sole churn cause.

Native selected-skill wrappers can improve full-document injection facts, but
selection itself is invocation. Reject empty, truncated, user-authored lookalike,
or extension-mutated wrappers unless their provenance is proved. Do not promote
Pi K solely because a complete selected document can be parsed.

The official worker example runs children without session persistence and stores
nested results in the parent. Its default scope is user; project overrides only
apply when the call admits them. Explicit worker model selection changes parent
thinking inheritance. Bind both scope and version before editing a worker file.

## 10. Codex Plan

Sources: [config reference][cx-config], [advanced config][cx-advanced],
[loader][cx-loader], [skills][cx-skills], [worker docs][cx-workers], and
[pinned role loader][cx-role].

### Controls by check

| Check | Candidate action                                                                 | Gate or limit                                                                             |
| ----- | -------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------- |
| D     | `model_auto_compact_token_limit` and reviewed limit scope                        | `total` differs from `body_after_prefix`; do not falsify `model_context_window`           |
| T     | `model_reasoning_effort`, Plan-mode effort, worker/default effort                | Bind main, Plan, or worker control; unknown custom effort strings are not rankable        |
| S     | `agents.default_subagent_model` or exact role-file `model`                       | Explicit spawn, defaults, parent inheritance, and role projection have distinct ordering  |
| M     | Exact server `enabled = false`, tool filters, plugin/app selectors               | Preserve complete merged definitions; role-local MCP controls are not proved effective    |
| B     | Reviewed `features.shell_tool`, `web_search`, agent/multi-agent feature controls | No universal top-level tool deny list; implementation selection is not schema removal     |
| K     | Ordered `skills.config` disable rule for exact document path                     | Pinned source uses SKILL.md path and User/SessionFlags layers, not ordinary project rules |
| O     | Main `model`, review model, worker selector as actually responsible              | Preserve provider/auth/catalog provenance and runtime thread choices                      |
| F     | `service_tier = "default"` on a supporting version                               | Explicit standard sentinel; deleting a key may reveal inherited fast tier                 |
| C     | Cause-specific guidance                                                          | No reviewed general config TTL/cache-key setting                                          |

### Profiles and precedence

Current profiles are `$CODEX_HOME/<name>.config.toml`, selected by `--profile`.
This changed in 0.134.0. Legacy `[profiles.<name>]` and top-level `profile` need a
different historical contract. Do not migrate them automatically.

Resolve CLI/session flags, trusted project layers nearest CWD, selected profile,
user, cloud/system defaults, and separate requirements. Project-root markers and
`--cd` affect discovery. Project config cannot redirect provider/auth routing or
select profiles. Embedding-host and unresolved managed sources block automatic
attribution.

Tables generally merge; arrays generally replace. Skills use a separate ordered
resolver. Relative paths anchor at their declaring file. Prove these rules with
fixtures, not one generic merge routine used for every setting.

Promise next-startup application for direct TOML edits. Runtime reload protocols
are not permission to call the running agent. Existing thread selections can
remain independent from new defaults.

### Worker-specific limits

Default worker model/effort resolution and role-file overrides differ. A role
changing only model may preserve an incompatible earlier effort and fail spawn
validation when model metadata is known. Review them
together when needed, without silently broadening the edit.

The pinned role loader projects a bounded set of fields. Current docs describe
broader role configuration than the source applies. Do not offer role-local MCP,
sandbox, or compaction edits until their actual loader path is proved. Root
service tier is applied after role config in the reviewed spawn path, so a
worker-local standard tier is not a reliable F fix.

Prefer editing an existing role. A newly created standalone role requires name,
description, and instructions; a model-only file is not a safe replacement for an
existing worker. If a declaration plus config file is eventually needed, plan a
recoverable two-file operation separately.

### Skill rule conflict resolved by source

The rolling config reference describes a skill folder path, while the skill guide
uses `SKILL.md`. The pinned [rule resolver][cx-skill-rules] and
[host matching][cx-skill-host] use the canonical document path. Adopt that exact
path only for the characterized producer; preserve the discrepancy as a test.

Rules are read from User layers and SessionFlags. Selected profile files count
as User layers. Ordinary project layers do not establish this control. Later
matching name/path rules can re-enable an earlier disabled skill. Source supports
either name or path, not both; prefer precise path rules initially.

`allow_implicit_invocation: false` is a different policy from disabling a skill.
Catalog token limits do not limit the full selected document. Restart is required
for config enablement changes according to the skill guide.

### Passive source improvements

Review [rollout policy][cx-policy], [protocol][cx-protocol],
[world state][cx-world], [deferred tools][cx-tools], and
[selected-skill fragments][cx-fragments].

| Persisted shape                 | Candidate use                                             | Limit                                                                |
| ------------------------------- | --------------------------------------------------------- | -------------------------------------------------------------------- |
| `session_meta`                  | Harness, CWD, provider, role/fork identity, dynamic tools | Dynamic tools are not every built-in or MCP definition               |
| `turn_context`                  | Model, effort, collaboration mode, root/turn identity     | Not necessarily emitted for every model request                      |
| `thread_settings_applied`       | Position-sensitive selected model/effort/tier             | Applied preference is not proof a request consumed it                |
| `token_usage_record`            | Response identity and request/aggregate usage             | Join exact model/tier/route; avoid aggregate double counting         |
| `configuration_update`          | Reasoning-control transitions                             | Requires ownership and ordering                                      |
| `tool_search_output`            | Exact observed server/definition subset                   | Not a full inventory                                                 |
| `world_state` snapshots/patches | Availability and context transitions                      | Apply bounded RFC 7386 semantics; each section has different meaning |
| Selected-skill fragments        | Full document and optional resource authority             | Remote/opaque authority is not a local editable path                 |
| Paginated completed items       | Typed tool and delegation facts                           | Separate from legacy event persistence                               |
| Compacted/window records        | Replacement history and usage boundaries                  | Replacement content is not fresh usage                               |

Both the baseline and current researched rollout policy omit `AdditionalTools`.
Current policy also omits several startup/reroute events. Deferred namespace
snapshots contain names and shortened descriptions, and rendering can omit entries
to fit a smaller announcement budget. Do not price every stored namespace as an
injected full definition or claim complete historical resource exposure.

## 11. Claude Code Plan

Sources: [settings][cc-settings], [reference][cc-reference], [models][cc-models],
[subagents][cc-workers], [MCP][cc-mcp], [skills][cc-skills],
[fast mode][cc-fast], and [prompt caching][cc-cache].

### Controls by check

| Check | Candidate action                                                   | Gate or limit                                                                                       |
| ----- | ------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------- |
| D     | `autoCompactEnabled`, `autoCompactWindow`                          | Review disable/window environment overrides; future policy, not a hard historical fix               |
| T     | Per-model or top-level `effortLevel`; separate optional effort cap | Resolve same-source precedence, model-default holds, supported levels, and environment              |
| S     | Exact existing worker `model` and possibly `effort`                | Preserve prompt, tools, skills, isolation, and per-invocation override limits                       |
| M     | Source-specific disable selector                                   | `.mcp.json` rejection, default-on opt-out, default-off opt-in, plugin and connector controls differ |
| B     | Exact whole-tool deny or worker tool restriction                   | Whole-tool denial can remove schema; scoped command/path denial does not                            |
| K     | Remove worker preload; exact skill visibility/disable control      | Plugin and bundled skills differ; already injected history remains                                  |
| O     | Effective main/worker/skill model selector                         | Aliases, provider deployments, resume, and default holds require separate treatment                 |
| F     | `fastMode: false` or `fastModePerSessionOptIn: true`               | Show global/session consequences; per-session opt-in does not erase saved preference                |
| C     | `promptCacheTtl` or `subagentPromptCacheTtl` where justified       | Longer writes cost more and do not fix prefix mutation                                              |

### Version-aware settings

Managed, `--settings`, project local, shared project, and user define the ordinary
stack. Dedicated flags and environment values have per-key precedence. Arrays
usually combine. Read all applicable layers before selecting a control.

Shared settings are based on primary CWD. From 2.1.211, project-local settings
normally use repository root/main checkout, with Windows, ownership, home-root,
and non-Git exceptions. Legacy CWD local files can coexist. From 2.1.246, `/cd`
changes project settings. Agent and skill discovery use different rules.
`CLAUDE_CONFIG_DIR` relocates the home. Do not reuse one path resolver everywhere.

| Feature                           | Documented gate or special rule                                                                                 |
| --------------------------------- | --------------------------------------------------------------------------------------------------------------- |
| Auto-compact launch flag          | 2.1.221+; window JSON is numeric, environment is plain integer tokens                                           |
| `modelSettings`                   | 2.1.251+; highest source defining either general or model effort wins, then model entry wins within that source |
| Worker environment precedence     | Changed in 2.1.251; force override added in 2.1.257                                                             |
| Effort caps                       | 2.1.267+; lowest applicable cap, unlike ordinary default precedence                                             |
| Existing nested worker precedence | 2.1.178+; duplicate names within one scanned tree can remain ambiguous                                          |
| Built-in Explore model            | Changed in 2.1.198; do not assume it always uses Haiku                                                          |
| Managed-provided MCP definitions  | 2.1.259+                                                                                                        |
| Prompt TTL settings               | 2.1.242+                                                                                                        |
| Worker cache TTL frontmatter      | 2.1.248+; bucket overrides can win                                                                              |

Minimum versions not established by research remain unresolved gates. Do not use
the current docs' model examples as replacement policy or entitlement proof.

Direct edits to model/effort defaults do not switch active requests. Some model
defaults can remain held above saved effort until an explicit choice. Current
docs exclude `max` from saved `effortLevel` and `modelSettings.effortLevel`.
`ultracode` also changes
orchestration; it is not just an effort label. Caps are separate restrictions,
not silent substitutes for changing a default.

### MCP ownership and exact operations

| Intent                                              | Control                                        | Safety condition                                                                                  |
| --------------------------------------------------- | ---------------------------------------------- | ------------------------------------------------------------------------------------------------- |
| Reject a project `.mcp.json` server                 | `disabledMcpjsonServers`                       | Exact source/name; preserve merged rejection lists                                                |
| Disable a regular default-on server for one project | Project `disabledMcpServers` in `.claude.json` | Separate agent-owned-state editor needed; concurrent writes and credentials make this higher risk |
| Disable a default-off built-in                      | Remove exact project `enabledMcpServers` entry | Do not add it to the unrelated opt-out list                                                       |
| Apply an explicit policy deny                       | `deniedMcpServers`                             | Review broader policy scope and supported selector                                                |
| Disable fetched connectors                          | `disableClaudeAiConnectors`                    | Broad action; not one-server remediation                                                          |
| Disable a plugin                                    | `enabledPlugins` entry                         | Affects all plugin components; not a narrow server fix                                            |

Do not invent `enabled: false` inside Claude MCP definitions. Do not copy or edit
credentials. Managed, CLI, SDK, plugin, connector, and duplicate-endpoint origins
must be resolved. Tool search and `alwaysLoad` affect upfront exposure; connected
does not mean all schemas were injected. Raw MCP edits generally need restart.

### Tools and skills

Official caching docs state whole-tool denial removes the definition. A bare
`WebFetch` deny differs from `Bash(rm *)`. `allowedTools` and skill `allowed-tools`
preapprove execution rather than define an availability allowlist. Test request
schema removal on the exact supported harness/model before offering B savings.

For K, first prefer removing an exact optional skill from a named worker's
preload list. Alternatives include `skillOverrides` visibility and
`disable-model-invocation`. `user-invocable: false` does not disable model use.
`name-only` changes listing overhead, not full-document overhead. Plugin skills
are not governed by the same `skillOverrides` contract.

Skill precedence differs from agent precedence. Reinvocation can use a short
already-loaded note, and compaction reattachment can truncate documents. Track
full-document identity and lifecycle without retaining private bodies. Do not
count reattachment as fresh full injection automatically.

### Cache actions

The reviewed docs distinguish main and worker/helper buckets. Resolve force-5m
environment, bucket environment, bucket JSON, applicable worker frontmatter,
global one-hour enablement, and defaults in the documented order.

Offer one hour only when observed reuse gaps, route support, and reviewed rates
justify higher write cost. Model/effort switches, first fast activation, tool
changes, CWD, compaction, and gateway behavior can explain churn independently.
MCP discovery caching is not prompt caching. Do not remove a disable environment
entry and claim it unsets the variable in an already running process.

## 12. Cursor Plan

Keep CLI, IDE composer, CLI store, native transcript, and legacy compatibility
surfaces separate. Sources: [CLI config][cu-config], [changelog][cu-changelog],
[workers][cu-workers], [skills][cu-skills], [MCP][cu-mcp], and
[model governance][cu-governance].

### Controls by check

| Check | Candidate action                                                              | Gate or limit                                                             |
| ----- | ----------------------------------------------------------------------------- | ------------------------------------------------------------------------- |
| D     | Prompt-guided summarize/new conversation/context reduction                    | No verified numeric CLI/IDE threshold config found                        |
| T     | Native model parameter picker or exact supported future-run variant           | No published complete nested model/effort JSON schema                     |
| S     | Existing custom-agent `model`; reviewed model parameter syntax                | Need exact worker definition and actual-model delegation evidence         |
| M     | Native disable action; future narrow persisted selector after schema research | Definition, saved approval/scope selection, plugin, and team state differ |
| B     | Prompt guidance only unless a real schema-exclusion control is proved         | Permissions/Ask/Plan do not establish definition removal                  |
| K     | Exact skill `paths` or `disable-model-invocation` policy                      | Availability/listing change, not proof of unused full document            |
| O     | Add exact-target prompts; native picker for actual available replacement      | Do not rewrite undocumented CLI-managed model object                      |
| F     | Explicit model parameter such as supported worker `fast=false`                | Preserve premium-speed semantics; avoid blind `/fast` toggle              |
| C     | Cause-specific guidance only                                                  | No reviewed persisted cache counters or config TTL/key                    |

Global CLI config is normally `~/.cursor/cli-config.json`; project `cli.json`
supports permissions only. Respect Windows, XDG, and `CURSOR_CONFIG_DIR` variants
only through characterized resolution. `model` is documented as an object but
its complete schema is not published. `maxMode` and
`hasChangedDefaultModel` are not effort/fast booleans to rewrite independently.

Custom agents use Markdown under reviewed project/user roots. Fields include
model, readonly, and background behavior. Current docs show parameter syntax
such as `composer-2.5[fast=false]`; this proves a worker-definition syntax, not
the global CLI model object's schema. Reject unknown variant semantics.

MCP uses `mcpServers` in project/user `mcp.json`, but later CLI versions add
duplicate endpoint consolidation and per-project saved scope selection. An entry
in the nearest project file is not always the active owner. Do not invent a
disable key or remove a server definition until the persisted manager action and
merge semantics are pinned. A copied prompt may guide the user through the native
manager; Antiburn must not run the manager for evidence.

### Passive evidence investigations

Community sources are leads, not native contracts:
[txcript 0.8.0][cu-format], [pinned parser][cu-parser],
[context-field report][cu-gauge], and [vct-core 2.7.1][cu-vct].

| Candidate                                                             | Investigation                                                                   | Do not infer                                                          |
| --------------------------------------------------------------------- | ------------------------------------------------------------------------------- | --------------------------------------------------------------------- |
| Composer `promptTokenBreakdown.totalUsedTokens` / `contextTokensUsed` | Determine producer meaning, freshness, and exact request association            | A current gauge is not observed request input or full-session D clean |
| Bubble `modelInfo` and request metadata                               | Distinguish selected from actual served model, including Auto                   | Current composer model is not every earlier response model            |
| CLI `providerOptions.cursor.modelName`                                | Pin actual-versus-requested semantics and known effort/fast variants            | Thinking text or duration is not effort                               |
| `subagentInfo.parentAgentId` and child stores                         | Join exact parent delegation request to worker execution and both actual models | Counts, nesting, filenames, and parent conversation alone are not S   |
| `agentKv` / `messageRequestContext`                                   | Research exact request-bound tool/resource catalog persistence                  | Temporal proximity or context category totals are not named exposure  |
| Native transcripts and structured tool state                          | Preserve IDs, calls, results, skills, and omissions                             | CLI stdout JSON is not the native transcript format                   |

First improve lossless native O extraction and prompts. D/T/S/F remain research
gated until the exact existing check requirements are met. A separately labeled
context snapshot could be useful, but requires a separate product/policy decision
and must not silently replace D. Context gauges are never cache-read tokens.

## 13. Antigravity Plan

Keep 2.0 desktop, standalone IDE, IDE extensions, CLI, and SDK distinct. Shared
directories and related binaries do not prove identical config or persistence.
Sources: [MCP][ag-mcp], [CLI releases][ag-releases], [settings][ag-settings],
[CLI settings][ag-cli-settings], [workers][ag-workers], and [SDK source][ag-sdk].

### Application controls by check

| Check | Candidate action                                                             | Gate or limit                                                                                |
| ----- | ---------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------- |
| D     | Prompt-guided context reduction/new session                                  | No reviewed general application numeric threshold setting                                    |
| T     | CLI `/effort` or future-run `--effort`; product model picker                 | CLI feature and fix gates; persisted preference schema still needs proof                     |
| S     | Exact existing custom-agent model tier and capability settings               | `inherit`, `flash`, `pro` are configuration tiers, not actual model evidence                 |
| M     | Existing active server `disabled` and exact `disabledTools`                  | Strongest config-edit candidate; prove surface, ownership, and merge                         |
| B     | Existing custom-agent tool allowlist where actual exclusion is proved        | Default agent inventory and inheritance remain incomplete                                    |
| K     | Exact worker preload/dependency changes where documented                     | No universal disable-model-invocation equivalent proved; hiding slash command is not removal |
| O     | Native model selection guidance; future writer after schema characterization | Served model, selected model, and automatic replacement differ                               |
| F     | No application premium-tier action established                               | Planning/Fast modes, Flash names, and animation speed are not paid priority                  |
| C     | Cause-specific guidance                                                      | CLI generation metadata is a research candidate, not a proven cache policy knob              |

### MCP writer candidate

Current docs specify global `~/.gemini/config/mcp_config.json` and workspace
`.agents/mcp_config.json`, with `mcpServers`, `command` or `serverUrl`, and
`disabled` / `disabledTools`. A narrow edit to an existing verified active entry
is plausible. Never rewrite the whole definition or expose headers/OAuth values.

The docs say legacy `url` is unsupported, but release notes record support and
later preservation fixes. Preserve existing `url`, `enabledTools`,
`timeoutSeconds`, `tools.eager`, and unknown keys. Preservation does not establish
their full semantics. Comments/trailing commas changed in later CLI releases.

Do not create `{disabled: true}` as a higher-layer server tombstone until merge
behavior is proved. Shared global edits can affect multiple Antigravity products;
show that scope in review. Missing-file creation requires a complete supported
shape and an unambiguous target, not copying a lower-layer credential-bearing
definition.

### Release and configuration gates

| CLI release | Research significance                                               |
| ----------- | ------------------------------------------------------------------- |
| 1.0.4       | Native SQLite conversation support                                  |
| 1.0.5       | Release note adds remote `url` support despite current docs wording |
| 1.1.0       | Legacy `/fast` execution mode removed; not inference-tier evidence  |
| 1.1.5       | `/effort`, `--effort`, model slugs and worker model tiers           |
| 1.1.10      | Model/effort launch override fixes                                  |
| 1.1.14      | Custom-agent `inheritCustomizations` introduced                     |
| 1.1.16      | MCP disable subcommand and unknown-property preservation fixes      |
| 1.1.21      | Explicit customization path collision behavior changed              |
| 1.1.22      | Persistent `/model <name>` selection                                |
| 1.1.24      | MCP comments/trailing-comma handling                                |
| 1.1.25      | Ambient customization inheritance changed                           |
| 1.1.27      | One-prompt model override and agent dependencies                    |
| 1.1.28      | Model/effort resolution logging and plugin-relative CWD fixes       |

Do not use table entries as blanket producer ranges. Establish tag containment,
actual config parser behavior, and storage shape with fixtures before enabling an
operation. UI labels in rolling docs do not version every page.

CLI preferences use sparse `settings.json`, but a complete model/effort schema is
not established by the public reference. Permission fields are not cost controls.
Project/global/shared scope selection has its own state. Do not create trust,
disable sandboxing, or import Gemini CLI settings into Antigravity.

Worker roots and skill roots differ across products and releases. Customization
inheritance can keep MCP servers, skills, or agents active despite a narrow tool
list. Do not replace a worker with a model-only stub. Managed and plugin assets
remain native-manager actions rather than direct edits.

### Passive source investigations

Use [versioned SQLite format research][ag-format],
[pinned native transcript research][ag-native], and
[generation-decoder review][ag-usage] as secondary leads.

| Candidate                          | Intended evidence                                 | Required proof                                                                  |
| ---------------------------------- | ------------------------------------------------- | ------------------------------------------------------------------------------- |
| CLI native `transcript_full.jsonl` | Calls, selected skills, exact delegation results  | Producer framing, rewrites, truncation, reinjection, and call/result ownership  |
| CLI `conversations/<id>.db` steps  | Ordered native activity and companion joins       | Exact schema, protobuf descriptors, request versus step semantics, WAL snapshot |
| `gen_metadata`                     | Actual response model, response ID, token classes | Field semantics, request order, retries, reset/compaction, provider route       |
| Summary database                   | Discovery and parent candidates                   | Never substitute summary counts for request context or actual models            |
| 1.1.28 CLI logs                    | Selected effort and model-resolution diagnostics  | Exact line grammar and session/request IDs; no time-only association            |
| Existing MCP caches                | Candidate schema/availability evidence            | Freshness, historical ownership, actual request exposure                        |
| Desktop 2.0 native stores          | Separate D/O and possible future facts            | Do not transfer CLI producer semantics to desktop 2.11.0                        |

The promising S investigation joins parent and child generation model evidence
through a native `invoke_subagent` result with exact child identity. No complete
actual-premium-parent plus actual-premium-worker contract was proved in this
research. Worker count or lineage alone does not satisfy S.

Published generation decoding identifies candidate cache-read and response-model
fields, but uncached-input interpretation is disputed. Do not use disputed sums
for D/C, savings, or clean results. Runtime SDK descriptors and older encrypted
protobuf trajectories do not resolve native storage semantics automatically.

### SDK controls: separate prompt-assisted scope

The pinned SDK has real controls but no accepted passive saved-session contract:

| Check | Runtime/source control                                                 | Plan boundary                                                                             |
| ----- | ---------------------------------------------------------------------- | ----------------------------------------------------------------------------------------- |
| D     | `CompactionConfig.checkpoint_interval_tokens`, `max_context_tokens`    | Checkpoint summary and context eviction are separate; do not use deprecated threshold API |
| T     | `GeminiModelOptions.thinking_level`                                    | Model-dependent enum; configured is not served effort                                     |
| S     | Model targets, declared subagents, allowed agents, delegation depth    | Depth is not S itself; actual models and native execution join still required             |
| M/B   | Mutually exclusive enabled/disabled tool filters                       | Source states disabled built-ins are stripped; policy denial leaves them visible          |
| K     | Explicit `skills_paths`                                                | Ambient discovery/inheritance and saved activation remain unresolved                      |
| O     | `ModelTarget` and model-purpose resolution                             | Preserve endpoint/auth and text/image purposes                                            |
| F     | `ServiceTier.STANDARD`, `PRIORITY`, `FLEX` on supported model endpoint | Real inference control, unlike application execution Fast mode                            |
| C     | Runtime prompt/cached counts                                           | No general cache TTL/key control established; runtime events are not persistence          |

Provide reviewed source-edit prompts, not automatic arbitrary Python rewrites.
Never create a universal SDK `settings.json`. A user-selected existing save root
can become a future source only after its native schema and producer are pinned.
Do not scan arbitrary temporary directories or add logging to manufacture support.

## 14. Planned Action Parity

This is a target backlog, not delivered support. Every cell remains subject to
version, ownership, precedence, safe-write, and user-confirmation gates.

`Config` means a documented candidate control. `Narrow` means only a named
worker/resource or provider-specific control. `Guide` means native UI/command or
source-edit prompt. `Research` means the writer schema remains unproved.

| Check | OpenCode              | Pi                    | Codex         | Claude Code           | Cursor              | Antigravity applications |
| ----- | --------------------- | --------------------- | ------------- | --------------------- | ------------------- | ------------------------ |
| D     | Config                | Config                | Config        | Config                | Guide               | Guide                    |
| T     | Narrow preventive     | Config                | Config        | Config                | Guide/Research      | Guide/Research           |
| S     | Narrow                | Narrow example        | Config/Narrow | Narrow                | Narrow              | Narrow                   |
| M     | Config preventive     | Extension guide       | Config/Narrow | Narrow                | Guide/Research      | Narrow                   |
| B     | Narrow preventive     | Config preventive     | Narrow        | Narrow                | Research            | Narrow worker            |
| K     | Narrow preventive     | Config preventive     | Narrow        | Narrow                | Narrow preventive   | Narrow worker/Guide      |
| O     | Config                | Config                | Config        | Config                | Guide/Research      | Guide/Research           |
| F     | Narrow preventive     | SDK guide             | Config        | Config                | Narrow worker/Guide | No equivalent proved     |
| C     | Narrow cause-specific | Environment/SDK guide | Guide         | Narrow cause-specific | Guide               | Guide                    |

Do not count a prospective worker setting as action coverage for every historical
worker. Do not count a copied generic prompt as successful remediation. SDK
Antigravity can offer source guidance for more controls, but it is not application
config-edit parity.

## 15. Prompt and UI Contract

Every supported finding should have an exact, bounded prompt even when automatic
editing is unavailable. Cursor O is the first missing prompt to add. Preventive
recommendations must be visibly separate from failures and excluded from failure
counts and historical burn totals.

Each prompt should include the check observation or preventive reason, exact
agent/product, bounded target facts, desired scope, relevant documented controls,
version caveats, and verification limits. Ask the receiving agent to inspect the
effective config before editing, preserve capabilities, and return a diff and
activation instructions. Do not tell it to install collectors or gather private
evidence through live RPC.

Treat resource names and evidence text as quoted data, not instructions. Avoid
embedding private document bodies, commands containing credentials, or arbitrary
transcript excerpts. Use opaque local references where possible.

Batch prompts should have one shared instruction section and bounded per-target
facts, not repeated full templates. Return included and omitted counts, expose
selection, and provide a typed size-limit outcome. Persist any associated action
records transactionally; a failed copy preparation must not leave partial watches.

Review must show:

- Detected versus preventive reason.
- Agent, product surface, version gate, and selected scope.
- Existing setting, inserted setting, or new file.
- Old effective behavior and proposed behavior.
- Wider effects, including inherited defaults, workers, teammates, or products.
- Restart/refresh requirements and runtime override limits.
- Config-write verification versus unavailable behavioral/savings verification.

Use factual wording: above the reviewed effort cap, premium worker observed,
fast tier observed on delegated requests, or repeated paid context detected.
Do not say a task did not need reasoning/speed based solely on these checks.

Keep existing external-store React patterns and do not add `useEffect`. Read the
desktop design contract before UI changes. Show unsupported/truncated target
counts and actionable unavailable reasons rather than hiding coverage gaps.

## 16. Verification and Savings

### Verification levels

| Level                | Proof                                                        | User-facing claim                                             |
| -------------------- | ------------------------------------------------------------ | ------------------------------------------------------------- |
| Prepared             | Current supported setting resolved and diff ready            | Review available                                              |
| Written              | Intended file state and effective local resolution confirmed | Config updated; activation may be pending                     |
| Observed             | Later exact eligible activity uses changed control           | Change observed in later activity                             |
| Verified improvement | Reviewed target-specific transition and ownership            | Improvement verified within stated scope                      |
| Numeric savings      | Eligible units, rates, comparison, and non-overlap           | Bounded estimate or confirmed contribution, labeled by method |

File readback can verify preventive config application without proving a past
finding fixed. D historical maxima and old S call identities are immutable.
If future D/S verification targets a durable policy, introduce that target
explicitly; do not reuse historical identity and call absence a fix.

M/B/K can verify the current control disables a resource where the loader contract
proves it. That does not prove complete historical absence, future task success,
or exact tokens saved. Maintain those separate claims.

### Work and ownership

Do not enroll permanent unavailable prompts as active watches. Keep bounded action
history separately. Apply target/source/route/scope filters before session limits.
Dirty only relevant attempts after publication. Stream post-boundary activity with
resumable cursors and explicit correction invalidation.

Enforce exact source format where the watch contract requires it. A later source
format can participate only through an explicit reviewed compatibility rule.
Savings ownership must deduplicate activity across multiple source-specific
findings and physical-target attempts. A contribution belongs to eligible activity,
not merely to whichever attempt observed it first.

### Estimation

Use the same qualifying route/model/worker evidence as detection. Preserve
literal input tokens, assumed output tokens, cache-class tokens, API-equivalent
USD, and improvement counts as different units. Remove hidden reasoning reduction
defaults, fabricated positive minimums, and unrelated premium-worker estimates.

Missing prices, denominators, assumptions, model state, or ownership remain
unknown. Known zero and negative differences remain known. Separate subscription
credit multipliers from API prices. Do not promise monetary savings merely because
a replacement model is newer or a cache TTL is longer.

## 17. Performance and Memory Plan

The existing bounded detector collections and streaming cache query are useful.
Do not rewrite them without measurement. The principal risks are discovery,
adapter state, relationship ownership, repeated scans, snapshots, and verification.

| Area                      | Current risk                                                         | Planned approach                                                                                    |
| ------------------------- | -------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------- |
| Claude state              | UUID and usage maps grow with history and serialize repeatedly       | Indexed durable deduplication or a proved bounded replay window; no silent eviction/double counting |
| Thread resolution         | Long IDs and repeated root-string clones                             | Bound byte length before retention; compact/intern identities; count all parser state               |
| Skills/tools              | Invocation identity sets can bypass other map caps                   | Apply byte/cardinality limits before every owning insertion                                         |
| Claude fork preprocessing | Full parent scan, unbounded line reads, missing proof treated weakly | Bounded framing, cancellation, fingerprinted ownership, indexed reuse, partial on missing proof     |
| Cursor discovery          | Full transcript/store materialization and copies before parsing      | Keep files file-backed; metadata-first selection; row-streamed native readers                       |
| Cursor relationships      | All-pairs fork candidate comparisons                                 | Indexed native parent/root IDs and bounded candidate sets                                           |
| OpenCode SQLite           | Full-value fingerprints, ancestry map, repeated part queries         | Bound ancestry/values; cancellation/progress handling; inspect query plans; retain change detection |
| Reports                   | Repeated session scans and cohort-sized retained contributions       | Push filters into SQL, stream compatible aggregates, page display samples                           |
| Verification              | Broad dirtying and repeated unrelated-history scans                  | Exact indexed target filters, incremental cursors, bounded scheduling                               |
| SQLite companions         | Inconsistent multi-source reads                                      | Per-database snapshots plus companion fingerprints/rechecks                                         |

Budget the entire path: discovery, fingerprinting, framing, JSON/protobuf decode,
adapter state, sink, SQLite work, child folding, snapshot serialization, report
aggregation, and verification. The 8 MiB accumulator test is not a process bound.

Prefer one-pass normalization and indexed durable facts over repeated transcript
reads. Do not concatenate strings as an intermediate source when the original is
file-backed. Avoid cloning JSON to create usage keys. Preserve correction replay
and exact deduplication while optimizing.

### Benchmark design

Use synthetic 10k, 100k, and 1m-record inputs, plus near-limit and oversized
records. Measure cold full scans, no-change scans, single-record appends, many tiny
appends, source rewrites, many old sessions with one active session, and large
worker trees.

Record peak RSS, allocation high-water, retained adapter/snapshot bytes, bytes
read/copied, CPU time, SQLite rows visited, temporary sorting, cancellation
latency, and report/verification latency. Include Cursor discovery and SQLite
fingerprints, not only reader loops. Test 1, 30, and hundreds of descendants.

Set numeric budgets from a recorded baseline on supported hardware before merging
performance changes. Require bounded memory or an explicit fail-closed input limit,
bounded cancellation across all stages, and no accidental whole-history work for
tiny appends. Prefix validation may require deliberate extra I/O; document and
measure that correctness tradeoff rather than weakening validation silently.

## 18. Delivery Sequence

This sequence is the implementation checklist as of 2026-09-11. A checked item
was reported implemented with focused validation; it does not mean committed or
fully validated as a batch. The worktree remains uncommitted. An unchecked item remains required, including
items that have only been researched. Split work into focused changes; do not
implement all vendors or all controls in one patch.

### Phase 0: Preserve evidence and baseline boundaries

- [x] Reject resume after incomplete Claude, Codex, or Pi JSONL tails.
- [x] Validate the complete parsed prefix before resuming an append-only source.
- [x] Bound retained invoked-skill identities with the existing tool-name limit.
- [x] Keep Amp file-change records out of session discovery.
- [x] Reject unknown premium-looking models rather than treating them as premium evidence.
- [x] Exclude pre-replacement turns from old-model cause counts.
- [ ] Add focused regressions for all remaining P0/P1 findings in Section 5 before changing their behavior.
- [ ] Establish measured synthetic performance baselines before making performance-driven changes.

Exit condition: resume and source-admission contracts have regression coverage,
and no source or detector claim exceeds its accepted evidence.

### Phase 1: Complete write, trust, recovery, and report correctness

- [x] Re-resolve repository enabled/accessibility state before prepare, apply, and recovery.
- [x] Classify current scalar values as original, replacement, or conflict during recovery.
- [ ] Add complete-content recovery classification and release deferred-original reservations; exclude live apply from recovery.
- [x] Reject config writes when prepared file permissions or Unix ownership changed.
- [ ] Use practical no-follow/descriptor protections on supported platforms without requiring universal race freedom.
- [ ] Revalidate identity, permissions, bytes, and trusted scope before writing; serialize Antiburn apply/recovery by physical file.
- [ ] Add the non-blocking close-editor notice and latest `.backup` companion contract in Section 22.
- [ ] Test detected conflicts and recovery boundaries; document residual external-writer and power-loss limits.
- [ ] Complete Claude cross-layer environment resolution and current-default attribution separation.
- [ ] Enforce exact source/physical-target matching and non-overlapping ownership for old-model verification.
- [ ] Remove all hidden report assumptions and incomplete denominators; retain known zero and negative differences.
- [x] Separate display-only catalog estimates from detector-grade built-in-tool exposure.
- [ ] Segment mixed cache accounting by compatible family; retain current thresholds and implement the approved heuristic/sample policy in Section 22.
- [ ] Characterize OpenCode route aliases without broad fallback.
- [x] Preserve Cursor observations with equal text but distinct native identities.

Exit condition: all Section 5 P0/P1 correctness items, except requirements
explicitly superseded by Section 22, are implemented with regressions or the
affected operation, result, or savings claim is explicitly unavailable and
disabled. Disabling automatic writes alone does not resolve an incorrect report,
clean result, verification allocation, or prompt enrollment.

### Phase 2: Build versioned config resolver contracts

- [ ] Define vendor-specific resolver contracts for OpenCode, Pi, Codex, and Claude Code controls already eligible for action.
- [ ] Add pure fixtures for precedence, profiles, aliases, nested merge, list order, trust, managed sources, and runtime overrides.
- [ ] Model preventive recommendations independently from historical findings, failed-check counts, wins, and savings.
- [ ] Add durable control identity and typed operation metadata only where an implemented operation requires them.
- [ ] Add UI/state support for detected versus preventive reasons, scope, effects, activation, and unavailable verification.

Exit condition: each offered control resolves a physical target and effective
post-edit value through a versioned, tested contract.

Under the Section 22 decision, a documented file-default operation can still be
offered when runtime effect is unknown. Show `file updated` and the override
limit instead of claiming effective resolution. Unknown schema or physical
target remains unavailable.

### Phase 3: Safely insert missing settings and create supported files

- [ ] Extend prepared operations to represent missing leaves and missing files as distinct expected states.
- [ ] Add parser-specific insertion for JSONC, TOML, and Markdown while preserving comments, layout, unknown fields, and bodies.
- [ ] Add exclusive creation, latest backups for existing files, same-directory temporary files, supported sync, typed readback, and persisted uncertain-write handling under Section 22's best-effort contract.
- [ ] Reject concurrent key/file creation, duplicate semantic definitions, malformed documents, unsupported paths, and untrusted new directories.
- [ ] Re-resolve complete effective configuration after insertion or creation.
- [ ] Add deterministic tests for missing-key insertion, exclusive creation, crashes, symlink/ancestor replacement, and concurrent writes.

Exit condition: existing O/T controls can safely add one documented leaf or one
minimal file without copying inherited configuration or credentials.

### Phase 4: Deliver core model, effort, compaction, and fast-tier actions

- [ ] Add OpenCode, Pi, Codex, and Claude Code D compaction operations with exact scope and activation limits.
- [ ] Add Codex and Claude Code F default operations with exact service-tier semantics.
- [ ] Add only release-gated OpenCode provider effort/service-tier operations after route translation is characterized.
- [ ] Keep Pi F as SDK/source guidance unless a persisted core config contract is proved.
- [ ] Add prepared, written, observed, and verified state transitions without reusing immutable historical D/S identities.

Exit condition: every automatic D/T/O/F operation has an exact control, version
gate, resolver, safe write, readback, and stated verification limit.

### Phase 5: Deliver existing named-worker and resource controls

- [ ] Add existing named-worker model/effort edits for the applicable OpenCode, Pi, Codex, and Claude Code contracts.
- [ ] Prove durable worker-definition ownership, precedence, and actual delegation linkage before action eligibility.
- [ ] Add exact existing MCP disable, built-in schema-removal, and skill-control operations where the loader contract proves semantics.
- [ ] Keep resource controls preventive-only when historical exposure is incomplete.
- [ ] Add exact bounded prompts for every eligible finding, beginning with Cursor O.
- [ ] Add transactional batch prompt enrollment, size limits, omitted counts, action expiry, and copy analytics deduplication.

Exit condition: no worker/resource action uses a model-only stub, a guessed
disable key, or a generic prompt as evidence of parity.

### Phase 6: Characterize and add passive native evidence

- [ ] Characterize Pi producer metadata, cache accounting, compaction, and selected-skill wrappers against versioned fixtures.
- [ ] Characterize Codex newer rollout records, world-state patches, deferred tools, completed items, and selected-skill fragments.
- [ ] Add OpenCode CoreV2 as a separate source contract with sequence, mutation, epoch, and dual-schema deduplication tests.
- [ ] Characterize Cursor native CLI/IDE persistence for lossless O extraction, then add exact-target prompts.
- [ ] Characterize Antigravity CLI transcript/SQLite/generation evidence and separately characterize native MCP ownership/merge behavior.
- [ ] Keep Cursor D/T/S/F and Antigravity S/C unavailable until their exact persisted evidence requirements are proved.

Exit condition: each new format has producer/version evidence, synthetic
fixtures, bounded processing, and only the clean/finding eligibility it proves.

### Phase 7: Add cache, verification, and savings parity

- [ ] Implement cause-specific cache actions only for reviewed routes, accounting, sample policy, and rate data.
- [ ] Add source/target/route/scope filtered verification cursors and correction invalidation.
- [ ] Deduplicate later activity across findings and physical-target attempts.
- [ ] Separate file-readback, observed behavior, verified improvement, and numeric savings in persistence and UI.
- [ ] Add M/B/K control verification only when the loader proves exact current resource disablement.

Exit condition: verification and estimates use the same qualifying evidence as
detection and do not convert missing proof into savings.

### Phase 8: Bound processing and validate platform support

- [ ] Bound Claude state, thread identities, fork preprocessing, Cursor discovery/relationships, OpenCode SQLite state, reports, and verification scheduling.
- [ ] Implement indexed durable facts or bounded replay without silent eviction or double counting.
- [ ] Run the Section 17 synthetic benchmark matrix and record hardware-specific budgets.
- [ ] Add native Windows ACL, reparse-point, sharing, replacement, and crash-recovery write tests before enabling Windows writes.
- [ ] Define a passive Linux-side WSL path/environment contract before enabling WSL writes.

Exit condition: supported paths meet recorded memory, cancellation, and
incremental-work budgets; unsupported platforms remain unavailable.

### Phase 9: Release validation and coverage maintenance

- [ ] Run the Section 19 acceptance matrix for each delivered operation and source format.
- [ ] Run required formatter, lint, type, test, analytics, design, and `aislop` checks for each implementation batch.
- [ ] Update `docs/session-coverage.md` and `docs/check-coverage.md` with every accepted parsing or eligibility change.
- [ ] Record dated maintainer confirmation and reviewed passive alternatives for each newly accepted source shape.
- [ ] Keep failed research gates as explicit unavailable reasons and bounded guidance.

Exit condition: released claims, coverage baselines, tests, UI states, and
platform availability agree.

Phases 2 and 6 can proceed in parallel after Phase 1 write safety. Phase 6
research can also proceed independently. Shipping an editor operation depends on
the Phase 1 and Phase 3 safety exits. A failed research gate must produce a
documented unavailable reason and useful guidance, not a fallback that guesses at
evidence or configuration.

## 19. Acceptance Tests

### Native evidence

- Compare full and resumed normalized rows, facts, findings, and clean states at
  arbitrary byte splits, including UTF-8, escapes, and the final newline.
- Rewrite head, middle, and saved tail independently; replace files and mutate
  companion sources while retaining apparent timestamps or counts.
- Test request/model/provider/effort/tier transitions, missing and unknown fields,
  malformed records, and unknown evidence-bearing source types.
- Test forks, inherited usage, reused children, background tasks, duplicate claims,
  missing sidecars, and exact actual-parent/actual-worker model joins.
- Preserve distinct equal-text observations in Cursor.
- Test catalog availability versus selected full skills, preloads, repeated
  invocation, truncation, compaction reattachment, and lookalike wrappers.
- Reject resource clean results when historical inventory is incomplete.
- Test cache accounting families, first-request baseline, sample floors, TTL
  subclasses, retries, compaction, route switches, and missing request identities.
- Test Codex world-state patches and paginated history separately from legacy
  rollouts; never infer persisted AdditionalTools.
- Test CoreV2 sequence gaps, mutations, epoch replacement, and dual-schema dedupe.
- Keep Cursor gauges, Antigravity step counts, and runtime SDK events outside D
  until request semantics and persistence are proved.

### Config and write safety

- Version-boundary fixtures for every advertised field and path rule.
- Global/project/profile/worker precedence, aliases, nested merge, list order,
  duplicate semantic definitions, managed restrictions, CLI/SDK overrides, and
  changed trust/CWD/worktree state.
- Pi model-specific global value versus project fallback and first-file trust.
- Claude cross-file environment, same-source effort precedence, effort holds,
  force worker model, and model-default versus active-session behavior.
- Codex old/new profiles, document-path skills, source-layer restrictions, and
  root tier overriding worker role tier.
- OpenCode variant-over-agent options, post-merge overrides, exact schema removal,
  JSONC/Markdown collisions, and CoreV2 syntax isolation.
- Cursor manager-selected MCP scope and undocumented model object rejection.
- Antigravity legacy keys preserved, unknown tombstones rejected, product-specific
  roots, and customization inheritance.
- Existing-file replace, missing-key insertion, exclusive new-file creation,
  symlink/ancestor replacement, mode/owner changes, readback failure, and agent
  concurrent write at deterministic boundaries.
- Crashes before write, after replacement, before durable finalization, and during
  any later multi-file operation; original state releases safe reservations.
- Windows ACL/reparse/sharing and WSL environment separation on native machines.

### UI, verification, and analytics

- Exact prompts for every eligible finding, unsupported reason for every other
  target, real-template batch bounds, clipboard failure/retry, and action expiry.
- Two expanded checks whose combined targets exceed the old shared cache limit.
- Prompt-only action retention cannot block a supported automatic edit.
- Preventive suggestions do not increase failed-check counts or verified wins.
- Config-written, activation-pending, observed, verified, recurred, and unavailable
  states remain distinct across restart and correction replay.
- Old-model exact-source matching and shared-activity ownership across attempts.
- Missing denominators/prices, known zero/negative estimates, mixed units, and
  duplicate contribution ownership.
- Review shows scope, shared effects, insertion/creation, restart requirements,
  unsupported runtime overrides, and local privacy behavior.
- Analytics follows [analytics.md](../analytics.md) and
  [measurement definitions](../analytics-measurement.md). Add reviewed fixed
  categories for preventive versus detected actions only if needed. Never send
  paths, model/resource names, values, prompts, exact costs, or stable target IDs.
- One successful-copy measurement per prepared prompt; visible review readiness
  only after accepting the result. Keep denominators limited to supported
  agent/product/version/platform cohorts and expose unavailable-action rates.

### Commands for implementation changes

Run engine checks from `crates/antiburn-local`:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo test --test check_coverage_contract
```

Run shell checks from `apps/desktop/src-tauri`:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Run frontend checks from the repository root:

```sh
pnpm --filter @antiburn/desktop lint
pnpm --filter @antiburn/desktop type-check
pnpm --filter @antiburn/desktop test
pnpm --filter @antiburn/desktop build
```

Run `aislop scan --changes` after a coherent supported-language implementation
batch and before final project validation. Use the actual PR base for branch
work. Run analytics-enabled shell checks with the documented loopback collector
when instrumentation changes. Run the design contract check when styles/tokens
change. This plan-only change does not require application builds or scans of
unchanged code.

Update session and check coverage together when parsing changes affect check
eligibility. Keep every `SourceFormat` exactly once in baseline inventories and
matrices. Add new dated confirmations with reviewed passive alternatives. Do not
convert this plan's research candidates into current baseline support.

## 20. Open Research Gates

| Gate                                   | Next investigation                                                                       | Allowed fallback                                                      |
| -------------------------------------- | ---------------------------------------------------------------------------------------- | --------------------------------------------------------------------- |
| Historical resource exposure           | Exact persisted definition inventories and request joins                                 | Scoped findings or preventive advice; no session-wide clean           |
| OpenCode effective effort/speed        | New persisted final controls with producer semantics                                     | Preventive route-specific settings only                               |
| Pi final provider effort               | Payload mutation boundary and persisted native metadata                                  | Preserve selected-policy T                                            |
| Cursor actual models and request input | Pin native bubble/binary semantics and exact IDs                                         | O improvements where proved; separate snapshot advice                 |
| Cursor MCP/model persistence           | Observe documented manager writes through public producer/schema evidence                | Native UI prompt, no guessed JSON                                     |
| Antigravity exact parent/worker proof  | Join both served model generations with native delegation                                | S unavailable                                                         |
| Antigravity cache accounting           | Descriptor-backed input/cache/retry/request semantics                                    | C unavailable and no estimated cache tokens                           |
| SDK persistence                        | Native saved format under an explicitly selected existing root                           | Source-edit guidance only                                             |
| Unreleased/new config fields           | Release containment and loader fixtures                                                  | Hide editor operation for unknown versions                            |
| Remote/managed/runtime precedence      | Safe local proof of effective scope without execution                                    | Explain blocked edit and provide bounded guidance                     |
| Concurrent agent-owned files           | Best-effort conflict checks, latest backup, serialized local writes, close-editor notice | Allow reviewed writes without stopped-agent proof; no race-free claim |

## 21. Source Index

All rolling URLs below were researched on 2026-09-11. Commit and crate-version
links are immutable research anchors. Secondary decoders are explicitly not
authoritative producer contracts.

### OpenCode primary sources

[oc-config]: https://opencode.ai/docs/config/
[oc-agents]: https://opencode.ai/docs/agents/
[oc-skills]: https://opencode.ai/docs/skills/
[oc-loader]: https://github.com/anomalyco/opencode/blob/193de13a88d62a6409c6d385831180f1def527dc/packages/opencode/src/config/config.ts
[oc-paths]: https://github.com/anomalyco/opencode/blob/193de13a88d62a6409c6d385831180f1def527dc/packages/opencode/src/config/paths.ts
[oc-request]: https://github.com/anomalyco/opencode/blob/193de13a88d62a6409c6d385831180f1def527dc/packages/opencode/src/session/llm/request.ts
[oc-transform]: https://github.com/anomalyco/opencode/blob/193de13a88d62a6409c6d385831180f1def527dc/packages/opencode/src/provider/transform.ts
[oc-sql]: https://github.com/anomalyco/opencode/blob/193de13a88d62a6409c6d385831180f1def527dc/packages/core/src/session/sql.ts
[oc-message]: https://github.com/anomalyco/opencode/blob/193de13a88d62a6409c6d385831180f1def527dc/packages/schema/src/session-message.ts
[oc-epochs]: https://github.com/anomalyco/opencode/blob/193de13a88d62a6409c6d385831180f1def527dc/packages/core/src/session/context-epoch.ts
[oc-guidance]: https://github.com/anomalyco/opencode/blob/193de13a88d62a6409c6d385831180f1def527dc/packages/core/src/skill/guidance.ts

Additional primary evidence: [permission exclusion][oc-permissions],
[V1 config schema][oc-schema], [worker producer][oc-task], and
[compaction implementation][oc-compaction].

[oc-permissions]: https://github.com/anomalyco/opencode/blob/193de13a88d62a6409c6d385831180f1def527dc/packages/opencode/src/permission/index.ts
[oc-schema]: https://github.com/anomalyco/opencode/blob/193de13a88d62a6409c6d385831180f1def527dc/packages/core/src/v1/config/config.ts
[oc-task]: https://github.com/anomalyco/opencode/blob/193de13a88d62a6409c6d385831180f1def527dc/packages/opencode/src/tool/task.ts
[oc-compaction]: https://github.com/anomalyco/opencode/blob/193de13a88d62a6409c6d385831180f1def527dc/packages/opencode/src/session/compaction.ts

### Pi primary sources

[pi-settings]: https://github.com/badlogic/pi-mono/blob/d12cd92e45e308d4af000554292165ef1984253b/packages/coding-agent/docs/settings.md
[pi-settings-source]: https://github.com/badlogic/pi-mono/blob/d12cd92e45e308d4af000554292165ef1984253b/packages/coding-agent/src/core/settings-manager.ts
[pi-trust]: https://github.com/badlogic/pi-mono/blob/d12cd92e45e308d4af000554292165ef1984253b/packages/coding-agent/src/core/project-trust.ts
[pi-sdk]: https://github.com/badlogic/pi-mono/blob/d12cd92e45e308d4af000554292165ef1984253b/packages/coding-agent/src/core/sdk.ts
[pi-changelog]: https://github.com/badlogic/pi-mono/blob/d12cd92e45e308d4af000554292165ef1984253b/packages/coding-agent/CHANGELOG.md
[pi-workers]: https://github.com/badlogic/pi-mono/blob/d12cd92e45e308d4af000554292165ef1984253b/packages/coding-agent/examples/extensions/subagent/index.ts

Additional primary evidence: [assistant types][pi-types],
[Anthropic effort producer][pi-anthropic], [Responses stream accounting][pi-responses],
and [agent runtime/tool/skill behavior][pi-runtime].

[pi-types]: https://github.com/badlogic/pi-mono/blob/d12cd92e45e308d4af000554292165ef1984253b/packages/ai/src/types.ts
[pi-anthropic]: https://github.com/badlogic/pi-mono/blob/d12cd92e45e308d4af000554292165ef1984253b/packages/ai/src/api/anthropic-messages.ts
[pi-responses]: https://github.com/badlogic/pi-mono/blob/d12cd92e45e308d4af000554292165ef1984253b/packages/ai/src/api/openai-responses-shared.ts
[pi-runtime]: https://github.com/badlogic/pi-mono/blob/d12cd92e45e308d4af000554292165ef1984253b/packages/coding-agent/src/core/agent-session.ts

### Codex primary sources

[cx-config]: https://learn.chatgpt.com/codex/config-file/config-reference
[cx-advanced]: https://learn.chatgpt.com/codex/config-file/config-advanced
[cx-loader]: https://github.com/openai/codex/blob/fc948f8c473e5d11e780ffcf1fd7f812a2020932/codex-rs/config/src/loader/mod.rs
[cx-skills]: https://learn.chatgpt.com/codex/build-skills
[cx-workers]: https://learn.chatgpt.com/codex/agent-configuration/subagents
[cx-role]: https://github.com/openai/codex/blob/fc948f8c473e5d11e780ffcf1fd7f812a2020932/codex-rs/core/src/agent/role.rs
[cx-skill-rules]: https://github.com/openai/codex/blob/fc948f8c473e5d11e780ffcf1fd7f812a2020932/codex-rs/config/src/skills_config.rs
[cx-skill-host]: https://github.com/openai/codex/blob/fc948f8c473e5d11e780ffcf1fd7f812a2020932/codex-rs/ext/skills/src/host_service.rs
[cx-policy]: https://github.com/openai/codex/blob/fc948f8c473e5d11e780ffcf1fd7f812a2020932/codex-rs/rollout/src/policy.rs
[cx-protocol]: https://github.com/openai/codex/blob/fc948f8c473e5d11e780ffcf1fd7f812a2020932/codex-rs/protocol/src/protocol.rs
[cx-world]: https://github.com/openai/codex/blob/fc948f8c473e5d11e780ffcf1fd7f812a2020932/codex-rs/core/src/context/world_state/mod.rs
[cx-tools]: https://github.com/openai/codex/blob/fc948f8c473e5d11e780ffcf1fd7f812a2020932/codex-rs/core/src/context/world_state/tools.rs
[cx-fragments]: https://github.com/openai/codex/blob/fc948f8c473e5d11e780ffcf1fd7f812a2020932/codex-rs/ext/skills/src/fragments.rs

Additional primary evidence: [spawn precedence][cx-spawn], [tier types][cx-tier],
[model limits and lifecycle metadata][cx-models], and [internal cache key][cx-client].

[cx-spawn]: https://github.com/openai/codex/blob/fc948f8c473e5d11e780ffcf1fd7f812a2020932/codex-rs/core/src/tools/handlers/multi_agents_common.rs
[cx-tier]: https://github.com/openai/codex/blob/fc948f8c473e5d11e780ffcf1fd7f812a2020932/codex-rs/protocol/src/config_types.rs
[cx-models]: https://github.com/openai/codex/blob/fc948f8c473e5d11e780ffcf1fd7f812a2020932/codex-rs/protocol/src/openai_models.rs
[cx-client]: https://github.com/openai/codex/blob/fc948f8c473e5d11e780ffcf1fd7f812a2020932/codex-rs/core/src/client.rs

### Claude Code primary sources

[cc-settings]: https://code.claude.com/docs/en/settings
[cc-reference]: https://code.claude.com/docs/en/settings-reference
[cc-models]: https://code.claude.com/docs/en/model-config
[cc-workers]: https://code.claude.com/docs/en/sub-agents
[cc-mcp]: https://code.claude.com/docs/en/mcp
[cc-skills]: https://code.claude.com/docs/en/skills
[cc-fast]: https://code.claude.com/docs/en/fast-mode
[cc-cache]: https://code.claude.com/docs/en/prompt-caching

Also consult [environment precedence](https://code.claude.com/docs/en/env-vars)
and [CLI overrides](https://code.claude.com/docs/en/cli-reference) for each operation.

### Cursor primary and secondary sources

[cu-config]: https://cursor.com/docs/cli/reference/configuration
[cu-changelog]: https://cursor.com/docs/cli/changelog
[cu-workers]: https://cursor.com/docs/subagents
[cu-skills]: https://cursor.com/docs/skills
[cu-mcp]: https://cursor.com/docs/mcp
[cu-governance]: https://cursor.com/docs/enterprise/model-and-integration-management
[cu-format]: https://docs.rs/crate/txcript/0.8.0/source/docs/formats/cursor.md
[cu-parser]: https://github.com/tim-hua-01/cc_transcript_viewer/blob/ef068f7a07f6f083d208c2084767f01e1e30928c/cursor_parser.py
[cu-gauge]: https://github.com/getagentseal/codeburn/issues/574
[cu-vct]: https://docs.rs/vct-core/2.7.1/src/vct_core/session/cursor.rs.html

The last four references are secondary format research. The txcript contract
describes an observed 2026.06.26 CLI build, not all Cursor history. Do not adopt
decoder estimates that relabel context gauges as cache tokens.

### Antigravity primary and secondary sources

[ag-mcp]: https://antigravity.google/docs/mcp/
[ag-releases]: https://github.com/google-antigravity/antigravity-cli/blob/main/CHANGELOG.md
[ag-settings]: https://antigravity.google/docs/settings/
[ag-cli-settings]: https://antigravity.google/docs/cli/settings/
[ag-workers]: https://antigravity.google/docs/subagents/
[ag-sdk]: https://github.com/google-antigravity/antigravity-sdk-python/commit/52ea99480960ed02be1561f6fe57b99e7186962a
[ag-format]: https://docs.rs/crate/txcript/0.8.0/source/docs/formats/antigravity.md
[ag-native]: https://github.com/rimio-ai/rimz/blob/6246f75399061b2dd97fd450960026a86437b7d0/docs/externals/agent-adapter/antigravity-reference.md
[ag-usage]: https://github.com/junhoyeo/tokscale/pull/713

Additional primary sources: [CLI effort release](https://github.com/google-antigravity/antigravity-cli/releases/tag/1.1.5),
[CLI diagnostic release](https://github.com/google-antigravity/antigravity-cli/releases/tag/1.1.28),
[2.0 skills](https://antigravity.google/docs/skills/),
[IDE skills](https://antigravity.google/docs/ide/skills/), and
[CLI launch controls](https://antigravity.google/docs/cli/headless/).

SDK implementation: [types][ag-sdk-types], [models][ag-sdk-models],
[local config][ag-sdk-config], and [transport mapping][ag-sdk-local].

[ag-sdk-types]: https://github.com/google-antigravity/antigravity-sdk-python/blob/52ea99480960ed02be1561f6fe57b99e7186962a/google/antigravity/types.py
[ag-sdk-models]: https://github.com/google-antigravity/antigravity-sdk-python/blob/52ea99480960ed02be1561f6fe57b99e7186962a/google/antigravity/models.py
[ag-sdk-config]: https://github.com/google-antigravity/antigravity-sdk-python/blob/52ea99480960ed02be1561f6fe57b99e7186962a/google/antigravity/connections/local/local_connection_config.py
[ag-sdk-local]: https://github.com/google-antigravity/antigravity-sdk-python/blob/52ea99480960ed02be1561f6fe57b99e7186962a/google/antigravity/connections/local/local_connection.py

The txcript, RimZ, and tokscale links are secondary evidence. Do not adopt RimZ
hooks/RPC collection, tolerate corrupt prefixes as clean, or infer all product
roots from a community scanner. Public older encrypted-trajectory extraction
tools and closed unmerged broad-scanning PRs are not accepted native contracts.

## 22. Continuation Handoff

Follow-up research date: 2026-09-11. This section records static code inspection,
fetched primary documentation and producer source, secondary-format limitations,
and the maintainer's answers. Only this plan was edited during the research task.
No application code, coverage baseline, producer fixture, or benchmark changed.

### 22.1 Decisions and priority

These are product decisions, not new upstream guarantees or source acceptances.

| Topic              | Maintainer decision                                                                                                                                          | Implementation consequence                                                                                                                                                           |
| ------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| External writers   | Allow editing without knowing whether the agent will overwrite it. Recommend closing the editor in a simple apply-dialog notice.                             | No stopped-process check, required shutdown confirmation, or mandatory vendor locking protocol. Serialize our own writes and detect observable conflicts.                            |
| Safety             | Aim not to corrupt anything. Keep a `.backup` companion and prefer atomic writes; universal atomicity is not required.                                       | Do not make a complete hostile-writer, filesystem, or power-loss proof a Phase 1 prerequisite. Do not deliberately fall back to unsafe live-file truncation.                         |
| Backup retention   | Keep only the latest backup. The approved question specified no broader access than the original, local storage, and no automatic rollback over later edits. | Refresh `<config>.backup` before each existing-file write. No numbered history. Show backup creation locally; keep content and paths out of telemetry.                               |
| Config eligibility | Prefer the approach that enables config editing for the most agents.                                                                                         | Offer edits to documented current defaults even when historical launch provenance is unknown. Describe the exact file change and activation limits, not proven historical causation. |
| Cache policy       | Keep existing cache thresholds and use best guesses for each agent/model where needed.                                                                       | Do not block Phase 1 on statistical calibration. Keep heuristics explicit and versioned. Do not fabricate missing native counters, routes, actual models, or verified savings.       |

Suggested apply notice: "Close your editor before applying to reduce the chance
of conflicting changes. We'll save the current config to a .backup file."
This is non-blocking. A separate short effect note can say that active sessions
may keep their current settings. Avoid forcing the user through an advanced
filesystem or provenance questionnaire.

For current-default edits, distinguish `file updated` from `effective local
default confirmed` when a launch override or managed input remains unknown.
Known managed restrictions, an unknown JSON schema, credentials, or ambiguous
physical ownership are not solved by broader eligibility. Offer the known exact
operation or native-manager guidance instead of guessing a field.

The following are implementation recommendations, not separately approved
numeric policies: retain current cache sample eligibility and ratio definitions
for the initial correctness patch; do not add an arbitrary sample floor or
silently reuse old thresholds on a newly defined metric. Include the initial
paid baseline in descriptive totals and test it separately from the retained
pair-only detector ratio. Later heuristic changes can proceed without another
research gate if they are labeled, tested, and revisioned.

### 22.2 Next work order

1. Re-read the dirty worktree and add failing tests for backup collisions,
   apply/recovery exclusion, deferred-original recovery, and native OpenCode
   verification. Do not revert unrelated work or trust the previous full-test claim.
2. Implement the practical writer contract below: complete backup, observable
   conflict checks, same-directory replacement where available, ownership-aware
   temporary cleanup, local serialization, and the short UI notice.
3. Fix Claude all-layer inspection and label current-default matching separately.
   Fix Pi merge-then-lookup while introducing its resolver fixtures.
4. Fix exact-source old-model verification, native route consistency, contribution
   ownership, report units/denominators, and independent cache segments.
5. Complete the Section 5 P1 prompt/watch defects. They are scheduled in Phase 5
   but still belong to the Phase 1 exit unless explicitly left unavailable.
6. Continue Phases 2-5 with small documented operations. Characterize Pi, Codex,
   and CoreV2 in parallel without promoting unproved source eligibility.
7. Measure before performance-driven changes. Windows and WSL need native
   implementation/testing; a general safety relaxation does not make Unix calls
   work on those platforms.

### 22.3 Writer and recovery implementation map

Paths in this subsection are relative to `apps/desktop/src-tauri/src/`.

| Owner                                                                                            | Static finding                                                                                                      | Next test or change                                                                                                                                        |
| ------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `agent_config/filesystem.rs::read_checked`                                                       | Path metadata checks bracket a separate path read. Size validation must remain bounded during growth.               | Read through the validated handle where practical; stop at `MAX_CONFIG_BYTES + 1`. Test substituted symlink, FIFO, and growing file.                       |
| `filesystem.rs::reject_symlink_ancestors`, `FileIdentity`, `check_owner`                         | Unix device/inode identify an object, not its content version. Ancestor checks do not freeze namespace or metadata. | Keep identity and byte checks separate. Reject observed unsafe targets; do not describe them as a snapshot.                                                |
| `filesystem.rs::create_temporary`, `agent_config/editor.rs::apply`                               | Error cleanup can remove the temporary path even when exclusive creation failed.                                    | Inject a temporary-name collision. The pre-existing file must remain untouched. Track whether we created the entry.                                        |
| `editor.rs::apply`                                                                               | Last byte check precedes `fs::rename`; readback checks a selected value.                                            | Add latest-backup ordering and complete-content readback. Re-resolve accepted layers when claiming an effective local value.                               |
| `agent_config/config.rs::PreparedChange`, `ApplyError::replacement_may_have_occurred`            | In-memory original bytes/metadata exist; only readback errors currently mean publication may have occurred.         | Represent backup and publication stages sufficiently to report failures without blindly retrying a possibly completed replacement.                         |
| `remediation/mod.rs::apply_prepared_at`, `revalidate_prepared`                                   | A `writing` row becomes visible before filesystem apply finishes.                                                   | Share file-level ownership with recovery. A recovery worker must not cancel a live apply.                                                                  |
| `remediation/recovery.rs::recover_uncertain_write`                                               | Scalar equality is described as original/replacement state. Unrelated bytes can have changed.                       | Compare complete-content fingerprints when available; otherwise report a limited current-value observation, not exact original/replacement identity.       |
| `store/remediation.rs::defer_remediation_recovery`, `cancel_pre_replacement_write`               | Deferred state becomes `recoveryNeeded`; cancellation currently accepts only `writing`.                             | Test deferred recovery followed by unchanged original. Release the reservation and restore any upgraded watch correctly.                                   |
| `store/remediation.rs::next_remediation_write_recovery`, `insights_worker.rs::process_next_work` | Recovery selects `writing` as well as `recoveryNeeded`.                                                             | Pause apply after persistence; prove recovery cannot act on its live reservation.                                                                          |
| `store/mod.rs::from_connection`                                                                  | WAL is requested and `synchronous=NORMAL` is used for the mixed cache/remediation store.                            | Record the power-loss limitation. A stronger durability mode is an optional later improvement, not the newly approved initial gate.                        |
| `remediation/config.rs::trusted_workspace_still_matches`                                         | Refreshed repository authorization is already implemented.                                                          | Preserve it. Coordinate our own trust-change boundary where practical; do not require reconstructing vendor launch trust to edit a reviewed local default. |

#### Backup and publication sequence

Use the existing editor and remediation lifecycle, not a second writer.

1. Serialize by physical file, not selector: two settings in one JSON document
   share the whole-file replacement resource. Recovery uses the same ownership.
2. Refresh scope/trust and reread the bounded document. If identity, bytes, or
   relevant permissions differ from review, require a fresh review. An unrelated
   observed edit is not permission to silently merge during apply.
3. For an existing file, prepare the full original bytes in a separate exclusive
   temporary for `<config>.backup`. Use restrictive permissions while populating
   it. Do not include a parsed/merged reconstruction in place of the original.
4. If the backup path exists, accept only the expected safe regular companion;
   reject links, directories, unsafe ownership, or unsupported access handling.
   This reserved path is overwritten with the latest original, as approved.
   Do not overwrite a file merely because a canonicalized symlink points to it.
5. Publish the backup, preferably by same-directory rename, before publishing the
   config. If backup creation/readback fails, leave the config unchanged. A crash
   between these steps can update the backup without changing the config; that
   is acceptable and must not be reported as an applied change.
6. Write the proposed complete config to its own exclusive temporary. Preserve
   supported metadata; never broaden backup access or silently strip security
   metadata that cannot be preserved safely. Reject that operation with a clear
   reason rather than block every ordinary configuration on a global proof gate.
7. Recheck observable conflicts, then replace the config using the platform's
   reviewed operation. Sync supported handles where practical. Never retry a
   publication of uncertain outcome as though nothing happened.
8. Read back and parse the result. Distinguish successful file update, overridden
   or unknown local effect, and failed/uncertain readback. Finalize the attempt.

No automatic restoration from `.backup`: another writer may have made a newer
change. An explicit restore is another reviewed write. Do not add a general
restore UI or backup-retention subsystem unless needed; manual recovery guidance
is enough for the first delivery. Backups can contain credentials. Keep them out
of analytics, logs, prompt bodies, and committed fixtures. They inherit the
target's scope: a project backup may be visible to Git or a sync tool. Mention
that effect in review; do not silently alter global Git ignores.

Missing-file creation has no previous contents and creates no fake empty backup.
Use exclusive/no-replace publication. Missing-parent creation can be implemented
incrementally when needed, but needs its own trust, permissions, failure, and
cleanup tests. Never remove a directory another writer populated.

#### Actual API boundaries

These findings explain implementation choices; they are not all release gates.

| API/source                                                                                                                       | Guarantee useful to this plan                                                                                 | Limit                                                                                                                                         |
| -------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------- |
| [Linux `open/openat`](https://man7.org/linux/man-pages/man2/open.2.html)                                                         | An open descriptor retains the opened object. Combined `O_CREAT` and `O_EXCL` reject an existing final entry. | `O_NOFOLLOW` covers the final component only; opened objects remain mutable.                                                                  |
| [Linux `openat2`](https://man7.org/linux/man-pages/man2/openat2.2.html)                                                          | Linux 5.6+ offers `RESOLVE_BENEATH` and all-component `RESOLVE_NO_SYMLINKS`.                                  | Runtime support is required. `RESOLVE_NO_XDEV` also rejects bind mounts. Do not silently claim equivalent containment from a weaker fallback. |
| [POSIX `renameat`](https://pubs.opengroup.org/onlinepubs/9799919799/functions/rename.html)                                       | Atomic namespace replacement relative to parent descriptors.                                                  | No expected-inode/hash argument. It is not compare-and-swap and does not stop same-inode writes.                                              |
| [Linux `renameat2`](https://man7.org/linux/man-pages/man2/rename.2.html)                                                         | `RENAME_NOREPLACE` prevents overwriting a concurrently created destination on supporting filesystems.         | `RENAME_EXCHANGE` is not compare-and-swap. NFS can report failure after performing a rename.                                                  |
| [Apple rename manual](https://raw.githubusercontent.com/apple-oss-distributions/xnu/main/bsd/man/man2/rename.2)                  | `renameatx_np` with `RENAME_EXCL` supports no-replace publication where the filesystem supports it.           | Current source is not an OS-version guarantee. `RENAME_SWAP` does not solve concurrent destination replacement.                               |
| [Apple open manual](https://raw.githubusercontent.com/apple-oss-distributions/xnu/main/bsd/man/man2/open.2)                      | `openat` dates to OS X 10.10; component-wise descriptor traversal is a practical baseline.                    | Newer all-path flags need separate SDK/runtime admission. Approved root aliases and symlinks below the root are different cases.              |
| [Linux `fsync`](https://man7.org/linux/man-pages/man2/fsync.2.html)                                                              | Flushes file data/metadata; parent directory needs a separate sync.                                           | Rename alone is not durability. Errors can arrive late.                                                                                       |
| [Apple `fsync`](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fsync.2.html) | Documents `F_FULLFSYNC` for stronger device ordering.                                                         | Ordinary sync may leave device-cache data vulnerable to power loss. Do not promise stronger durability than tested.                           |
| [SQLite WAL](https://www.sqlite.org/wal.html), [fullfsync](https://www.sqlite.org/pragma.html#pragma_fullfsync)                  | `FULL` syncs WAL per commit; Apple full flush is a separate setting.                                          | `NORMAL` can lose committed intent after power loss. A config backup is not a transaction with the store.                                     |
| [Linux ACLs](https://man7.org/linux/man-pages/man5/acl.5.html)                                                                   | ACLs can govern access beyond mode bits.                                                                      | Equal mode bits do not prove equal security policy; temporary-file inheritance can differ.                                                    |

Use deterministic barriers rather than sleeps for conflict tests. Cover ancestor
replacement, same-inode rewrite, mode/owner change, temp and backup collision,
backup symlink, backup failure, changed higher-priority file, two selectors in one
file, and external mutation after the last check. The last case characterizes
the acknowledged race; do not write a test that falsely asserts a guarantee the
API cannot provide. Test process termination before backup, after backup, after
config replacement, and before store finalization. These are process-crash tests,
not power-loss experiments.

Recovery observes, classifies, and releases or preserves reservations. Exact
original can release a reservation; exact proposed state can establish a current
replacement observation. Matching only a scalar, missing replacement files,
changed trust, or changed unrelated content must not trigger automatic rollback.
Original-after-restart does not prove no earlier write occurred. Existing stored
records lack any newly introduced fingerprints, so provide an explicit limited
legacy classification rather than inventing them. Fingerprints avoid persisting
another full credential-bearing document in the database.

### 22.4 Resolver contracts ready for implementation

#### Claude: all layers, then each control

Local owners: `agent_config/vendors/claude.rs::{resolve_target,resolve_reasoning,
reasoning_selector,reject_settings_overrides,reject_runtime_overrides}` and
`remediation/vendors/claude.rs::{runtime_override_present,
managed_configuration_present}`. `ConfigContext` currently uses booleans that
cannot distinguish unobserved runtime/managed input from proved absence.

Both model and effort resolution can return before inspecting lower files.
Fixture: local `model=A`, user `env.ANTHROPIC_MODEL=B`. The environment control
can win despite the local candidate. Add the equivalent fixture for
`CLAUDE_CODE_EFFORT_LEVEL`. Fix by collecting accepted layers before choosing a
control, not by adding another early-return exception.

Primary rolling sources: [environment](https://code.claude.com/docs/en/env-vars),
[managed settings](https://code.claude.com/docs/en/managed-settings),
[server policy](https://code.claude.com/docs/en/server-managed-settings),
[CLI](https://code.claude.com/docs/en/cli-reference), [settings][cc-settings],
[reference][cc-reference], and [models][cc-models], fetched 2026-09-11.

| Control/boundary | Researched rule                                                                                                                                                                        | Fixture or limitation                                                                                                                                                                             |
| ---------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Ordinary files   | User, shared project, project local, `--settings`, managed; `env` merges by variable.                                                                                                  | Lower files can supply variables absent above. CLI/SDK source selection can omit ordinary files.                                                                                                  |
| Managed files    | Base `managed-settings.json`, then non-hidden `managed-settings.d/*.json` alphabetically.                                                                                              | Base-file existence checks alone miss drop-ins. Never execute `policyHelper`.                                                                                                                     |
| Managed sources  | Ordinary first-wins policy differs from `managedSourcesBehavior: "merge"` added in 2.1.242. Since 2.1.223, managed `env` can fill variables across sources even under first-wins.      | Remote model plus lower managed-file `ANTHROPIC_MODEL` is a required fixture. Credential-paired routing and telemetry groups have special source ownership; do not implement one universal merge. |
| Main model       | `/model` and launch model precede `ANTHROPIC_MODEL`, then saved model.                                                                                                                 | Current matching defaults do not prove prior launch controls. An explicit managed default is not universally a lock.                                                                              |
| Default fallback | `ANTHROPIC_DEFAULT_MODEL` is a fallback from 2.1.236, not equivalent to `ANTHROPIC_MODEL`.                                                                                             | Explicit full saved model plus default fallback should not be blocked as if the fallback always wins.                                                                                             |
| Saved effort     | From 2.1.251, highest source defining applicable model effort or general effort wins; model entry wins within that source.                                                             | Global per-model high versus project general low resolves low. Unrelated local model entry does not displace it.                                                                                  |
| Model keys       | Canonical model matching can include dated IDs, aliases, `[1m]`, and reviewed provider IDs.                                                                                            | Exact string lookup is not the full contract. Unknown aliases remain unknown, not generic family matches.                                                                                         |
| Explicit effort  | `CLAUDE_CODE_EFFORT_LEVEL` precedes flag/frontmatter effort; model holds and caps remain separate.                                                                                     | `CLAUDE_EFFORT` was not established as an official variable. Current saved levels exclude `max`.                                                                                                  |
| Caps             | From 2.1.267, choose applicable per-source cap, then the lowest cap across sources.                                                                                                    | Do not deep-merge as an ordinary winning scalar or silently add a cap instead of editing the default.                                                                                             |
| Paths/lifecycle  | Shared primary CWD and root/main-checkout local rules differ. `/cd` from 2.1.246 can accumulate environment; removing a file entry does not unset an already running process variable. | Test fresh-start policy separately from resumed/running behavior. `_workspace_cwd` is currently ignored locally.                                                                                  |
| Trust            | Safe project variables can apply before trust; other variables wait, with separate noninteractive rules.                                                                               | Untrusted does not mean every project `env` entry is ignored. Antiburn enabled-repository authorization is not vendor launch trust.                                                               |

Implement a bounded native current-file resolver first. Unknown flags, SDK host,
remote state, and accumulated environment limit the effect claim, not every
documented future-default operation under the maintainer's decision. Explicitly
observed conflicting policy must be shown. Do not remove security or routing
controls to make an edit effective.

Attribution owners are `remediation/config.rs::{publication_setting_attribution,
publication_config_attribution_with_home,physical_key}` and
`agent_config/config.rs::EffectiveConfig`. Matching observed and current values
plus a physical hash establishes a matching current control. Rename/relabel or
version that provenance without rewriting historical activity as proven launch
ownership. A future-default edit can be linked to a finding for motivation while
remaining distinct from verified remediation or savings.

#### Pi: release gates closed

The [recursive-merge comparison](https://github.com/badlogic/pi-mono/compare/97f0ccdd96cc207b6ad3630c56eea4d32dbdcf53...v0.84.0)
and tagged source establish `v0.84.0`; `v0.83.0` lacks the introducing commit.
The [native-effort comparison](https://github.com/badlogic/pi-mono/compare/4e69b0c28060f0f02fbe38bfa7c21a2e2eb25057...v0.85.0)
establishes containment in `v0.85.0`, absent in `v0.84.3` and `v0.84.4`.
This is not a promise that every provider emits the field.
[Per-model compaction](https://github.com/badlogic/pi-mono/compare/v0.85.1...46bde88a1cd752966aa2a357d292e83aff98b132)
remains outside `v0.85.1`. Pin tag SHAs in implementation fixtures rather than
depending indefinitely on mutable tag names.

`agent_config/vendors/pi.rs::reasoning_selector` currently selects a project
general fallback before a global per-model entry. Upstream
`deepMergeObjects`/`getModelThinkingLevel` in [settings manager][pi-settings-source]
merge objects first, replace arrays, then look up the exact route. Test global
`modelThinkingLevels[p/m]=high` plus project `defaultThinkingLevel=low`: high wins.
Provider/model are independently merged leaves. The local `effective_model_source`
requires both in one file; keep that as an explicit narrow-edit limitation until
a reviewed single-file override or multi-target operation exists.

`global_root_for` already supports `PI_AGENT_DIR` and `PI_CODING_AGENT_DIR` and
rejects conflicting values. Do not duplicate or undo that support. Native trust
order includes run override, resource-free shortcut, extension, nearest saved
ancestor, global fallback, then UI. A first `.pi/settings.json` changes the
resource-free case. Never create trust to activate it. SDK managers and resumed
thinking have separate startup rules; promise fresh-default changes only.

#### Codex: use separate operation contracts

The [0.134.0 release](https://github.com/openai/codex/releases/tag/rust-v0.134.0)
and [advanced config][cx-advanced] establish separate profile files. The current
local `agent_config/vendors/codex.rs::reject_active_profile` detects legacy
`profile`, not launch-selected new profiles. Add a fixture with a selected
profile and conflicting legacy table: upstream rejects, not silently merges.

Pinned [loader][cx-loader] includes legacy managed/MDM layers above ordinary
and session configuration, with requirements separately composed. Do not state
that CLI universally wins. Use the exact `PROJECT_LOCAL_CONFIG_DENYLIST` when
characterizing allowed project controls.

Pinned [skills resolver][cx-skill-rules] uses ordered User and SessionFlags rules;
selected profile counts as User. Project rules do not control this setting.
Test user path-disable, profile name-enable, ignored project rule, then session
path-disable. Use canonical `SKILL.md`, not folder equivalence. Path resolution
can fall back to the absolute path when canonicalization fails; that is not proof
that the document is safely editable.

`AgentRoleOverrides` in [role source][cx-role] is bounded. It is not a generic
config overlay. [Spawn][cx-spawn] applies requested/default model and effort,
role projection, then root tier. Root absence also clears a role tier. A
model-only role can preserve effort and fail known-model validation; unknown
model metadata is a separate branch. Do not expand role edits to MCP, sandbox,
or compaction based only on broad docs. These current role/skill semantics remain
pinned-shape gates, not all behavior of release 0.134.0.

#### OpenCode: additional input and release limit

The [release comparison](https://github.com/anomalyco/opencode/compare/193de13a88d62a6409c6d385831180f1def527dc...v1.18.30)
is diverged: the research pin is not contained in `v1.18.30`. Do not ship every
pinned field under that release label.

Beyond Section 8, [loader][oc-loader] reads an extensionless legacy global
`config`, dynamically imports it as TOML, and migrates it after normal global
files. Local `agent_config/vendors/opencode.rs::reject_directory_overrides` does
not detect this source. Detect its presence without invoking migration. Also
characterize `ConfigV2Compat.lower`; valid JSON alone does not identify V1
semantics. Test the documented enablement-only MCP overlay and last-rule
whole-tool denial directly against pure synthetic inputs.

### 22.5 Route, cache, report, and verification contracts

#### Route normalization

Pinned sources: [provider loader](https://github.com/anomalyco/opencode/blob/193de13a88d62a6409c6d385831180f1def527dc/packages/opencode/src/provider/provider.ts),
[translation][oc-transform], [request][oc-request],
[usage normalization](https://github.com/anomalyco/opencode/blob/193de13a88d62a6409c6d385831180f1def527dc/packages/opencode/src/session/session.ts#L338-L404),
and [native V1 assistant schema](https://github.com/anomalyco/opencode/blob/193de13a88d62a6409c6d385831180f1def527dc/packages/schema/src/v1/session.ts#L453-L485).

| Proven producer mapping                                                                   | Permitted use                       | Not proved                                                             |
| ----------------------------------------------------------------------------------------- | ----------------------------------- | ---------------------------------------------------------------------- |
| `@ai-sdk/openai` uses `openai` option namespace; direct OpenAI loader calls `responses`   | Narrow reviewed route inference     | Every custom provider named `openai` uses the same endpoint/accounting |
| Anthropic and Vertex-Anthropic SDKs use `anthropic` options                               | Option translation                  | Equal billing, account, or persisted API identity                      |
| Google/Vertex use `google`/`vertex`; Bedrock uses `bedrock`; Bedrock Mantle uses `openai` | Preserve actual loader/SDK identity | Namespace `openai` means direct OpenAI billing                         |
| Azure emits options under both `openai` and `azure` and selects among multiple APIs       | Route-specific contract only        | One fixed Azure API                                                    |
| Copilot uses endpoint metadata/model selection and SDK fallback                           | Keep its own route contract         | A universal Responses alias                                            |

`model.api.id` is the upstream model ID, not Antiburn's API discriminator. The
reviewed native V1 schema does not establish persisted `messages`,
`anthropic-messages`, `responses`, or `openai-responses` alias fields. Do not add
these from Pi conventions. Native `vendors/opencode.rs` copies provider but leaves
API absent. Local `model_catalog.rs::model_control` can infer an API while durable
rows in `analysis/rows.rs` retain `None`.

This causes a concrete end-to-end verification mismatch: a watch stores inferred
API, but `insights_report/findings.rs::model_attribution_matches` compares it with
the raw absent API. Existing routed tests manually update turn API and bypass
the native path. Add native JSONL and SQLite publication-to-verification tests
without SQL repair. Normalize the accepted omitted-route contract consistently
at both boundaries; preserve unknown explicit routes as unknown.

`analysis/evidence_query.rs::{request_accounting,request_pair_is_compatible}`
currently rejects every explicit OpenCode API and compares routes separately.
Fixing dispatch alone does not fix pair equivalence. The explicit-alias issue is
a normalized-input characterization task until a native producer shape is proved.
Do not make an invented alias table a Phase 1 prerequisite.

#### Accounting and retained heuristic

Fetched official rolling sources on 2026-09-11:
[Anthropic caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching),
[OpenAI caching](https://developers.openai.com/api/docs/guides/prompt-caching),
and [OpenAI pricing](https://developers.openai.com/api/docs/pricing).
Record dated model/route price entries when implementing; these pages are not
historical account-specific invoices.

| Accounting  | Source meaning                                                                                       | Required handling                                                                                                                                        |
| ----------- | ---------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Anthropic   | Ordinary input excludes cache creation and reads. Total input is the sum of all three.               | Five-minute writes commonly cost 1.25x and one-hour writes 2x. Read multipliers are model-specific; current docs include exceptions to 0.1x.             |
| OpenAI      | Raw input includes cached and separately reported cache-write tokens. Ordinary input subtracts both. | Current GPT-5.6+ docs describe paid writes and 30-minute default/minimum TTL; older models differ. Provider-family-only `UncachedInput` is insufficient. |
| OpenCode V1 | Producer already separates ordinary input, read, write, visible output, and reasoning.               | Do not subtract cache twice. Add reasoning to visible output once for total output.                                                                      |
| Pi          | `cacheWrite1h` is part of `cacheWrite`; reasoning is part of output.                                 | Do not add subclasses twice; route adapters can zero-fill absent counters.                                                                               |

Local C selects the first accounting family and skips others. Replace that with
independent compatible segments keyed by existing thread, route, model, accounting,
and compaction/link boundaries. Both family orders must produce the same scoped
results. Unsupported segments prevent session-wide clean, not supported findings.
Do not join nonadjacent requests merely because their route eventually matches.

Current pair metric is:

```text
depth = ordinary input + cache read + cache write
growth = max(current depth - previous depth, 0)
repeated = max(selected paid bucket - growth, 0)
P = sum(selected paid bucket over compatible successor requests)
R = sum(repeated)
multiple = P / (P - R)
```

Keep thresholds `2.35` for the existing cache-write policy and `2.0` for the
existing uncached-input policy initially. There is currently no extra sample
floor. Retain that behavior explicitly for the first fix; report request/pair
counts and flag short observations as heuristic. For newly supported accounting,
select by proved bucket semantics, not a premium-looking model name. A best-guess
threshold can reuse the applicable accounting policy, labeled as such, without
inventing a new native capability or claiming calibration.

Two writes of 10,000 then 11,000 input tokens produce a current pair ratio of 11.
Including the cold baseline would instead produce `21,000 / 11,000 = 1.909`.
At 1.25x write and 0.1x read rates, an assumed perfectly reusable 10,000-token
prefix produces observed input cost 26,250 versus counterfactual 14,750 in base
input-price units. That is neither 11,500 fewer token events nor proven savings.
Two equal 10,000-token writes yield an infinite current ratio but a baseline-
inclusive ratio of 2. These examples are required regressions and explain why
changing the denominator while retaining thresholds is a policy change.

Token growth does not prove repeated content or the cause of a cache miss. Keep
TTL/key changes cause-specific. Do not recommend a static shared cache key.
Known zero/negative price differences remain known; unavailable rate classes
produce an unavailable exact price or an explicitly labeled approximation.

#### Remaining report defects

Engine owner: `crates/antiburn-local/src/insights/report.rs`. Shell owner:
`apps/desktop/src-tauri/src/insights_report.rs`.

| Symbol/path                                                        | Remaining static issue                                                                                        | Regression/contract                                                                                                                                       |
| ------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Shell `token_burn_evidence` path                                   | Unknown-model rows can be skipped before known tokens enter the total.                                        | Known usage stays in denominator even when model-specific numerator is unknown.                                                                           |
| `TokenBurnAccumulator::observe`, `TokenBurnEvidence::from_session` | Missing totals can leave `total_complete` true; partial evidence is treated as total.                         | Missing actual usage prevents an unqualified percentage. A scoped subtotal must be labeled.                                                               |
| `estimated_replacement_savings_tokens`                             | Price fraction is converted to tokens; zero/negative differences disappear; tiny positives are forced to one. | Separate signed USD from literal tokens. Equal rates mean zero, not unknown.                                                                              |
| Effort comparison accumulator                                      | Similar context size is treated as comparable task; only shorter lower-effort outputs contribute.             | Best guesses must be explicit counterfactual methods, not causal/verified savings. No retained comparator means unavailable, not a fabricated percentage. |
| S report path                                                      | A session S finding authorizes pricing unrelated premium workers against hard-coded family alternatives.      | Use the detector's actual qualifying parent/worker relationship and reviewed replacement route.                                                           |
| D/resource report paths                                            | Excess cache tokens proxy removable depth; context size proxies repeated resource exposure.                   | Keep observations separate from assumed removals. Context size alone does not prove full schema reinjection.                                              |
| Combined accumulator                                               | Max/sum combinations are not an exact union of saved activity.                                                | Do not present unexplained maxima as total savings. Display method-specific estimates until overlap is defined.                                           |

The earlier work removed fixed 20%/35%/10% effort fallback estimates and bounded
retained comparisons at 4,096. It did not resolve the above issues. Check the
nearby accumulator comment for obsolete fallback wording during implementation.

#### Exact source and one owner per activity

| Owner                                                                      | Required work                                                                                                                                             |
| -------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Shell `insights_report/findings.rs::query_old_model_verification_evidence` | Filter the watch's exact `SourceFormat` before limits/aggregation. Physical matching already exists and must remain.                                      |
| `remediation/target.rs`, `remediation/config.rs::physical_key`             | Keep finding identity separate from environment, physical file, selector, and resolver-contract identity.                                                 |
| Engine `remediation/verification.rs`                                       | Pure verification lacks source/physical fields; either pass already qualified evidence with a tested shell boundary or extend the minimal typed contract. |
| Shell `remediation/watch.rs`                                               | `allocation:v1:<remediation_id>` deduplicates one attempt, not one observation across attempts.                                                           |
| `store/remediation.rs` and schema                                          | Add transactional shared-activity allocation or deterministic nonoverlapping control intervals; do not let evaluation order pick the winner.              |

Required invariant: one replacement observation contributes to at most one
additive old-model allocation. A later activation of the same physical control
supersedes its prior interval; ambiguous overlapping attempts yield no additive
credit. Bind environment, accepted source/route, scope, control, boundary, and
stable activity identity. Source generation/replay fence versions evidence; it
must not create a new billable observation. Retention cannot release previously
credited activity to another attempt.

Tests: `old-A -> C` and `old-B -> C` on the same control; two physical files with
the same model; JSONL versus SQLite source; equal timestamps at recurrence;
restart; reordered evaluation; correction moving the activation/recurrence
boundary; missing retained evidence versus explicitly invalidated old proof.

Existing correction logic preserves terminal proof when evidence is merely
missing and revokes it when corrected positive evidence proves unresolved state.
Preserve that distinction. `reconcile_remediations` dirties watching/fixed but
not every recurred contribution. A method-constant bump alone cannot invalidate
all old retained credit. Correct shared allocations atomically and reject stale
dirty revisions. Never silently reprice historical rates with today's catalog.

### 22.6 Passive evidence: concrete admissions and stop points

#### OpenCode CoreV2

Add these primary pin paths to Section 8's sources:
[projector](https://github.com/anomalyco/opencode/blob/193de13a88d62a6409c6d385831180f1def527dc/packages/core/src/session/projector.ts),
[updater](https://github.com/anomalyco/opencode/blob/193de13a88d62a6409c6d385831180f1def527dc/packages/core/src/session/message-updater.ts),
[publisher](https://github.com/anomalyco/opencode/blob/193de13a88d62a6409c6d385831180f1def527dc/packages/core/src/session/runner/publish-llm-event.ts),
[runner](https://github.com/anomalyco/opencode/blob/193de13a88d62a6409c6d385831180f1def527dc/packages/core/src/session/runner/llm.ts),
[event SQL](https://github.com/anomalyco/opencode/blob/193de13a88d62a6409c6d385831180f1def527dc/packages/core/src/event/sql.ts),
and [event persistence](https://github.com/anomalyco/opencode/blob/193de13a88d62a6409c6d385831180f1def527dc/packages/core/src/event.ts).

`session_message` stores `id`, `session_id`, `type`, `seq`, timestamps, and JSON
`data` excluding the outer ID/type. `(session_id, seq)` is unique. Variants are
`agent-switched`, `model-switched`, `user`, `synthetic`, `system`, `shell`,
`assistant`, and `compaction`. Assistant model is `{id, providerID, variant?}`;
tokens/cost/finish/error are optional; tool content retains call identity/state.

Important corrections to possible readings of Section 8:

- Sequence gaps are normal: message sequences come from the broader durable
  event stream, whose other events mutate rows. A gap alone is not missing history.
- Reverts delete rows. Count/max sequence/timestamp is not a sufficient fingerprint.
- Missing/nonfinite usage can be zero-filled by the publisher. Zero counters do
  not prove measured zero usage or complete clean-result evidence.
- A new assistant can complete an older unfinished record. Completion time alone
  is not settled successful usage.
- Saved model is producer-resolved request model, not universal served-model
  proof. The reviewed settlement path emits `cost: 0`; do not treat it as a bill.

Start separate source characterization with positive request facts only. O needs
an accepted model-provenance rule; C and clean need completeness. Test sparse
sequences, same-row mutation, revert deletion, zero-filled usage, stale completion,
pending tools, fork/dual-schema ownership, and output/reasoning separation.
Existing event tables are a reviewed passive alternative for correction proof,
not automatically a full request archive. Epochs/listings still do not prove M/B/K.

#### Pi persisted metadata

[Session manager](https://github.com/badlogic/pi-mono/blob/d12cd92e45e308d4af000554292165ef1984253b/packages/coding-agent/src/core/session-manager.ts)
serializes message objects under version-3 framing with entry ID, parent ID,
timestamp, and nested message. Optional response model/ID, diagnostics, effort,
and usage fields therefore have a real saved path. Also inspect
[Completions producer](https://github.com/badlogic/pi-mono/blob/d12cd92e45e308d4af000554292165ef1984253b/packages/ai/src/api/openai-completions.ts).

Completions keeps requested `model` and adds a differing `responseModel`;
Anthropic assigns the response model to `model` itself. A generic requested-model
rule is wrong. Anthropic native effort is assigned before `onPayload`, not final
wire proof. `compaction` and `branch_summary` usage, including `fromHook`, are
separate from main-loop usage. Native `<skill name=... location=...>` text has no
proved unforgeable provenance marker; identical user text remains possible.

Proceed with provider-specific synthetic metadata/accounting tests, missing versus
zero counters, summary isolation, response-model differences, payload mutation,
and TTL subsets. Preserve selected-policy T and no unused-skill K claim.

#### Codex physical history

Additional primary sources:
[rollout envelope](https://github.com/openai/codex/blob/fc948f8c473e5d11e780ffcf1fd7f812a2020932/codex-rs/history/src/rollout_payload.rs),
[history types](https://github.com/openai/codex/blob/fc948f8c473e5d11e780ffcf1fd7f812a2020932/codex-rs/history/src/lib.rs),
and [recorder](https://github.com/openai/codex/blob/fc948f8c473e5d11e780ffcf1fd7f812a2020932/codex-rs/rollout/src/recorder.rs).

Lines contain timestamp, optional ordinal, snake-case type, and payload.
Response-item harness metadata is a sibling of payload. `TokenUsageRecord`
contains thread/turn/session/root-turn/response IDs and request/turn/thread usage,
not model/provider/effort/tier. Join those explicitly. `turn_context` is once per
real user turn and relevant compaction, not every request.

Legacy and paginated contracts differ. Account for `history_base`,
`forked_from_ordinal_exclusive`, `subagent_history_start_ordinal`, physical rollout
ownership, and compressed framing. Either implement compression under bounds or
decline it explicitly. Compaction's copied latest usage is not new usage.

World state uses RFC 7386: null deletes members, arrays replace, `full` starts a
baseline. Test patch-before-baseline, inherited history, missing references,
duplicate response IDs, completed/raw-item overlap, aggregate dedupe, and mismatched
replacement-history metadata lengths. Deferred tool descriptions are shortened
to 250 characters and rendering has a 4 KiB budget. Names are not full definitions.
Selected-skill authority/package/resource can be remote, not a local edit path.

#### Cursor and Antigravity: useful work without invented support

| Surface                   | Evidence found                                                                                                                                                                                                                                                   | Next work and remaining boundary                                                                                                                         |
| ------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Cursor CLI                | txcript 0.8.0 documents observed 2026.06.26 macOS metadata-v1 `store.db`, `blobs(id,data)`, `meta(key,value)`, encoded meta key `0`, model selection, and tool IDs.                                                                                              | Preserve native identities and field provenance. Decoder row-order, fallback timestamps, and synthetic IDs are not producer guarantees.                  |
| Cursor IDE                | Pinned parser shows `composerData`, bubble IDs, ordered headers, and model selection propagation. vct-core decodes binary context gauges.                                                                                                                        | Actual response model/request input still need primary proof. Do not import estimated cache tokens from gauges. Existing eligible O prompts can proceed. |
| Cursor SDK                | First-party [`@cursor/sdk@1.0.31`](https://registry.npmjs.org/@cursor/sdk/1.0.31) and [store interfaces](https://unpkg.com/@cursor/sdk@1.0.31/dist/esm/store/local-agent-store.d.ts) describe persisted run IDs, selected model, usage, events, and checkpoints. | Candidate separate existing-store contract, not proof CLI/IDE stores share its schema. No SDK execution for evidence.                                    |
| Antigravity CLI           | [Pinned official changelog](https://github.com/google-antigravity/antigravity-cli/blob/34406bef8e87fc103783c0c9715e5e2cce3c3e1b/CHANGELOG.md) confirms features but repository does not supply the CLI producer. It includes 1.2.0.                              | Section 13's table is a reviewed subset, not a latest-version claim. No database descriptor or 1.1.28 request-log grammar was established.               |
| Antigravity native stores | txcript/RimZ identify `steps`, `gen_metadata`, parent tables, and transcript step/call fields.                                                                                                                                                                   | Exact structural synthetic fixtures can proceed. Need immutable binary/descriptor provenance and semantic request ownership before new S/C.              |
| Antigravity SDK           | [Persistence example](https://github.com/google-antigravity/antigravity-sdk-python/blob/52ea99480960ed02be1561f6fe57b99e7186962a/examples/getting_started/persistence.py) passes `save_dir`; local connection forwards storage directory to Go harness.          | Persistence exists. Missing contract is saved serializer/schema, not the existence of persistence.                                                       |

Antigravity decoder [PR 713][ag-usage] is merged, with decoder revision
`e9af34fbd805674df0181dfaa2c6820da0b2b71d`. Candidate generation paths are
`#1.#19` response model, `#1.#4.#11` response ID, `#1.#4.#5` cache read,
`#1.#4.#9/#10` visible output/thinking, and `#1.#4.#1 + #1.#4.#2` billable input.
These remain decoder interpretations; disputed input/cache meanings are not
resolved by agreement between readers. [PR 737](https://github.com/junhoyeo/tokscale/pull/737)
adds a timestamp lead at `#1.#9.#4`. Do not substitute session creation time for
detector-grade timed usage. The SDK's
[runtime protobuf](https://github.com/google-antigravity/antigravity-sdk-python/blob/52ea99480960ed02be1561f6fe57b99e7186962a/google/antigravity/proto/localharness.proto)
has a different named usage layout and does not validate these CLI fields.

Reviewed passive alternatives now include CoreV2 events, Pi provider/native
persistence, Codex referenced history, Cursor bubbles/graphs/request-context/SDK
stores, and Antigravity transcripts/generations/parent tables/logs/existing SDK
roots. No new collector, binary execution, dashboard RPC, or temporary-directory
scan is authorized. Public evidence does not establish every private product
contract. Keep exact missing proof and native guidance as the completion outcome
for those research gates, not an endless prerequisite for other agents' editing.

### 22.7 Remaining UI, scheduling, and platform work

| Owner                                                                                                                                     | Concrete work/test                                                                                                                                                                                                                                                                                        |
| ----------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Engine `remediation/prompts.rs::{remediation_prompt,MAX_PROMPT_BYTES}`; shell `RemediationController::copy_prompt_fix_burn_check_targets` | Current batch concatenates full templates and enrolls one watch at a time. Build shared instructions, bound real templates, return included/omitted counts, and make all enrollment/snapshot/join writes transactional. Inject failure on the second write; leave no partial new or reused-watch changes. |
| Shell single-prompt controller                                                                                                            | Watch creation precedes final bounded reference suffix. Test the complete final text before persistence.                                                                                                                                                                                                  |
| `remediation/mod.rs::{start_watch,watch_verification_available}`; `Store::create_or_reuse_remediation`                                    | Permanently unavailable prompts can remain `Watching` and consume the 1,000-row non-recurred capacity. Separate bounded history from useful verification. Test full unavailable history followed by eligible auto edit.                                                                                   |
| `BurnCheckTargetActions.tsx`, `BurnCheckDetail.tsx`                                                                                       | Three-second success reset retains prepared text but allows repeated copy analytics. Deduplicate measurement by prepared text, not temporary success state.                                                                                                                                               |
| Target `prepare` handler                                                                                                                  | Review-ready measurement precedes stale-result rejection. Emit ready only when the review is accepted for display.                                                                                                                                                                                        |
| `BurnCheckReviewDialog.tsx`, `display_config_file`                                                                                        | Show the non-blocking editor notice and latest backup effect. Existing local path display contradicts `docs/remediation.md` wording; reconcile docs with reviewed local target/backup display. No path telemetry.                                                                                         |
| Shared action cache                                                                                                                       | Test two expanded checks exceeding the shared target-handle limit. Reuse stable targets or expose expiry/refresh; do not silently lose visible actions.                                                                                                                                                   |

Use `BurnChecksView.test.tsx`, engine prompt tests, remediation controller tests,
and `store/tests/remediation_tests.rs`. Include clipboard failure/retry, repeated
copy after timeout, stale review, real-template overflow, transaction rollback,
capacity, and restart. Follow analytics measurement docs before instrumentation.
Do not add React effects. Local path visibility is for choosing the file and
backup, not permission to log private paths.

#### Windows and WSL implementation information

Native Windows remains attribution-only today. Broader editing is a goal, not an
implemented capability. Use the same practical backup contract with native APIs
and tests rather than waiting for absolute power-loss/race guarantees.

- [`CreateFileW`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew)
  provides exclusive `CREATE_NEW` and handle-lifetime sharing modes. Final-component
  reparse flags do not by themselves prove every ancestor was not reparsed.
- [`NtCreateFile`](https://learn.microsoft.com/en-us/windows/win32/api/winternl/nf-winternl-ntcreatefile)
  supports relative directory handles; `OBJ_DONT_REPARSE` in
  [`OBJECT_ATTRIBUTES`](https://learn.microsoft.com/en-us/windows/win32/api/ntdef/ns-ntdef-_object_attributes)
  can reject encountered reparses. This is an implementation candidate, not a
  requirement to introduce low-level bindings if a simpler adequate path exists.
- [`FILE_ID_INFO`](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_id_info)
  supplies volume/file identity. Modification time is not identity.
- [`ReplaceFileW`](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-replacefilew)
  preserves listed metadata/security/streams but has documented partial failures;
  `REPLACEFILE_WRITE_THROUGH` is unsupported. Handle-based
  [`FILE_RENAME_INFO`](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_rename_info)
  can reject existing destinations, but has no expected-destination CAS.
- [`GetSecurityInfo`](https://learn.microsoft.com/en-us/windows/win32/api/aclapi/nf-aclapi-getsecurityinfo)
  reads security through a handle; backup security must not be broader.
  [`LockFileEx`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-lockfileex)
  does not stop mapped writes. Holding a target without delete sharing may block
  our own replacement. Test rather than assuming a lock solves everything.

Test native ACL preservation, backup access, reparse/junction rejection, sharing
violations, replacement partial errors, and process-crash recovery. Do not enable
Windows by removing the OS gate alone.

WSL discovery currently uses distribution identity and can infer a default user
in `crates/antiburn-local/src/platform/environment.rs`. That is not write
authorization. [WSL permissions](https://learn.microsoft.com/en-us/windows/wsl/file-permissions)
and [filesystem interoperability](https://learn.microsoft.com/en-us/windows/wsl/filesystems)
show that UNC access uses the default user and DrvFS may synthesize permissions
from Windows ACLs. Host UNC, `/mnt/c`, and Linux-native storage are distinct.
Bind distribution, intended user/home, root, and mount for writes. Do not edit
host config for WSL evidence. A Linux-side writer could be a later explicit
architecture decision; this research did not approve deploying/invoking one.
Keep WSL writes unavailable until a real platform path is tested.

### 22.8 Performance, revisions, and acceptance

Existing engine harnesses are `benches/pipeline_baseline.rs` (Criterion timing),
`benches/memory_baseline.rs` (counting allocator), and
`tests/support/corpus.rs` (`SessionSpec` and generators). The memory harness also
runs 500 MiB identity cases; do not mistake it for a cheap unit test.
`benches/BASELINE.md` records historical results and explicitly lacks desktop
discovery, queue, persistence, report, and IPC measurements. Existing numbers are
not current hardware budgets.

Run separately from `crates/antiburn-local` during a later measurement task:

```sh
cargo bench --bench pipeline_baseline
cargo bench --bench memory_baseline
```

Record commit, toolchain, OS, CPU/RAM, optimized build, fixture seed, warm/cold
definition, and missing instruments. Extend to Section 17's 10k/100k/1m-record,
append/rewrite/old-session/worker-tree matrix before claiming it covered. Measure
RSS separately from allocator high-water and retained state. Include queue,
SQLite sorting/rows, reports, verification, cancellation, bytes read/copied, and
full-versus-incremental correctness. A benchmark run on available hardware can
establish a local baseline; it cannot assert all supported hardware meets it.
Do not block correctness work on nonexistent numeric budgets.

`docs/runbooks/memory-reporting.md` covers isolated synthetic popover/WebContent
measurement, not the full scan/watch pipeline. It needs macOS GUI/Accessibility
setup. Do not run real-history measurements or add runtime instrumentation just
to satisfy this research handoff.

Re-read revision constants before editing; concurrent work can change them.

| Change                               | Invalidation to review                                                                                                         |
| ------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------ |
| Native route/token parsing           | Parser revision and durable-turn reparse                                                                                       |
| Segment/accounting derivation        | Analyzer revision; evidence schema if stored shapes change                                                                     |
| Resume state                         | Resume snapshot revision and rejection of incompatible snapshots                                                               |
| Capability/eligibility               | Coverage revision, detector/report policy, and both coverage baselines                                                         |
| Report units/denominators/heuristics | Report computation/catalog and applicable savings-method revision                                                              |
| Resolver/current-default provenance  | Resolver/remediation policy and stored attribution compatibility                                                               |
| Verification/allocation              | Verification method, savings method, transactional schema migration if needed, and explicit retained-contribution invalidation |

Do not reserve next revision numbers in this plan. Old recurred contributions
need explicit handling; startup dirtying alone is insufficient. Preserve saved
price snapshots when intentional and distinguish unavailable retained evidence
from proof invalidated by a corrected method.

For each implementation batch, run Section 19's relevant checks and focused new
regressions. Coverage edits need `check_coverage_contract`; plan research does
not grant dated source acceptance. Run analytics/design/frontend checks when
those surfaces change. This plan-only research requires Markdown formatting and
diff checks, not builds, tests, or `aislop` scans of unrelated code changes.

Remaining non-research dependencies are explicit: implementation and synthetic
characterization, actual performance measurements, native Windows/WSL testing,
and source acceptance for privately produced formats. None is honestly replaced
by a guessed contract. Agents with documented config controls can continue under
the approved practical write policy without waiting for those independent gates.
