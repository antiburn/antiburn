# Burn Check Remediation

Implementation status: complete as of 2026-09-10. Real-machine validation is a
release check.

This guide describes the current implementation. See
[`check-coverage.md`](check-coverage.md) for evidence limits and
[`session-coverage.md`](session-coverage.md) for source parsing.

## Architecture

The engine and desktop shell have separate responsibilities.

| Module                                                  | Responsibility                                                                                         |
| ------------------------------------------------------- | ------------------------------------------------------------------------------------------------------ |
| `crates/antiburn-local/src/remediation/findings.rs`     | Build typed findings, canonical identities, and safe display facts.                                    |
| `crates/antiburn-local/src/remediation/prompts.rs`      | Build bounded prompts and enforce the prompt support matrix.                                           |
| `crates/antiburn-local/src/remediation/verification.rs` | Apply pure verification rules to later evidence.                                                       |
| `crates/antiburn-local/src/remediation/estimates.rs`    | Calculate all nine typed estimate methods.                                                             |
| `apps/desktop/src-tauri/src/remediation/`               | Group current findings, issue IDs, join actions, run watches, recover writes, and expose safe results. |
| `apps/desktop/src-tauri/src/remediation/vendors/`       | Define each agent's source, attribution, override, and action policy.                                  |
| `apps/desktop/src-tauri/src/agent_config/`              | Resolve effective settings and prepare or apply one exact file edit.                                   |
| `apps/desktop/src-tauri/src/agent_config/vendors/`      | Define each agent's files, precedence, selectors, parsing, and edits.                                  |
| `apps/desktop/src-tauri/src/store/remediation.rs`       | Store durable attempts, snapshots, action joins, and contributions.                                    |
| `apps/desktop/src/views/main-window/`                   | Load reports and targets only while the Burn checks section is active and visible.                     |

The engine is the functional core. It has no desktop file access. The shell is
the imperative boundary. It owns local storage, trusted roots, file edits, IPC,
and analytics.

## Data Flow

The flow is check to finding to target to action:

1. A `SessionReader` parses an accepted local source into normalized evidence.
2. A detector returns a typed `Finding` only when its evidence contract allows it.
3. The desktop reads current findings and groups equal canonical targets.
4. The desktop returns safe display facts for every visible target.
5. The target reports prompt and Auto Fix availability separately.
6. A target prompt action starts or joins a durable attempt. Non-named checks can prepare one bounded prompt for their selected current targets.
7. An Auto Fix action prepares one edit, waits for confirmation, and then starts or joins an attempt.
8. Later winning evidence dirties matching attempts.
9. The evidence worker verifies each dirty attempt and stores one contribution when proof exists.

When the report still has failed evidence but the bounded current-finding query
returns no exact targets, a separate check-level action re-runs both queries. It
returns a generic inspection prompt only while that mismatch remains. It does
not create a target, action ID, Auto Fix operation, or verification watch.

Target listing does not require prompt support. A finding stays visible when no
safe action exists. The UI explains the unavailable action.

## IDs

Four IDs have different jobs and lifetimes.

| ID                    | Lifetime                         | Use                                                                                                       |
| --------------------- | -------------------------------- | --------------------------------------------------------------------------------------------------------- |
| Finding ID            | Durable for the canonical target | Reconcile one visible target and key saved display facts.                                                 |
| Attempt ID            | Durable across restart           | Track one passive or action-started verification lifecycle. It forms the `ABR-<opaque>` prompt reference. |
| Action ID             | Ten minutes, in memory           | Authorize prompt preparation or Auto Fix review for current evidence.                                     |
| Prepared-operation ID | Ten minutes, in memory           | Authorize one reviewed apply operation.                                                                   |

The IDs are distinct. An action ID is not a stable UI key. A prepared-operation
ID cannot select another edit. A prompt reference does not prove completion and
never enters analytics.

## Scope And Precedence

The editor changes an existing effective project or global setting. It does not
create a config file. A project value wins only when the agent's exact
precedence rule selects it. Otherwise, the effective global value wins.

