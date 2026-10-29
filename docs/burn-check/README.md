# Burn Checks

Use Burn Checks to review selected agent work against rules you write in YAML.
A check can ask a model a narrow question or apply local status rules. Antiburn
shows the result and its saved evidence in Checks, so you can decide what needs
attention. A result covers the selected work, not the quality of a whole session.

> **Experimental draft: `v1-alpha`.** Burn Checks are not yet standardized.
> Syntax, semantics, and capabilities can change, including breaking changes.
> There is no backward compatibility promise. Beta and stable scope are not set.
> Use the current [specification](specification.md) and
> [supported subset](authoring.md#supported-subset).

## Install your own check

1. Create `~/.antiburn/checks/` for global checks, or `.antiburn/checks/` inside a
   known native repository. The app watches these folders but does not create them.
2. Save a `.yml` or `.yaml` definition directly in that folder. Subfolders are not
   searched. A project file replaces a global file with the same lowercase
   filename stem; files do not merge. See
   [identity and installation](authoring.md#identity-and-installation)
   for naming, access, and invalid-override rules.
3. Configure a connection and enable **Run checks** as described below. Local
   checks also need this setup, but do not send model requests.

Start with [authoring](authoring.md#write-one-bounded-check) to choose evidence,
write local or model rules, and validate a file. Selected messages, arguments,
results, paths, instruction text, and skill descriptions can contain private data.
Inputs selected by a model check can leave the machine in provider requests.

## Try the repository dogfood checks

The five [canonical definitions](#examples) live in `.antiburn/checks/`. The docs
link to those files; they do not keep a second YAML copy.

1. Enable this repository in Antiburn's native session discovery.
2. Open **Settings → Checks → Model-based checks** and turn **Run checks** off
   during setup. Choose **Add connection**, select a provider, enter its fields,
   then select **Add connection** in the dialog. The first connection becomes
   selected without enabling checks. Choose **Use** to select another saved
   connection. Enable **Run checks** when ready.
3. Continue work in a supported active native main session. Fresh attachment can
   assess the latest ready window from bounded recent tool results once. Later
   safe terminal results enter the live window. It does not replay older windows.
   OpenCode V2 starts from a verified future boundary instead of stored results.
4. Find the check by its display name in Checks or local search. Open its detail
   pane for evidence, waiting progress, and diagnostics. **Open check file** opens
   the definition in its default application.

Jev is the recommended route. **Test connection** is optional: it sends synthetic
input and can incur charges. All five examples need label probabilities. A
label-only Choice route cannot run them. Do not substitute provider confidence
or lower a probability gate to make a connection fit. See
[decision connections](authoring.md#decision-connections).

Fresh setup without a connection stays off. **Run checks** preserves an existing
saved preference; malformed or unreadable storage disables execution. Removing
the selected connection turns checks off and clears selection without choosing
a replacement or deleting saved results. Check cards and navigation require a
discovered definition. Settings stays available without one. A discovered check
without a configured provider shows a setup error in its detail pane; the app
does not create a placeholder check.

Turning **Run checks** off stops execution, not local definition discovery. The
app can still read and parse bounded YAML files and watch their directories,
including at cold startup, to show names, invalid-file diagnostics, paused cards,
and file counts. This does not restore provider configuration, discover model
capabilities, catch up session sources, or run local/model assessments.

## Upgrade from fixed model-based checks

Authored YAML checks replace the fixed Ignored instructions, Over-exploring,
Scope creep, and Skill opportunities detectors. The nine deterministic built-in
checks remain separate. Install a YAML definition to run its replacement;
the app does not install the repository examples into your other projects.

The database upgrade removes the retired detectors' assessment history, response
cache, and old snoozes. It does not convert their findings into YAML results.
It preserves incurred usage totals and saved connection settings. New YAML
checks start Not assessed until they write their own result.

This migration is different from deleting a YAML definition. Removing a YAML
file stops its effective check but retains its accepted assessment history and
incurred usage.

## What works today

The runtime supports local status checks and model checks with independent named
decision blocks. Each block can mix up to eight Choice, Score, and Noul questions.
Settings selects one connection for the complete assessment. A decision-level
YAML `model` field is invalid; coding-agent model fields remain source evidence.

Choose one trigger family. Each event still needs proof from a supported source:

| Family         | Events                                                          | Current boundary                                                                                                                            |
| -------------- | --------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------- |
| Message        | `user_message`, `assistant_message`, `tool_call`, `tool_result` | Public, admitted records only. Alternatives can share one rolling window.                                                                   |
| Response cycle | `turn_end`                                                      | Requires a proven complete native response-cycle boundary. Inactivity does not prove one.                                                   |
| Session        | `session_start`, `session_idle`                                 | The desktop emits start reason `attached` only. Idle is a persisted 120-second observation, not proof of real idle time or model occupancy. |

Only tool results can seed bounded recent work at attachment. Other public
message kinds and response cycles are future-only. Source, producer-version,
identity, order, and native-main-session limits still apply. Remote, WSL, child,
and historical-only sources do not start new live checks. See
[event selection](authoring.md#event-selection) and
[live attachment](authoring.md#identity-and-installation).

Cursor Desktop has a separate ordered snapshot route for terminal tool results
and their user-slot context. It does not supply the other public or session
events. Only the characterized native inline main-session source qualifies;
Cursor CLI and generic stores do not. See the
[Cursor snapshot limits](../session-coverage.md#cursor-desktop-yaml-tool-evidence).

Inputs can use frozen context, pure functions, dynamic brackets, nested
projections, text templates, and bounded maps. Optional `fit` rules permit loss
only from named input targets. The runtime prepares all model blocks before
inference, runs them in order on one captured connection, and waits for the full
valid answer set before actions. Shared evaluation and retained-state limits
bound the assessment. See [input transformations](authoring.md#transform-selected-input),
[typed questions](authoring.md#typed-questions-and-actions), and
[input fitting](authoring.md#fit-large-inputs).

Local checks omit `decisions`. They use frozen context without model discovery,
warm-up, provider preparation, transport, inference, attempts, or usage records.
They still need the master setting, a selected configured connection, and the
credential/authentication and runtime gates. Failed cached model discovery alone
does not block them. Local cooldown defaults to `0s`; model cooldown defaults to
`60s`. See [local status checks](authoring.md#local-status-checks).

### Read a result safely

A card stays **Not assessed** until an accepted action writes **Passed** or
**Failed**. Queued or running work adds **Checking** beside the saved result; it
does not clear a failure. Snooze hides active failure summaries but does not pause
evaluation. A capture alert describes a current source failure, not an older
history gap alone.

All five examples use a 0.95 failure-label probability gate. Ignored instructions
also requires `no_conflict` at 0.95 or above to write success; uncertainty leaves
the saved result unchanged. The other four use the exact complement of their
failure condition for success, including valid uncertain answers. These are
author policies, not accuracy guarantees or confidence in source facts. Success
does not prove repair, whole-session correctness, or savings. Missing evidence
and execution failures do not create success.

Usable partial evidence can be assessed, with its limits shown to the model.
Missing is not empty or clean. A user-slot record can come from an API or
extension and does not prove human approval. Tool execution status does not prove
program success: a completed shell tool can report a nonzero exit. Current
instruction files and skill descriptions do not prove past activation, effective
session loading, or historical non-use.

Each example permits middle-text shortening of
`activity[*].tool.result.content`. Ignored instructions also permits unused skill
descriptions to shrink. Protected requests, arguments, instruction text, scope
limits, skill names, and paths stay intact. The model and detail pane report
shortening. A local preparation stop sends no provider request; a resource-limit
stop does not prove that further allowed shortening could not fit. Omitting
`fit` is valid and creates no warning by itself.

## What does not work today

Schema acceptance alone does not make a check runnable. The compiler rejects
unsupported fields and actions before activation, including message actions
behind `if: false`. The current runtime does not supply effective `agent.skills`,
`repo.branch`, historical instruction activation, or historical effective skill
inventories. Desktop `session_start` does not emit `new` or `resumed`.

Burn Checks provide status and explanations through the existing collection,
detail pane, search, navigation, and snoozes. They do not provide native fix
prompts, Auto Fix, verification watches, burn estimates, or savings. A YAML name
does not identify a native detector or grant its capabilities. Automatic history
assessment is unavailable; developer replay uses explicit isolated state.

These capabilities have no assigned release. Beta and stable scope are not set.
The broader draft does not promise implementation.

## Examples

These five tool-result checks use the local schema modeline and the supported
runtime subset. Structural validity does not prove source support. The coverage
documents state accepted sources, producer versions, and result limits.

| Definition                                                              | Assessed scope and limits                                                                                                                                              |
| ----------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [Over-exploring](../../.antiburn/checks/over-exploring.yml)             | Clearly unrelated completed reads/searches. Plausible dependencies, tests, and verification remain allowed.                                                            |
| [Scope creep](../../.antiburn/checks/scope-creep.yml)                   | Supported completed work on a separate objective. User approval stays separate from tool claims.                                                                       |
| [Ignored instructions](../../.antiburn/checks/ignored-instructions.yml) | Uses separate labels for current-file mismatches and conflicts with an earlier recorded correction. Keeps partial-scope limits explicit.                               |
| [Skill opportunities](../../.antiburn/checks/skill-opportunities.yml)   | Suggests reviewing the configured skill descriptions in the saved input for useful next steps. Does not choose a specific skill or claim that it was never used.       |
| [Fast mode value](../../.antiburn/checks/fast-mode-value.yml)           | Standard-mode trial advice for explicitly non-urgent work. Speed records, catalog mode, requested-tier preference, and name hints do not prove served tier or savings. |

The instruction and skill checks select whole current scopes to preserve missing
arrays and scope limits. Each can include the other inventory in that scope.
Ignored instructions can shorten only unused skill descriptions there; Skill
opportunities keeps its inventory intact. See the
[snapshot boundary](architecture.md#identity-order-and-frozen-evidence).

Over-exploring does not treat arbitrary shell commands as reads. Codex admits
only the reviewed single-file numbered shell-read shape when a successful result
proves the complete output extent. Other shell exploration stays outside its
window. See [check limits](../check-coverage.md#burn-check-definitions).

## Reference

- [Authoring guide](authoring.md): write, install, validate, and diagnose a check.
- [Specification: `v1-alpha`](specification.md): exact draft rules.
- [Schema](schemas/burn-check.schema.json): structural validation only.
- [Provider contracts](provider-contracts.md): accepted typed responses and local
  limits, separate from YAML and source support.
- [Architecture](architecture.md): ownership, dataflow, and recovery.
- [Expression parity corpus](tests/expression-parity.json): synthetic YAML with
  separate schema and compiler expectations, not runtime evidence proof.
- [Session coverage](../session-coverage.md#burn-check-source-facts): accepted
  source shapes, producer pins, and evidence limits.
- [Check coverage](../check-coverage.md#burn-check-definitions): finding and result
  eligibility.
