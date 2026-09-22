# Burn Check Remediation

Readers should receive a safe edit or prompt and know which evidence can verify
its result. This guide defines action ownership and the path from a finding to
that result. Use it when
changing action flow, file safety, durable attempts, or the user-visible result.
See
[`check-coverage.md`](check-coverage.md) for the maintained agent/check support,
source-format remediation, attribution, verification, and savings matrices, and
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
| `apps/desktop/src-tauri/src/agent_config/`              | Resolve effective settings and prepare or apply one safe file edit.                                    |
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
6. A target prompt action starts or reuses a durable action attempt. A
   check-level prompt gives selected targets one prompt-group ID while each
   target keeps an independent attempt. A Copy fallback can describe current
   targets without claiming an exact edit.
7. An Auto Fix action prepares one edit, waits for confirmation, and then starts
   or joins an attempt.
8. Later winning evidence dirties matching attempts.
9. The evidence worker verifies each dirty attempt and stores one contribution when proof exists.

The check-level Copy action re-runs the target query. It returns no prompt when
the query has no selectable target. When exact target text is unsuitable, it can
return a generic inspection prompt for the selected current targets. Each
selected target still gets a durable action attempt, and the returned prompt has
one opaque attempt reference. The fallback does not create an Auto Fix operation
or claim an exact edit.

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

Each Auto Fix edits one winning control. It selects the global or user control
when a project inherits that value. It selects a project control only when the
project has the exact explicit setting or resource needed for the edit. Scalar
model, reasoning, compaction, and speed controls are not batched across layers.
The editor never creates a project config file. The only approved missing-global
creation path is `~/.claude/settings.json` for an eligible optional Claude Code
built-in tool; it creates only the exact bare deny rule.

Findings from different projects group into one target when they resolve to the
same global control. Prepare and apply revalidate every grouped project context
against that same scope, physical target, selector, and expected value. A stale
or diverged context blocks the edit instead of being dropped or retargeted.

For D only, the editor can enable an existing automatic-compaction control or
lower an existing numeric compaction limit to the finding depth cap. It does not
create a compaction control, change an enabled in-range control, or infer that
ordinary session growth or fixed instructions have a configurable cause.

| Agent       | Project resolution                                                                                                                                     | Global resolution                                                                                                    |
| ----------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------- |
| Claude Code | `.claude/settings.local.json`, then `.claude/settings.json`; the selected file must contain the setting                                                | `~/.claude/settings.json`                                                                                            |
| Codex       | `<root>/.codex/config.toml`; the setting must exist; the global config must mark the exact root as trusted                                             | `~/.codex/config.toml`                                                                                               |
| OpenCode    | Merge `opencode.json` then `opencode.jsonc` from the trusted root through the actual cwd; then merge `.opencode` directories from cwd back to the root | Merge `config.json`, `opencode.json`, and `opencode.jsonc` under the effective config root; then merge `~/.opencode` |
| Pi          | `<cwd>/.pi/settings.json` when it contains both `defaultProvider` and `defaultModel`; reasoning can use the project model map or default level         | The effective Pi agent directory `settings.json`                                                                     |
| Cursor      | `<root>/.cursor/cli.json`                                                                                                                              | `~/.cursor/cli-config.json`                                                                                          |
| Antigravity | No project model target is registered                                                                                                                  | `~/.gemini/antigravity-cli/settings.json`                                                                            |

For Claude built-in tools, a project settings file is an exact project target
only when its `permissions.allow` array contains the bare canonical tool name.
An inherited tool uses the global settings target. OpenCode uses the last
matching model value in its reviewed merge order. Pi
rejects a split provider and model route. Codex rejects an active profile and a
nested cwd. Claude reasoning prefers a per-model `modelSettings` value, then a
top-level `effortLevel`.

The trusted root and actual cwd are separate facts. The trusted root comes from
the enabled, accessible repository inventory. It sets the file safety boundary.
The actual cwd comes from the session. It selects cwd-sensitive project config.
OpenCode and Pi allow a cwd below the trusted root. Codex requires the cwd to
equal the trusted root. Recovery stores only the cwd relative to the trusted
root and reconstructs it inside that root.

Cursor MCP files are `<root>/.cursor/mcp.json` and `~/.cursor/mcp.json`.
Antigravity MCP files are `<root>/.agents/mcp_config.json` and
`~/.gemini/config/mcp_config.json`. Generic agent skill inventory paths are
`<root>/.agents/skills` and `~/.agents/skills`. These resource paths are
documented for future editor support only. The current resolver does not mutate them.

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