| Agent       | Project resolution                                                                                                                                     | Global resolution                                                                                                    |
| ----------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------- |
| Claude Code | `.claude/settings.local.json`, then `.claude/settings.json`; the selected file must contain the setting                                                | `~/.claude/settings.json`                                                                                            |
| Codex       | `<root>/.codex/config.toml`; the setting must exist; the global config must mark the exact root as trusted                                             | `~/.codex/config.toml`                                                                                               |
| OpenCode    | Merge `opencode.json` then `opencode.jsonc` from the trusted root through the actual cwd; then merge `.opencode` directories from cwd back to the root | Merge `config.json`, `opencode.json`, and `opencode.jsonc` under the effective config root; then merge `~/.opencode` |
| Pi          | `<cwd>/.pi/settings.json` when it contains both `defaultProvider` and `defaultModel`; reasoning can use the project model map or default level         | The effective Pi agent directory `settings.json`                                                                     |

OpenCode uses the last matching model value in its reviewed merge order. Pi
rejects a split provider and model route. Codex rejects an active profile and a
nested cwd. Claude reasoning prefers a per-model `modelSettings` value, then a
top-level `effortLevel`.

The trusted root and actual cwd are separate facts. The trusted root comes from
the enabled, accessible repository inventory. It sets the file safety boundary.
The actual cwd comes from the session. It selects cwd-sensitive project config.
OpenCode and Pi allow a cwd below the trusted root. Codex requires the cwd to
equal the trusted root. Recovery stores only the cwd relative to the trusted
root and reconstructs it inside that root.

Global findings use a hashed physical target as their scope key. Project
findings use the hashed trusted workspace. Session and worker targets stay
session-scoped or worker-scoped when no safe durable control exists.

## Passive And Action Attempts

Each winning `Ready` publication can enroll exact passive attempts only for T,
O, and F targets with supported positive verification. Fair bounded selection
gives each detector with eligible findings an opportunity. Enrollment uses the
publication time in milliseconds as the immutable boundary. Schema V45 does not
backfill older publications. A replay reuses an active target and does not move
its boundary. Session-scoped and other permanently unverifiable findings do not
enroll and do not consume the active attempt bound.

A later user action can join an active passive attempt. The attempt keeps its
passive origin and original boundary. The store records the first action join
time separately. This design measures passive discovery and deliberate use
without claiming that the action caused the result.

After recurrence, a later publication can create a new attempt. The new attempt
gets a new durable ID and prompt reference.

## Prompt Fallback

Prompt support is independent from Auto Fix support. The UI offers `Copy fix
prompt` when the engine can build a safe prompt. This action remains available
when Auto Fix lacks an attributed setting, supported editor, safe platform, or
trusted target.

An exact-target prompt includes the finding, safe facts, source limits,
requested result, and required evidence. A check-level fallback names the failed
check and practical objective. It asks the coding agent to inspect
representative session evidence and the effective project and user
configuration before it proposes a change. It does not claim an exact model,
setting, scope, config file, or replacement that current target evidence cannot
prove.

Both prompt operations can add up to three absolute paths resolved from the
local session index. The prompt quotes and labels them as representative session
evidence, not configuration edit targets. Non-file, relative, unresolved, duplicate,
or oversized path candidates are omitted. Paths appear only in prompt text
returned after the explicit copy action. They do not enter target or sample
payloads, rendered details, analytics, logs, snapshots, finding IDs, action IDs,
or watches.

Every prompt contains at most 8 KiB. Exact-target facts contain at most eight
sanitized identities. Prompts exclude session IDs, transcript content, config
content, credentials, and unrelated history. A private or truncated essential
identity makes an exact-target prompt unavailable. Clipboard success is separate
from prompt preparation. A failed clipboard write can retry the same prepared
text without a second backend operation.

## Auto Fix Safety

Auto Fix uses two explicit steps.

1. `prepare_auto_fix_burn_check_target` revalidates the evidence and effective setting. It prepares exact replacement bytes and returns a bounded semantic review.
2. `apply_prepared_burn_check_operation` accepts only the prepared-operation ID. It revalidates evidence, scope, precedence, selector, file identity, and original bytes before replacement.

The review names the agent, scope, setting, current value, proposed value,
effect, and side effect. It does not expose a path or raw config.

The editor rejects missing or ambiguous targets, dynamic values, runtime
overrides, managed config, unsupported profiles, malformed or duplicate data,
files above 256 KiB, non-regular files, symlinks in the target path, unsafe
roots, and unsupported Unix ownership. It preserves unrelated values. TOML
keeps formatting where `toml_edit` can preserve it. JSON uses formatted output.

