# Authoring Burn Checks

Use YAML to define one bounded assessment of recorded agent work. The
[specification](specification.md) gives the precise rules; the supported subset
below states what the runtime can execute.

> **Experimental draft: `v1-prealpha`.** Burn Checks are not yet standardized.
> Syntax, semantics, and capabilities may change, including breaking changes.
> There is no backward compatibility promise. Check the current draft before
> creating or changing a definition.

## Write one bounded check

Define one useful behavior, its required evidence, a bounded input, one narrow
question, and independent status actions. Include a positive case, an allowed
case, and an unclear case. Prefer a selected-work claim over an unsupported
whole-session claim. Missing activity does not prove that an action never
happened. Unknown evidence is different from a known empty collection.

Start with [Failed retry loop](examples/failed-retry-loop.yml). It includes
successful results so relevant recovery can end the failure pattern. Its
five-result window and three-attempt condition belong to that definition, not
the generic engine. An unrelated success does not reset the pattern. The check
can describe only the selected results and requests, not unobserved external
changes.

The maintained examples write failure only when their explicit failure condition
is true. Each success condition is its exact logical complement. For example,
Failed retry loop uses these actions:

```yaml
actions:
  - type: status
    if: decisions.review.outputs.verdict == 'stuck' && decisions.review.details.verdict.probabilities.stuck >= 0.95
    input:
      result: failure
      message: The agent may be stuck repeating a failed operation. Review the latest attempts before it continues.
  - type: status
    if: "!(decisions.review.outputs.verdict == 'stuck' && decisions.review.details.verdict.probabilities.stuck >= 0.95)"
    input:
      result: success
```

Quote the negated condition so YAML does not read `!` as a tag. This policy
writes success for valid uncertain answers and answers below the failure gate.
Success means the assessment does not meet the authored failure condition; it
does not prove that the work is correct. Unsupported examples still require
capabilities outside the current runtime subset.

## Supported subset

The schema describes more than the runtime currently supports. The compiler
rejects unsupported definitions before activation, even behind `if: false`.
Do not discard unsupported fields or actions to make a definition run.

- **Trigger:** terminal `tool_result`, optionally filtered by normalized `tools`
  categories `shell`, `read`, `search`, `edit`, and `other`, and result `status`.
  The compiler rejects draft categories `write`, `delegate`, `skill`, and `mcp`
  because the production adapter does not emit them. The first non-final-to-terminal
  transition triggers once; later terminal changes are corrections.
  Unavailable categories stay absent and do not match `other`. The source
  normalizer can classify a recorded tool name as `other`; this is not proof of
  a separate write, delegation, skill, or MCP capability. Unfiltered checks
  can still use the remaining available fields.
- **Window:** rolling message window, written as an integer or `{size, min}`.
  Minimum defaults to one; `min <= size`; maximum size is 40. Fresh attachment can
  seed bounded recent results and assess only the latest ready window once.
  Later safely ordered first-terminal events enter the live window, including
  events with partial fields. Model cooldown defaults to
  `60s`; zero cooldown still uses single-flight execution.
- **Context:** `message`, eligible `window.messages`, `session.id`,
  `session.requests`, `agent.name`, and observed project fields such as
  `repo.root`. Field availability depends on source evidence. Missing values stay
  unavailable to expressions; available request text and tool fields remain
  usable when other context is partial. The model receives `message.quality`,
  `session.requests_complete`, and `session.source_coverage`. A required missing
  expression can make one candidate not assessable. It does not block later
  candidates from the same session.
- **Input and expressions:** JSON literals, nested fixed-key objects/arrays,
  whole `${{ ... }}` expressions, dotted paths, literal bracket keys/indexes,
  array `.length`, parentheses, `!`, `<`, `<=`, `>`, `>=`, `==`, `!=`, `&&`, and
  `||`. Conditions may be Boolean, bare expressions, or whole expressions.
  Type, phase, and availability rules still apply.