Passive and action attempts are independent for the same target. A later user
action does not replace the passive attempt or move its boundary. Each attempt
keeps its own origin and lifecycle. A check-level prompt groups its action
attempts only so a retry can return the same stored prompt and reference; the
targets still verify or remain unavailable independently. This design measures
passive discovery and deliberate use without claiming that the action caused the
result.

An action watch with unavailable verification does not block a later Auto Fix.
The store upgrades that watch through the same crash-safe reservation path. A
verifiable action watch and an active or uncertain write still block Auto Fix.

After recurrence, a later publication can create a new attempt. The new attempt
gets a new durable ID and prompt reference.

## Prompt Fallback

Prompt support is independent from Auto Fix support. The UI offers `Copy fix
prompt` when the engine recommendation gate accepts at least one selectable
current target. The check-level action can use the bounded generic text for
those targets. It returns no prompt when no target can be selected. Copy remains
available when Auto Fix lacks an attributed setting, supported editor, safe
platform, or trusted target.

An exact-target prompt includes the finding, safe facts, source limits,
requested result, and required evidence. A check-level fallback names the failed
check and practical objective. It asks the coding agent to inspect
representative session evidence and the effective project and user
configuration before it proposes a change. It does not claim an exact model,
setting, scope, config file, or replacement that current target evidence cannot
prove. The agent can list labeled hypotheses and the evidence needed to confirm
them. It must not present a hypothesis as a fact or apply an unproved edit.

Both prompt operations can add up to three absolute paths resolved from the
local session index. The prompt quotes and labels them as representative session
evidence, not configuration edit targets. Non-file, relative, unresolved, duplicate,
or oversized path candidates are omitted. Paths appear only in prompt text
returned after the explicit copy action. They do not enter target or sample
payloads, rendered details, analytics, logs, snapshots, finding IDs, action IDs,
or watches.

Every prompt contains at most 64 KiB. Exact-target facts contain at most eight
sanitized identities. Prompts exclude session IDs, transcript content, config
content, credentials, and unrelated history. A private or truncated essential
identity makes an exact-target prompt unavailable. The main window writes prompts
through the native Tauri clipboard manager. It has write-text permission only.
Clipboard success is separate from prompt preparation. A failed native write can
retry the same prepared text without a second backend operation. The UI reports
prompt preparation and clipboard write failures separately.

Each returned prompt contains one opaque `Remediation reference: ABR-<id>`
marker. Every selected target has a durable attempt. The attempt stays in the
Failed checks group until a
later captured user message contains that exact marker. The winning publication
that captures the marker starts verification at its publication time. The
marker-bearing session cannot verify the change. A copied prompt that never
appears in captured user content does not start verification.

## Auto Fix Safety

Auto Fix uses two explicit steps.

1. `prepare_auto_fix_burn_check_target` revalidates the evidence and effective
   setting. It prepares exact replacement bytes and returns a bounded semantic
   review.
2. `apply_prepared_burn_check_operation` accepts only the prepared-operation ID.
   It revalidates evidence, scope, precedence, selector, file identity, and
   original bytes before replacement.

Before Auto Fix replaces an existing config file, it atomically writes the exact
pre-update bytes to a sibling `<config-file>.bak` and syncs that backup. A safe
existing backup is replaced, and the new backup remains after success. Auto Fix
does not create a backup when it creates a new config file because no prior
content exists. A backup failure leaves the config unchanged. Recovery never
restores a backup without a user action.

The review names the agent, scope, setting, current value, proposed value,
effect, and side effect. It does not expose a path or raw config.

The editor rejects ambiguous targets, dynamic values, malformed or duplicate data,
files above 256 KiB, non-regular files, symlinks in the target path, unsafe
roots, and unsupported Unix ownership. It preserves unrelated values. TOML
keeps formatting where `toml_edit` can preserve it. JSON uses formatted output.

Model and reasoning targets remain pinned to their publication-time physical
attribution. Current resolution must match that scope and physical target; Auto
Fix does not move the edit to a different layer. The editor can prepare when the
controller reports a runtime or managed override.
The review warns that the effective behavior might not change. Vendor-detected
environment, remote, organization, and profile overrides remain unavailable.