Apply writes an exclusive temporary file in the same directory. It preserves
permissions and Unix owner and group. It syncs the file, checks the original
identity and bytes, atomically replaces the target, syncs the directory, reads
the file again, and verifies the typed setting. A conflict never retargets or
rebuilds the edit.

A failure before replacement cancels the reservation. A result that can follow
replacement enters durable `recoveryNeeded`. Recovery resolves the same agent,
source, setting, scope, trusted root, relative cwd, physical selector, and
replacement value. It starts verification only when readback proves the change.
An old value cancels the uncertain write. A changed or unprovable target remains
blocked and cannot start a second write.

Repeated confirmation returns the saved result only for the same completed
prepared operation. Expired, evicted, or failed operations cannot be reused.

## Support Matrix

The check codes are D session overdepth, T model overthinking, S overpowered
subagents, M unused MCP servers, B unused built-in tools, K unused skills, O old
model usage, F fast mode overuse, and C cache churn.

### Finding And Prompt

Prompt support matches implemented finding shapes for the five remediation
agents. Source coverage can still block a finding for one session.

| Agent and accepted source                                                                                 | D   | T   | S   | M   | B   | K   | O   | F   | C   |
| --------------------------------------------------------------------------------------------------------- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Claude Code, `ClaudeJsonl`                                                                                | Yes | Yes | Yes | Yes | Yes | Yes | Yes | Yes | Yes |
| Codex, `CodexRolloutJsonl`                                                                                | Yes | Yes | Yes | Yes | Yes | Yes | Yes | Yes | Yes |
| OpenCode, `OpenCodeJsonl` or `OpenCodeSqliteV2`                                                           | Yes | No  | Yes | No  | No  | Yes | Yes | No  | Yes |
| Pi, `PiV3Jsonl`                                                                                           | Yes | Yes | Yes | No  | No  | No  | Yes | No  | Yes |
| Antigravity, `AntigravityJson`, `AntigravityBrainJsonl`, `AntigravityCascadeJson`, or `AntigravitySqlite` | Yes | No  | No  | No  | No  | No  | Yes | No  | No  |

`AntigravityWorkspaceChatJson` is uncharacterized and has no prompt support.
Cursor and the other source formats have no remediation prompt support.

### Auto Fix

| Agent       | Model       | Reasoning            | Exact source                                       |
| ----------- | ----------- | -------------------- | -------------------------------------------------- |
| Claude Code | Auto Fix    | Auto Fix to `medium` | `ClaudeJsonl`                                      |
| Codex       | Auto Fix    | Auto Fix to `medium` | `CodexRolloutJsonl`                                |
| OpenCode    | Auto Fix    | Unavailable          | `OpenCodeJsonl`, `OpenCodeSqliteV2`                |
| Pi          | Auto Fix    | Auto Fix to `medium` | `PiV3Jsonl`                                        |
| Antigravity | Unavailable | Unavailable          | No accepted source binds one safe physical control |

Model Auto Fix applies only to a reviewed obsolete model and its reviewed
replacement. Reasoning Auto Fix applies only to a reviewed above-cap level when
`medium` is a valid below-cap value. MCP, built-in tool, skill, worker, depth,
speed, and cache edits remain prompt-only or unavailable. Their evidence does
not bind one safe durable control.

macOS and Linux can read, attribute, prepare, and apply. Native Windows can read
and attribute the setting, but it cannot prepare or apply an edit. Windows apply
needs reviewed ACL, reparse-point, sharing, file-identity, replacement, and
recovery behavior. WSL has a separate environment key. It does not read or edit
the native host config.

### Verification And Savings

