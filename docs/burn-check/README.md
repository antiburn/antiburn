# Burn Checks

Burn Checks assess recorded agent work from YAML definitions. The maintained
**Failed Retry Loop** check looks for repeated attempts at the same unresolved
failure. It excludes relevant recovery, requested retries, and expected polling.

> **Experimental draft: `v1-prealpha`.** Burn Checks are not yet standardized.
> Syntax, semantics, and capabilities may change, including breaking changes.
> There is no backward compatibility promise. Use the current
> [specification](specification.md) and [supported subset](authoring.md#supported-subset).

## Try the repository dogfood check

Use the permanent repository definition at
`.antiburn/checks/failed-retry-loop.yml`. It follows the maintained
[Failed Retry Loop example](examples/failed-retry-loop.yml).

1. Enable this repository in Antiburn's native session discovery.
2. Open **Settings → Checks**, enable **Model-based Checks**, and configure **Jev**
   under **Decision model**. The definition uses `typesafe/jev-1.13.0`.
3. Continue normal work in a supported native main session. Discovery is
   automatic for supported active native main sessions. Fresh attachment can use
   bounded recent results and assess the latest ready window once. Later new
   terminal results with safe identity and order enter the live window, including
   usable partial evidence. It does not replay older windows.
4. Find **Failed Retry Loop** in Checks or local search. It stays **Not assessed**
   until an eligible assessment writes status. Open its normal detail pane for
   the explanation, source details, and session cards.

The example needs at least three results in a rolling window of five. It does
not promise a finding in every session. If neither action matches uncertainty or
an answer below its 0.95 gate, the assessment writes no status and preserves any
earlier result. There is no fallback success. Snooze hides active failure summaries;
it does not pause evaluation. The native past-session control does not replay
YAML definitions. Dogfood work and developer evals are independent.

The definition permits middle-text shortening only for
`activity[*].tool.result.content`. It preserves selected result records, linked
arguments, and request/authorization text. Shortening is reported to the model
and in the check detail. If initial local request-size or preparation limits stop
preparation, the candidate is not assessed and no provider request is sent.
A preparation-limit stop does not prove that further permitted shortening could
not fit. Open the detail's **Open check file** action
to repair a definition in its default application. See
[fit large inputs](authoring.md#fit-large-inputs) for the limits.

Built-in Ignored Instructions, Scope Creep, Over-exploring, and Skill Opportunities
execution and historical enrollment are suspended. Their saved results,
preferences, and implementations remain. YAML examples with similar names are
separate authored checks; they do not restore the built-ins' evidence capabilities.

## Install your own check

Put an immediate `.yml` or `.yaml` file in `~/.antiburn/checks/` or a known native
repository's `.antiburn/checks/` directory. Create the directory yourself; the app
only watches and reconciles it. A project file replaces the global file with the
same lowercase filename stem. Definitions do not merge.

See [authoring](authoring.md) for identity, supported features, validation, and
diagnostics. Burn Checks use the existing Checks collection, detail pane,
search, and snoozes. They provide status and explanations, without native fix
prompts, Auto Fix, verification watches, burn estimates, or savings.

## Examples

All six examples use the local schema modeline. Structural validity does not
establish runtime or source support. Unsupported examples identify missing
capabilities; they are not delivery promises.

| Example                                                   | Current runtime support | Scope or missing capability                                                                      |
| --------------------------------------------------------- | ----------------------- | ------------------------------------------------------------------------------------------------ |
| [Failed Retry Loop](examples/failed-retry-loop.yml)       | Supported subset        | Repeated unresolved failure in selected tool results                                             |
| [Over-exploring](examples/over-exploring.yml)             | Supported subset        | Clearly unrelated read/search work in selected results                                           |
| [Scope Creep](examples/scope-creep.yml)                   | Supported subset        | Possible separate objective in selected results, not a whole-turn judgment                       |
| [Ignored Instructions](examples/ignored-instructions.yml) | Unsupported             | Needs instruction snapshots and message actions                                                  |
| [Skill Opportunities](examples/skill-opportunities.yml)   | Unsupported             | Needs effective skill snapshots and message actions                                              |
| [Fast Mode Value](examples/fast-mode-value.yml)           | Unsupported             | Needs session-idle events, model/mode filters, measured activity, functions, and message actions |

## Reference

- [Authoring guide](authoring.md): installation, supported subset, and validation.
- [Specification — `v1-prealpha`](specification.md): precise current draft rules.
- [Schema](schemas/burn-check.schema.json): structural validation for the draft.
- [Architecture](architecture.md): ownership and dataflow.
- [Session coverage](../session-coverage.md#burn-check-source-facts): accepted
  source shapes, producer pins, and evidence limits.
- [Check coverage](../check-coverage.md#burn-check-definitions): finding and result limits.