- **Input fitting:** optional decision-level `fit` maps selected input paths to
  `recent` for arrays or `truncate` for strings. Paths start at a declared input
  name and support `.field` and `[*]`. Loss is explicit, ordered, and reported to
  the model and in assessment details. See [fit large inputs](#fit-large-inputs).
- **Decision:** exactly one block with one Choice question through
  `typesafe/jev-1.13.0`, with 2–26 criteria. This is the pinned route's limit;
  the compiler rejects excess criteria at the YAML `criteria` field before
  activation. The draft schema can describe other routes. Choice labels and probabilities
  are validated. Enable
  **Model-based Checks**, configure the Jev connection in
  **Settings → Checks → Decision model**. Jev is the only exposed decision route.
  Existing provider admission and resource limits apply.
- **Actions:** independent status actions with literal `success` or `failure`
  and an optional fixed explanation. The last valid applied write wins in
  authored order.
- **Presentation:** one logical filename-stem row in the native 30-day report
  cohort, with normal groups, detail, search, session navigation, and snoozes.
  Status and explanations do not grant native fix prompts, Auto Fix,
  verifier/watch support, burn estimates, or savings.

Unsupported capabilities include other triggers, model/mode/path filters,
instruction and skill inventories, measured activity, functions, projections,
maps, dynamic bracket expressions, embedded text templates, multiple
decisions/questions, Score, Noul, dynamic status results, decision-free local
checks, and message actions. The draft schema retains these forms; schema
acceptance is not runtime admission. See the [example support labels](README.md#examples).

Source support is limited to the native main-session contracts in
[session coverage](../session-coverage.md#burn-check-source-facts). Format names
alone do not prove support for every producer version, remote/WSL source, child
session, or retained history. In `session.requests`, user messages mean accepted
user-slot content, including extension/API input. OpenCode parts marked
`synthetic: true` are excluded in both snapshot and journal paths. Real text in
a mixed message remains available. A generated-only message or replacement
leaves request history partial when no real request remains. These records do
not prove literal-human origin or relax native detectors' approval gates.

OpenCode V2 can supply recorded program facts in `message.tool.result.outcome`
and the optional `message.tool.provider_executed` flag. Result `status` describes
tool execution, not whether a shell command or child task succeeded. For example,
a completed shell tool can report a nonzero `outcome.exit`, and a completed
background launch can report work still running. Missing facts remain absent.
These optional fields do not add status filters or new YAML syntax.

## Identity and installation

The lowercase filename stem is the logical ID. It starts with a letter and
contains lowercase letters, digits, and hyphens. `name` is display text.

Discovery reads immediate `.yml` and `.yaml` files in `~/.antiburn/checks/` and
known native repository `.antiburn/checks/` directories. A nested session CWD
uses the deepest matching known repository root for definition lookup. Evidence
retains its exact environment/session CWD scope.

A project file replaces a global file with the same stem; files never merge.
Duplicate extensions in one scope invalidate that ID. An invalid or unreadable
override stays selected instead of authorizing fallback. Keep drafts outside
active folders. The app watches existing installation chains and reconciles
periodically; it never creates installation directories. Existing folder access
settings apply.

The Checks collection and search show one row per stem. Scoped activations and
branches remain separate. Internal display/snooze identity is separate from
native detector IDs; authors do not add detector enum values.

For the repository definition, follow the [dogfood setup](README.md#try-the-repository-dogfood-check).
Fresh activation attaches to a bounded recent source window. It can assess the
latest ready candidate once; it does not assess every older window. Retained
earlier user messages can supply context with explicit history limits. Restart
resumes unchanged work; changing the effective definition creates a new generation.

Live checks attach to supported native main sessions active within 180 seconds,
under an enabled live activation for the exact environment and stored session
CWD. Remote, WSL, known forks/subagents, unknown or empty CWD, and historical-only
sources do not create new live work. Compatible admitted work still drains after
activity expires; delayed or restarted consumption does not discard it.

After the initial source receipt and session publications are consumed, startup
selects at most 40 current terminal observations within 4 MiB in native order.
Your filters and condition select only the latest ready candidate, even without
`on.window`. Empty receipts get a durable marker. Startup coverage stays partial,
and existing observed work is not reseeded. Other attached sessions can continue
while one initializes. Both native delivery routes continue productive pages
immediately with durable fair rotation. Active sources poll at one second; idle
attached dormant sources use a 30-second resume sweep.

Bounded recent startup is the only automatic historical seed. Later corrections
retain the original first-terminal order and do not become new attempts after activation.

OpenCode V2 is an exception to recent snapshot seeding. For the pinned stock
`v2.0.32` source, retained rows do not prove tool completion order. The native
adapter subscribes to the registered local service before reading a SQLite
sequence boundary. It starts checks from safe terminal events after that
boundary, not the latest 40 stored rows. Request history before attachment stays
partial. A disconnect creates a gap and resets affected windows; a new live
boundary permits later safe results. Antiburn does not start the service, change
event persistence, or require a plugin. Stored sessions still work when the
service or its database binding is unavailable.

A retained pending result can enter when it first becomes terminal. Source
delivery and report readiness do not wait for legacy metric indexing or other
sessions. Historical assessment requires explicit isolated replay.
Live readers retain bounded state and read new suffixes. Their bounded integrity
checks do not audit all omitted or consumed history. See
[source delivery](../session-coverage.md#burn-check-source-facts) for exact
attachment, page, integrity, and recovery limits. Built-in smart-check execution and historical enrollment
are suspended. Their saved results, preferences, and implementations remain.

## Validate in layers

Examples use this editor modeline:

```yaml
# yaml-language-server: $schema=../schemas/burn-check.schema.json
```

When installing elsewhere, update that path or the editor's `yaml.schemas`
mapping. For `.antiburn/checks/failed-retry-loop.yml`, use
`../../docs/burn-check/schemas/burn-check.schema.json`. Do not add a YAML
`version` or `$schema` field.

Run from the repository root:

```sh
npm --prefix docs/burn-check ci
npm --prefix docs/burn-check test
npm --prefix docs/burn-check run validate
```

To validate a selected file:

```sh
node docs/burn-check/scripts/validate-checks.mjs .antiburn/checks/failed-retry-loop.yml
```

The validator checks restricted YAML 1.2 and Draft 2020-12 structure. It accepts
standard JSON-compatible tags and rejects duplicate/non-string keys, custom
tags, anchors, aliases, merge keys, multiple documents, and non-finite values.
It does not activate a check or call a provider.

| Layer             | Responsibility                                                                                          |
| ----------------- | ------------------------------------------------------------------------------------------------------- |
| YAML parser       | One restricted JSON-compatible mapping                                                                  |
| JSON Schema       | Required fields, types, unknown keys, fit path grammar, and rule names                                  |
| Runtime compiler  | Expressions, references, phases, window relationships, fit paths/overlap, and supported capabilities    |
| Source and engine | Order, completeness, frozen input, fit target types, complete request validation, and state transitions |
| Desktop           | Current report rows, evidence details, navigation, search, and snoozes                                  |

Passing one layer does not prove the next. Developer replay is a separate,
finite diagnostic; see the [eval guide](../../apps/desktop/src-tauri/eval/README.md#session-replay).
It uses isolated state and does not alter live rows. It is independent of
repository dogfood work, not a desktop preview or automatic history run.

## Fit large inputs

A small result window can still contain large tool output. Use `fit` only where
the question can tolerate missing evidence. Rules select the evaluated `input`
values, not context roots or YAML source paths. Failed retry loop permits only
result-text shortening:

```yaml
input:
  requests: ${{ session.requests }}
  request_history_complete: ${{ session.requests_complete }}
  source_coverage: ${{ session.source_coverage }}
  activity: ${{ window.messages }}
fit:
  "activity[*].tool.result.content": truncate
```

This keeps request and authorization text, result identities, linked arguments,
status, and the selected result count intact. Tool output can dominate the request
size, so the example grants loss there first. A large request history or argument
value can still prevent assessment. Do not add broad loss permission simply to
make every candidate fit.

`recent` applies to an array, such as `activity: recent`. Each turn drops its
oldest item and retains at least one item from a nonempty array. This can reduce
the evidence below a check's own question threshold; the model must not infer
omitted attempts. The maintained Failed retry loop example does not use this
rule because its question needs at least three attempts.

`truncate` applies only to strings. Each turn removes up to 1 KiB (1,024 bytes)
of original middle text, keeping a Unicode-safe head and tail and at least 256
bytes of original text. Shorter strings remain intact. Repeated turns expand one
middle omission with one marker, `\n[... omitted ...]\n`. Unicode alignment rounds
retention upward; the remaining text is not always exactly 256 bytes. The first
turn is skipped if replacing the omitted substring with the marker would not
reduce its JSON-serialized size, including escaping. Selecting an object or array with `truncate`
does not recursively shorten its contents and is invalid.

Rules expand once in YAML order, with wildcard matches in original array order.
Every concrete array or string gets one turn before the next round. The runtime
checks the complete provider request after every turn and stops as soon as it
fits. This fixed algorithm has no expressions or author-defined priorities.
The [specification](specification.md#85-input-fitting) defines the exact limits
and retry rules.

Unknown top-level inputs, unsupported path syntax, and overlapping paths are
errors. For example, do not combine `activity: recent` with
`activity[*].tool.result.content: truncate`. Missing optional properties and empty
arrays are skipped. Present values must have the expected traversal and target
types; null is a present value, not missing text. A rule that matches nothing
does not authorize shortening another field. The compiler and runtime check
these rules even when the request would fit without loss.

Fitting leaves frozen source evidence unchanged. The model receives original and
retained counts for each changed target, and a successful assessment shows an
input-shortening note. A shortened input cannot establish facts about the omitted
middle text or dropped items. The question should account for this limit; use an
uncertain answer when the remaining evidence cannot support its claim.

If local request validation still fails after the permitted changes, the
candidate is **Not assessed** with typed `NoFit`. The detail distinguishes no
rules, no matching targets, targets at retention/marker limits, and exhausted
reductions. A separate `PreparationResourceLimit` means preparation stops at a
local resource bound; it does not prove that further authorized reductions
could not fit. Legacy no-fit records without a typed reason use a generic
preparation-incomplete explanation.

Both initial failures settle before dispatch: zero provider calls, no new model
cooldown, no actions, and no status write. The same unchanged candidate does not
enter an immediate retry loop; later candidates remain eligible. A permitted
provider retry reuses the exact prepared request instead of shortening it again.
Recovery rejects changed decision policy, capabilities, or inconsistent saved
input/request metadata with zero additional attempts. An earlier safe attempt
can already exist and remains counted.

### Preparation limits

The deployed runtime applies these limits to each evaluated decision input and
its fitting operation, including inputs without `fit`. These are not source
parser limits or YAML-configurable budgets.

| Bound                                     | Maximum                                                                      | Implementation constant      |
| ----------------------------------------- | ---------------------------------------------------------------------------- | ---------------------------- |
| Original input bytes                      | 8 MiB (8,388,608 bytes), checked for raw keys/text and exact serialized JSON | `MAX_FIT_INPUT_BYTES`        |
| Original input nodes                      | 16,384 JSON values, including containers                                     | `MAX_FIT_INPUT_NODES`        |
| Original input depth                      | 64, with the input root at depth zero                                        | `MAX_FIT_INPUT_DEPTH`        |
| Authored fit rules                        | 128                                                                          | `MAX_FIT_RULES`              |
| Segments per fit path                     | 64, counting each field and `[*]`, including the root field                  | `MAX_FIT_PATH_SEGMENTS`      |
| Bytes per fit path                        | 2,048                                                                        | `MAX_FIT_PATH_BYTES`         |
| Expanded targets                          | 4,096 matched leaves, including targets already at their floor               | `MAX_FIT_TARGETS`            |
| Expansion traversal                       | 32,768 node visits across all rules                                          | `MAX_FIT_TRAVERSAL_NODES`    |
| Reduction turns                           | 4,096 attempted turns on non-exhausted targets                               | `MAX_FIT_TURNS`              |
| Repeated complete-request validation work | 64 MiB (67,108,864 bytes) of cumulative serialized-request measurement       | `MAX_PREPARATION_WORK_BYTES` |

Original input admission runs before expansion. Object keys count toward bytes,
not additional JSON-value nodes. Missing optional fields and empty traversed
arrays do not become targets. The compiler reports excess authored rule/path
limits at their YAML location. Evaluated admission, expansion, turn, and work
limits produce `PreparationResourceLimit`. Exhausted targets take no more turns;
an attempted turn can consume the budget without changing a target.

The 64 MiB budget counts the initial request and repeated validation requests.
The bounded writer stops at the crossing write, so a reported observed count is
not necessarily the full request size. This budget is not retained evidence size,
whole-process memory, provider usage, or a new shared retention cap.

The pinned Jev route separately checks a 256 KiB serialized request-body limit
and local token/resident-request limits. Its state plus longest-question ceiling
is 32 Ki tokens, with a 4 Ki reserve, leaving 28 Ki (28,672) estimated tokens.
The ASCII-weighted UTF-8 estimator charges 0.75 tokens per ASCII-letter byte and
one token per other byte, rounded up per serialized component. Framing, question,
criteria, and omission metadata count. This is a local heuristic, not a tokenizer
or a guarantee that Jev will accept the request.
Deterministic request-shape failures use `InvalidRequest`, with a typed criterion
count/limit when applicable. They are definition errors, not no-fit outcomes or
evidence that the provider connection is unavailable. Fitting cannot change a
question or its criteria to repair such an error.

## Diagnose and interpret results

Open the check's detail pane for an actionable error title, explanation, and
available YAML path and line/column. Definition, fitting, source, provider, and
storage errors use structured diagnostics; card statuses stay short. Errors
appear before empty or passing states, while earlier valid findings remain
visible. Repeated errors are grouped without merging distinct files or decisions.
Available technical details stay in a disclosure.
They include the diagnostic code, available decision/field/input path, typed
fitting reason, and observed local budget and maximum. Raw failure strings and
provider bodies are not displayed. An estimated token count is labeled as an
estimate.

Use **Open check file** with a diagnostic location to open that local YAML in
its default application. The action does not select an editor or promise a line
jump. Open failures appear inline. Definition and fitting errors do not link to a
session. Automatic file reconciliation clears stale errors after the definition
is repaired; a provider retry is shown only when one is actually scheduled.

Invalid files also emit
`installed_check_invalid` ERROR logs with the path, diagnostic code, and available
line/column or configuration location. Repeated unchanged errors are deduplicated.
Diagnostics omit arbitrary authored values and selected work input.

Available result fields and user-slot text remain usable when history has gaps,
text is truncated, or optional context is missing. The model receives explicit
quality limits; terminal state does not imply complete evidence. Missing result
output alone does not block a check that can use other available fields.
Missing legacy model, provider, usage, or timing metadata does not by itself
block native journal review. A bounded page is not a complete result inventory;
missing page membership does not mean that results were removed.

Unsafe identity, scope, bindings, or source order blocks the affected evidence.
An unsafe source update revokes saved assessments and pending work for its
branch and resets its window. Quality gaps preserve accepted assessments when
their saved input is unchanged.
Reading an unavailable required expression value makes that candidate not
assessable without a provider call; later candidates remain eligible. A model
answer cannot repair unsafe bindings. Partial or complete input can yield an
uncertain answer when the available evidence cannot establish the behavior or
current outcome. Relevant unfinished corrective work limits any unresolved-failure
claim.
See [check coverage](../check-coverage.md#burn-check-definitions) for evidence limits.

Finding evidence is bound to the assessment that wrote the displayed status.
When preparation was saved, **Assessed input** contains the exact fitted decision
input and **Input omissions** contains its omission report when nonempty. A later
assessment does not replace that evidence. Original full tool output is not
substituted for shortened input. Legacy assessments without saved preparation
use explicitly labeled **Original source** evidence with a limitation: it does
not prove which values were sent to the model.

A status message explains the result to the user; it is not a fix prompt or an
agent instruction. In Failed retry loop, only a `stuck` answer with probability
at least 0.95 writes failure. Every other valid answer writes success, including
`uncertain` and `stuck`/`clear` below 0.95. That success can clear an earlier
failure on the same branch. Relevant recovery can also produce a clear answer
and write success. Another branch's success does not clear a valid failure.
Explicit success covers the assessed work only; it does not prove repair, a
clear whole session, or savings.

These are authored action rules, not an implicit engine fallback. A valid
assessment advances the accepted watermark. If a different definition matches
no action, it preserves the earlier status, explanation, evidence, and
status-write time; without an earlier status, the branch remains unassessed.
Missing required evidence, skipped actions, and execution errors do not create
success. A valid uncertain answer writes success only when an authored action
selects it.

A probability threshold is author policy, not an accuracy guarantee. Do not
fabricate probabilities or lower a gate to make a diagnostic pass. Snooze changes
presentation only; it does not pause evaluation or reset activation. Definitions
use existing usage accounting and resource limits, without a new dollar-cap promise.

## Data limits

Select only what the question needs. Arguments, results, and user messages can
contain private project data. A message window does not bound the separate
`session.requests` collection. Do not commit real transcripts, credentials, or
private captures as examples.