| Check | Claude Code                          | Codex                                | OpenCode                             | Pi                                   | Antigravity |
| ----- | ------------------------------------ | ------------------------------------ | ------------------------------------ | ------------------------------------ | ----------- |
| D     | Unavailable                          | Unavailable                          | Unavailable                          | Unavailable                          | Unavailable |
| T     | Verified improvement                 | Verified improvement                 | Unavailable                          | Verified improvement                 | Unavailable |
| S     | Unavailable                          | Unavailable                          | Unavailable                          | Unavailable                          | Unavailable |
| M     | Unavailable                          | Unavailable                          | Unavailable                          | Unavailable                          | Unavailable |
| B     | Unavailable                          | Unavailable                          | Unavailable                          | Unavailable                          | Unavailable |
| K     | Unavailable                          | Unavailable                          | Unavailable                          | Unavailable                          | Unavailable |
| O     | Verified improvement and USD savings | Verified improvement and USD savings | Verified improvement and USD savings | Verified improvement and USD savings | Unavailable |
| F     | Verified improvement                 | Verified improvement                 | Unavailable                          | Unavailable                          | Unavailable |
| C     | Unavailable                          | Unavailable                          | Unavailable                          | Unavailable                          | Unavailable |

D is tied to one historical session. S is tied to one call and worker. Later
work has a different identity. M, B, and K expose only observed subsets, so
absence does not prove removal. C has no durable session-route control. OpenCode
lacks historical reasoning and speed controls. Pi lacks an effective speed
control. Antigravity has positive-only D and O evidence but no attributed
physical model target.

T requires a complete later assessment with an explicit lower level on the same
route and model. F requires an explicit standard tier on the same route, model,
and delegated scope. O requires actual replacement use on the same attributed
physical target, scope, provider, and API.

## Verification And Recurrence

The durable lifecycle is:

```text
reserved -> writing -> recoveryNeeded -> watching -> fixed -> recurred
```

Prompt actions enter `watching`. Auto Fix uses the write states first. A
successful file readback means `watching`, not `fixed`.

Only sessions that start after the effective boundary can prove a transition.
The source format and exact scope must match. Truncated pages, partial evidence,
missing controls, stale projections, and changed policy or catalog revisions do
not prove a fix.

T and F use positive control proof. O uses actual model-use proof. Generic clean
absence is not enough for resource targets. After a fixed transition, the first
later exact positive finding marks recurrence. Recurrence stops new savings at
its evidence time. It does not erase the earlier verified contribution.

Config readback, a copied prompt, an agent completion claim, inactivity, source
deletion, report age, or leaving the report window never proves a fix.

## Opportunity And Savings

`Estimated opportunity` describes the current finding before proof. It uses
only available bounded finding facts. The current UI can show a numeric
opportunity for D and for replicated B definitions. Other methods stay unknown
when their inputs are absent.

`Your savings` describes a verified later transition. Every verified transition
can store one improvement count. O can also store cumulative API-equivalent USD
for eligible replacement activity. A rate is not a cumulative saving.

All nine estimate methods are implemented as typed calculations:

| Check | Method                            | Unit                  | Required inputs                                                            |
| ----- | --------------------------------- | --------------------- | -------------------------------------------------------------------------- |
| D     | `RepeatedContextAboveDepthCap`    | Literal input tokens  | Observed request tokens and the reviewed depth cap                         |
| T     | `AssumedOutputReduction`          | Assumed output tokens | Observed output and an explicit basis-point assumption                     |
| S     | `WorkerModelPriceDifference`      | API-equivalent USD    | Exact worker tokens, old and alternative rates, route, revision, and owner |
| M     | `McpDefinitionExposure`           | Literal input tokens  | Definition tokens and compatible request count                             |
| B     | `BuiltInDefinitionReplication`    | Literal input tokens  | Catalog-backed replicated definition tokens                                |
| K     | `InjectedSkillDocument`           | Literal input tokens  | Full document tokens and compatible request count                          |
| O     | `OldModelPriceDifference`         | API-equivalent USD    | Eligible token classes, both model rates, pricing revision, and owner      |
| F     | `FastTierPricePremium`            | API-equivalent USD    | Eligible tokens and same-route fast and standard rates                     |
| C     | `CacheRehydrationPriceDifference` | API-equivalent USD    | Repeated paid tokens, paid and cache-read rates, and pricing revision      |

`CacheClassTokens` and `Improvements` are also distinct supported units. The
current confirmed contribution path uses improvement counts and O dollars. It
does not convert price differences into token reductions.

Known zero and negative values stay known. Missing evidence, assumptions,
comparisons, rates, revisions, or ownership stays unavailable. Overflow stays
unavailable.

## Overlap And Retention