Apply stages an exclusive temporary file in the affected directory. It preserves
permissions and Unix owner and group. It syncs the file, checks the original
identity and bytes, atomically replaces the target, syncs the directory, reads
the file again, and verifies the typed setting. A conflict never retargets or
rebuilds the edit. Approved global creation uses an exclusive new file, safe
parent directories, directory sync, and exact-byte readback.

A failure before replacement cancels the reservation. A result that can follow
replacement enters durable `recoveryNeeded`. Recovery resolves the same agent,
source, setting, scope, trusted root, relative cwd, physical selector, and
replacement value. It starts verification only when readback proves the change.
For an unverifiable operation, successful readback records
`verificationUnavailable` and returns an applied result without promising later
verification. An old value cancels the uncertain write. A changed or unprovable
target remains blocked and cannot start a second write.

Repeated confirmation returns the saved result only for the same completed
prepared operation. Expired, evicted, or failed operations cannot be reused.

## Support And Eligibility

The maintained support baseline is in:

- [First-tier product matrix](check-coverage.md#first-tier-product-matrix):
  reachable findings, prompts, Auto Fix, verification, and estimates by agent/check.
- [Source-format remediation matrix](check-coverage.md#source-format-remediation-matrix):
  accepted source limits.
- [Automatic editor support](check-coverage.md#automatic-editor-support):
  exact settings and platforms.
- [Passive verification](check-coverage.md#passive-verification):
  eligible discovery watches.
- [Evidence boundaries](check-coverage.md#evidence-boundaries) and
  [config attribution contracts](check-coverage.md#config-attribution-contracts):
  the rationale behind those cells.

A source or check without evidence remains unavailable even when its agent has an
editor. Prompt eligibility does not imply an exact file edit. A whole-check resource
prompt lists every selected target; selection is limited to 100 and each resource label
to 256 bytes before JSON escaping. The 64 KiB prompt bound includes the complete
selected list. The built-in-tool policy suggests only optional specialized tools; it
never suggests disabling shell, file, search, task, agent, or subagent tools.

Auto Fix requires current evidence bound to one reviewed control, an editor that
supports the setting and platform, and successful prepare/apply checks. Native Windows
has read attribution but no apply path; WSL never edits native host configuration. An
unavailable Auto Fix leaves a supported prompt action available.

Model replacement requires a reviewed obsolete model and its reviewed replacement.
Reasoning changes to `medium` only when the observed level exceeds the reviewed
cap and `medium` is a valid below-cap value. For one exact Claude Code MCP
server, the editor appends only `mcp__<name>__*` to an existing same-scope deny
list; it does not broaden another permission rule.

## Verification And Recurrence

The durable lifecycle is:

```text
reserved -> writing -> recoveryNeeded -> watching -> fixed -> recurred
waitingForPromptUse -> watching
```

Every exact prompt action enters `waitingForPromptUse` and stays in the Failing
group after the copy. It enters `watching` only after its exact marker is
captured in user content. A supported verifier then appears as Awaiting until it
passes or recurs. An activated prompt with unsupported verification, including
M/B/K, also appears as Awaiting, but its typed verification and savings remain
`verificationUnavailable` and `unavailable`; it cannot become Passed. Auto Fix
uses the write states first. A successful file readback starts `watching` only
when positive verification exists. Otherwise the applied result and retained
write record state that verification is unavailable.

Only sessions that start after the effective boundary can prove a transition.
The source format and exact scope must match. For T and F, one complete later
session must contain the exact positive control for the same target. For O, one
later session must contain actual replacement-model use for the same attributed
target. Report-level absence and historical counts do not verify a fix.
Truncated pages, partial evidence, missing controls, stale projections, and
changed policy or catalog revisions do not prove a fix.

Generic clean absence never verifies resource targets. After a fixed transition,
the first later exact positive finding recurs that target. Recurrence stops new
savings at its evidence time. It does not erase the earlier verified
contribution.

Config readback, a copied prompt, an agent completion claim, inactivity, source
deletion, report age, or leaving the report window never proves a fix.

## Opportunity And Savings

`Estimated opportunity` describes the current finding before proof. It uses
only available bounded finding facts. M/B/K target rows can show replicated
tokens and a burn percentage when their definition or listing tokens and the
report denominator are available. Skill estimates include listing frontmatter,
not the skill body. MCP estimates require measured indexed definitions. Other
methods stay unknown when their inputs are absent.

The Passed section separates two estimates. `Estimated savings` is the stored
pre-remediation opportunity from one target snapshot per active passed cycle. It
does not use recent or post-remediation usage. `Confirmed savings` describes
eligible sessions observed after that cycle passes. Savings status is `pending`
before enough later usage exists, `known` with a revisioned numeric value,
`unknown` when a required input or calculation failed, or `unavailable` when the
check has no confirmed-savings method. Every verified transition can store one
improvement count. O can also store cumulative API-equivalent USD for eligible
replacement activity. A rate is not a cumulative saving.

The aggregate-savings backend returns only exact current cycles that are still
fixed, have a fixed verification result and verified evidence boundary, match
their stored target snapshot, and are not actively snoozed. The renderer keeps
only cycles whose detector is visibly Passed. It shows both savings labels when
at least one such cycle remains and hides the entire Savings section otherwise.
Failing, Awaiting, Snoozed, and recurred cycles do not contribute. Recurrence
stops confirmed accumulation at its evidence time.

## Snoozed Checks

A snooze keeps the stored check state but removes that detector from active
check groups, counts, savings, session badges, session filters, session detail,
and discussion prompts. The report supplies one denominator-aware aggregate for
each of the 512 possible selections of the nine detectors. After snoozing, the
desktop selects the exact remaining-detector aggregate. Each aggregate is
recomputed from the selected detector contributions against the report's same
complete total-token denominator, with overlap and bounded-fallback rules
applied again; the desktop never subtracts displayed detector percentages.

Snooze state is a required input, not an optional filter. While it loads,
dependent check and session surfaces withhold derived results and show their
loading state. If it fails, they show an unavailable or explicit snooze-load
error instead of presenting unfiltered data; the main Burn Checks error offers
Retry. The main Burn Checks page keeps loaded snoozed checks in its collapsed
management group, where Unsnooze restores the underlying state. When every
assessed check is snoozed, active surfaces show `No active checks` instead of a
passing or unassessed result.

The nine typed methods, their units, required inputs, and numeric limits are maintained
in the [savings contracts](check-coverage.md#savings-contracts). Missing inputs, rates,
revisions, or ownership remain unavailable; known zero and negative values remain known.
The current confirmed contribution path uses improvement counts and eligible O dollars.
It does not convert price differences into token reductions.

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
target. The action cache holds up to 200 target IDs per detector, so one relist
does not evict the IDs still held by a window. Each winning publication retains
at most 100 findings per detector, selects at most 100 fairly across detectors, and
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
4. Define verification and recurrence proof. Do not use generic absence when the source is partial.
5. Add publication attribution only when evidence matches one effective physical control.
6. Add the private config operation and semantic review fields.
7. Add characterization, unavailable, correction, retention, privacy, and bound tests.
8. Update `check-coverage.md` when support changes. Update this guide when the
   action flow, safety rules, or user-visible result changes.

To add a vendor operation:

1. Implement or extend `VendorRemediationPolicy` for source and attribution policy.
2. Implement or extend `VendorConfig` for files, precedence, selectors, parsing, and minimal edits.
3. Keep vendor branches out of shared lifecycle, persistence, and atomic-write code.
4. Reject every unreviewed override, managed layer, dynamic value, and ambiguous scope.
5. Test global and project precedence, the actual cwd, the trusted root,
   prepare/apply conflicts, semantic readback, and recovery.
6. Add every source, setting, scope, environment, and platform cell to table-driven tests.
7. Update the support matrices in `check-coverage.md` and state each unavailable case.

## Local Tests

Use synthetic config and session fixtures. Do not use a real home directory or
real transcript.

Run focused engine tests first:

```bash
cd crates/antiburn-local
cargo test remediation
```

Run focused desktop tests next:

```bash
cd apps/desktop/src-tauri
cargo test remediation
cargo test agent_config
```

Run the checks that cover the changed behavior. Run the full repository checks
from `CONTRIBUTING.md` before a broad remediation change. For documentation-only
changes, run:

```bash
pnpm --filter @antiburn/desktop exec prettier --check \
  ../../docs/remediation.md \
  ../../docs/check-coverage.md \
  ../../docs/session-coverage.md \
  README.md design.md
node scripts/check-design-drift.mjs
git diff --check
```

Release validation must exercise macOS, Linux, and Windows on real machines.
It must verify supported data, prompts, clipboard behavior, file conflicts,
recovery, sample navigation, both themes, keyboard use, and assistive technology.