Each additive value needs one nonempty durable owner. Aggregation rejects a
duplicate owner or mixed units. The current contribution owner is the attempt
allocation. This prevents one verified transition from being counted twice.
Values with unresolved cross-method ownership stay separate. The app never sums
the nine report percentages.

A contribution stores bounded derived display and savings facts. Normal session
retention does not delete it. The store keeps at most 1,000 contribution rows and
1,000 durable attempt rows. It can remove an old fixed attempt only after its
contribution is durable. Active attempts remain until they reach a terminal
state or storage cannot enroll another target.

A correction replay dirties fixed and recurred attempts. A fixed result loses
its contribution only when corrected evidence positively shows that the target
was still unresolved. Missing evidence does not erase prior proof. A recurred
attempt stays closed. Equal or newer facts replace one owner atomically with the
verification result. Clearing local indexed data removes attempts and
contributions.

## Bounds And Privacy

Target listing scans at most 512 current sessions and retains at most 512 raw
findings. It returns at most 100 grouped targets and three sample handles per
target. The action cache holds 100 targets. Each winning publication retains at
most 100 findings per detector, selects at most 100 fairly across detectors, and
enrolls only verification-eligible passive attempts.

The prepared cache holds at most eight operations and 4 MiB of retained bytes.
One config file is at most 256 KiB. Definition, result, display, and contribution
JSON values are each at most 32 KiB. Aggregate reads and durable tables are
bounded at 1,000 rows.

Public DTOs contain safe labels, counts, time bounds, typed states, and opaque
handles. A sample additionally contains its stored normalized title, agent, and
safe source category for local display. They do not contain raw paths, session
IDs, call IDs, transcripts, prompts, config content, evidence bodies,
credentials, or private selectors. Sample navigation uses a separate opaque
handle and a guarded shell command. Sample display fields do not enter analytics.

Analytics records visible exposure, typed action outcomes, clipboard success,
and visible verified or recurred outcomes. It uses closed labels and coarse
states. It does not send prompts, target values, IDs, paths, exact tokens, exact
costs, private errors, or evidence revisions. Background verification emits no
engagement event. See [`analytics.md`](analytics.md).

## Extension Guide

To add a check operation:

1. Add or update the typed `FindingCause` and its canonical identity.
2. Add safe display facts and prompt text. Define explicit prompt support.
3. Add one estimate method or map the check to an existing typed method.
4. Define positive verification and recurrence proof. Do not use generic absence when the source is partial.
5. Add publication attribution only when evidence matches one effective physical control.
6. Add the private config operation and semantic review fields.
7. Add characterization, unavailable, correction, retention, privacy, and bound tests.
8. Update `check-coverage.md` and this guide.

To add a vendor operation:

1. Implement or extend `VendorRemediationPolicy` for source and attribution policy.
2. Implement or extend `VendorConfig` for files, precedence, selectors, parsing, and minimal edits.
3. Keep vendor branches out of shared lifecycle, persistence, and atomic-write code.
4. Reject every unreviewed override, managed layer, dynamic value, and ambiguous scope.
5. Test global and project precedence, the actual cwd, the trusted root, prepare/apply conflicts, semantic readback, and recovery.
6. Add every source, setting, scope, environment, and platform cell to table-driven tests.
7. Update the support matrices and state each unavailable case.

## Local Tests

Use synthetic config and session fixtures. Do not use a real home directory or
real transcript.

Run focused engine tests first:

```bash
cd crates/antiburn-local
cargo test remediation
cargo test --test check_coverage_contract
```

Run focused desktop tests next:

```bash
cd apps/desktop/src-tauri
cargo test remediation
cargo test agent_config
```

Run the full repository checks from `CONTRIBUTING.md`. For documentation-only
changes, also run:

```bash
pnpm --filter @antiburn/desktop exec prettier --check \
  ../../docs/remediation.md \
  ../../docs/check-coverage.md \
  ../../docs/session-coverage.md \
  ../../docs/plans/burn-check-remediation.md \
  README.md design.md
node scripts/check-design-drift.mjs
git diff --check
```

Release validation must exercise macOS, Linux, and Windows on real machines.
It must verify supported data, prompts, clipboard behavior, file conflicts,
recovery, sample navigation, both themes, keyboard use, and assistive technology.
