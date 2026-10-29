# Authoring Burn Checks

Write a check that can answer one useful question from selected agent work.
Use local status rules for recorded facts, or a model decision when the question
needs judgment. This guide covers the steps and current runtime support. The
[specification](specification.md) defines the exact language rules.

> **Experimental draft: `v1-alpha`.** Burn Checks are not yet standardized.
> Syntax, semantics, and capabilities may change, including breaking changes.
> There is no backward compatibility promise. Check the current draft before
> creating or changing a definition. Beta and stable scope are not set. Missing
> capabilities are unassigned future work, not features available today.

## Write one bounded check

1. Choose one behavior and the evidence that can prove it. Include a positive
   case, an allowed case, and an unclear case. Limit the claim to selected work.
   Missing activity does not prove that an action never happened; unknown data
   differs from a known empty collection.
2. Choose an [event](#event-selection) that the source can prove. Review its
   identity, order, completeness, and producer-version limits in
   [session coverage](../session-coverage.md#burn-check-source-facts).
3. Choose the execution form. Start with the complete
   [local status example](#local-status-checks) for a recorded tool status. For a
   model judgment, study [Over-exploring](../../.antiburn/checks/over-exploring.yml)
   or the [typed model example](#typed-questions-and-actions).
4. Review data exposure before enabling the check. A model input can include
   private messages, arguments, result text, paths, and current inventories.
   Select only what the question needs. See [data limits](#data-limits).
5. [Validate](#validate-in-layers), then [install](#identity-and-installation) the
   file. Keep unfinished drafts outside active check folders. Configure a
   [decision connection](#decision-connections) with **Run checks** off, then
   enable it when ready. Local checks need the selected configuration and base
   runtime gates too, even though they send no model requests.
6. Read the [saved result and evidence](#diagnose-and-interpret-results). Test
   uncertainty and missing data as well as the expected finding. A provider
   connection test does not prove check quality.

Over-exploring selects completed read/search results in a rolling window of 20,
with at least 10 matches before assessment. Those sizes, its 120-second cooldown,
and its question are author policy, not generic engine rules. It permits plausible
dependencies, tests, and verification. A path or read count alone does not prove
unrelated work.

The [five canonical definitions](README.md#examples) cover exploration, task
scope, instruction comparisons, skill guidance, and fast-mode trial advice. Edit
their files in `.antiburn/checks/`; the docs do not keep separate YAML copies.
All five write failure only when a declared failure label has probability at
least 0.95. Four use the exact logical complement of their failure condition for
success. This includes valid uncertain and below-threshold answers. Ignored
instructions instead requires `no_conflict` at probability 0.95 or above; an
uncertain answer writes no new result and preserves any earlier saved result.

Quote a negated condition so YAML does not read `!` as a tag. Success means only
what the authored action says about the selected evidence. It does not prove
correct work, repair, or a clean whole session. Label probability and provider
confidence are separate values; neither establishes confidence in source facts.

## Supported subset

The schema describes more than the runtime currently supports. The compiler
rejects unsupported definitions before activation, even behind `if: false`.
Do not discard unsupported fields or actions to make a definition run.

- **Triggers:** choose one family: message events (`user_message`,
  `assistant_message`, `tool_call`, `tool_result`), response cycles (`turn_end`),
  or session observations (`session_start`, `session_idle`). Entries within the
  message or session family are alternatives. Each source must prove the selected
  event; compiler acceptance does not establish source support. See
  [event selection](#event-selection).
- **Window:** only message checks support a rolling window, written as an integer
  or `{size, min}`. Minimum defaults to one; `min <= size`; maximum size is 40.
  Event alternatives, filters, and top-level conditions apply before one mixed
  window counts matching messages. Fresh attachment can seed bounded recent
  results and assess only the latest ready candidate once. Other message kinds
  and cycles are future-only. Corrections do not create another attempt. Cooldown
  defaults to `0s` for local checks and `60s` for model checks. Both use
  single-flight execution, even at zero cooldown.
- **Context:** `message` for message triggers, `turn` for `turn_end`, eligible
  `window.messages`, `session.id`, `session.requests`, `agent.name`, and observed
  project fields such as `repo.root`. Session triggers expose neither `message`
  nor `turn`. `session.start_reason` requires a configured `session_start`
  alternative; `session.idle_seconds` and `session.last_activity_at` require
  `session_idle`. Field availability still depends on the actual event. Current
  recorded `session.model` and `session.fast_mode` can be read on every family;
  they never fill past message or turn attribution. Missing values stay
  unavailable to expressions; available request text and tool fields remain
  usable when other context is partial. Frozen context carries `message.quality`
  when applicable, `session.requests_complete`, and `session.source_coverage`.
  A required missing expression can make one candidate not assessable. It does not block later
  candidates from the same session.
  Recorded generator/requester `message.model` and `message.fast_mode`, uniform
  `turn.model` and `turn.fast_mode`, and supported `message.tool.paths` come from
  immutable publications. Missing mode matches neither true nor false. Common
  filters use only the selected event's scope, never a fallback scope or Settings.
  Current `global` and `repo` instruction/skill scopes freeze during selection
  when the definition references them. Known entries can remain usable beside
  unavailable scopes; a direct read of an absent array stays unavailable.
  Recorded mode evidence includes Claude reported speed, OpenCode V2 selected
  Fast/Ultrafast catalog models, Codex requested-tier preference, and Pi model-ID
  suffix hints. `message.quality.fast_mode_evidence` identifies the qualifier.
  These hints have different limits; none establishes the actual served tier,
  endpoint, or savings. See [current inventory inputs](#current-inventory-inputs).
- **Input and expressions:** JSON literals, nested fixed-key objects/arrays,
  whole `${{ ... }}` expressions, embedded text templates, dotted paths, literal
  and dynamic bracket access, array `.length`, nested `.*` projections, and
  bounded `map` directives with nested aliases. Pure functions are `strictEquals`,
  `contains`, `startsWith`, `endsWith`, `join`, `format`, `toJSON`, and `fromJSON`.
  Operators are parentheses, `!`, `<`, `<=`, `>`, `>=`, `==`, `!=`, `&&`, and
  `||`. Conditions may be Boolean, bare expressions, or whole expressions.
  Type, phase, availability, and shared evaluation limits still apply.
- **Input fitting:** optional decision-level `fit` maps selected input paths to
  `recent` for arrays or `truncate` for strings. Paths start at a declared input
  name and support `.field` and `[*]`. Loss is explicit, ordered, and reported to
  the model and in assessment details. See [fit large inputs](#fit-large-inputs).
- **Decisions:** omit `decisions` for local status actions, or declare independent
  named blocks with one to eight questions each. A block can mix Choice, Score,
  and Noul. Choice uses a label/description map; Score uses an ordered
  description array. Both need at least two criteria. Noul has only a question,
  without authored criteria. Present empty or null `decisions` is invalid.
  Decision inputs cannot read `decisions`; local actions cannot read it either,
  even behind a false condition. **Settings → Checks → Model-based checks** selects
  one System One connection for the assessment. Selected-route admission checks
  each question type, its criterion limit, and every detail that actions read.
  See [typed questions](#typed-questions-and-actions) and
  [decision connections](#decision-connections). There is no syntax-only block
  count limit; shared preparation, retained-state, and result bounds limit work.
- **Actions:** independent status actions with literal `success` or `failure`,
  or a whole expression returning exactly one of those strings. Optional
  explanations accept fixed text, whole expressions, and embedded text templates;
  the evaluated message must be a nonblank string. The last valid applied write
  wins in authored order.
- **Presentation:** one logical filename-stem row in the native 30-day report
  cohort, with normal groups, detail, search, session navigation, and snoozes.
  Status and explanations do not grant native fix prompts, Auto Fix,
  verifier/watch support, burn estimates, or savings.

Unsupported capabilities include effective `agent.skills`, `repo.branch`,
historical instruction activation, historical effective skill inventories, and
message actions, including behind `if: false`. The desktop emits only `attached`
for `session_start`, not `new` or `resumed`. It does not admit remote, WSL, child,
or historical-only sources for new live checks. These features are not usable
today; beta and stable scope are not set.

The broader draft and schema retain some unsupported context and action forms.
Schema acceptance, compiler admission, source proof, and provider compatibility
are separate requirements. The context does not split elapsed time into
model-active and tool-only periods. Draft syntax does not promise implementation
of missing capabilities. See the [example limits](README.md#examples).

### Event selection

| Family         | Events and specific filters                                                                                                                                              | Direct root                                   |
| -------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | --------------------------------------------- |
| Message        | `user_message`, completed public `assistant_message`; explicit `tool_call` with `tools`, `paths`, `paths-ignore`; terminal `tool_result` with those filters and `status` | `message`; `window.messages` with `on.window` |
| Response cycle | `turn_end`, with `status: [completed, cancelled, error]`                                                                                                                 | `turn`, including ordered `turn.messages`     |
| Session        | `session_start`, with `reason: [new, resumed, attached]`; `session_idle`                                                                                                 | `session` event fields                        |

Every event also accepts exact `models` and literal `fast_mode` filters. They read
`message`, `turn`, or `session` for that event family. A user message has no
generator and cannot match either common filter. Turn model and mode values need
a complete, nonempty native generating-request set with a uniform value for each
field. A request without public text still counts. Model and mode are independent;
mixed models do not erase a proven uniform mode. Missing or mixed values do not
match, and filters never retry against current session selection. Use top-level
`if` with `session.model` or `session.fast_mode` to test that recorded selection.

Tool categories are `shell`, `read`, `search`, `edit`, `write`, `delegate`, `skill`,
`mcp`, and `other`. A category needs a reviewed native identity, not result text or
a collapsed metric category. An unavailable category stays absent, not `other`.
Tool-result status accepts `completed`, `error`, `cancelled`, and `unknown`; it
describes tool execution, not program success. A call is distinct from its result
and cannot use a result-status filter. Native text input stays a string in
`message.tool.arguments`, even when it looks like JSON or a shell command.

For example, this local check records when two selected messages are available:

```yaml
name: Selected message window
on:
  user_message:
  tool_result:
    status: [error]
  window: { size: 5, min: 2 }
actions:
  - type: status
    input:
      result: success
      message: "Selected messages: ${{ window.messages.length }}"
```

Only admitted user messages and error results count in this example. A window is
not a complete response cycle. Public text, calls, and results have separate
identities. Private reasoning, streaming fragments, and explicitly synthetic
OpenCode user text do not become public messages. See
[session coverage](../session-coverage.md#burn-check-source-facts) for each source's
message finality and cycle boundaries. Incomplete or unbound
cycles remain unavailable; standalone usable messages can still be assessed.

The five current native routes can provide admitted user messages, tool calls,
and tool results. Completed public assistant text is available only under the
characterized Codex, Pi, and OpenCode V1/V2 contracts, not Claude. Only Codex and
OpenCode V2 currently prove `turn_end`. Format names do not extend those claims
to every producer version or to exported, remote, WSL, or child-session sources.

Cursor Desktop uses a separate ordered snapshot `tool_result` route, with
accepted user-slot prefix context. It does not supply separate public user,
assistant, call, cycle, or session-observation triggers. Native inline main
ownership, recorded header order, exact bubble/call identities, current ready
evidence, and the characterized terminal-tool shape are required. It supplies no
requester model/mode or operation-path proof. Cursor CLI and generic stores do not
inherit this support. See the [Cursor source contract](../session-coverage.md#cursor-desktop-yaml-tool-evidence).

### Session observations

The desktop emits `session_start` once per observation attachment with reason
`attached`, including an empty source. The compiler accepts `new` and `resumed`,
but the desktop does not currently emit them. Attachment is not proof that the
native agent session was created or resumed.

`session_idle` records a persisted 120-second observation boundary. Genuine
admitted native work restarts the interval using local observation time. Exact
replay, historical baseline records, selection changes, UI actions, and notices
do not. The source must reach a usable frontier before the boundary can publish.
Lost continuity starts a fresh observation interval after recovery. Restart keeps
the attachment and emitted activity revision, so it does not repeat an idle event.

New activity cancels unfinished idle work. Claim, local start, provider dispatch,
and acceptance recheck the exact attachment and activity revision. An already
accepted idle assessment remains a record of its original boundary. Optional
`session.last_activity_at` is a recorded Unix-millisecond value, not the local
timer clock. This policy does not measure model occupancy, tool-only time, or
savings. It is independent of the native Fast mode overuse check.

### Current inventory inputs

`global.instructions` and `repo.instructions` contain observed current instruction
files with absolute paths, text, scope, and observation time. `global.skills` and
`repo.skills` contain observed configured names, declared descriptions, and paths,
not skill bodies or an effective session inventory. Each scope supplies separate
`instructions_complete`/`instructions_limits` and `skills_complete`/`skills_limits`
fields, plus an available `snapshot_revision`. Missing descriptions stay empty.
Missing arrays mean unavailable, not known empty.

The selector asks the host for a bounded snapshot only for definitions that read
these scopes, on every trigger family. Inventory conditions capture before the
condition runs. Candidate-only reads capture after event filters, conditions,
and any window minimum pass. The host checks the exact publication, source,
stored CWD, repository root, and recorded directory grants.
The candidate retains that snapshot during delayed preparation and retries.
It does not read files again or enrich old evidence with later edits.

The canonical instruction and skill checks select whole `global` and `repo`
objects. This preserves partial known entries and limits without making an
unavailable array block an unrelated correction. Current syntax cannot build a
selected-field object while omitting unavailable fields. Whole scopes can also
include the other inventory; the questions ignore it, but it still counts toward
request limits. Ignored instructions permits truncation of unused skill
descriptions through `current_global_scope.skills[*].description` and
`current_repo_scope.skills[*].description`. It preserves instruction text, scope
metadata, skill names and paths, all available requests, and selected arguments.
Its 20-result window is unchanged. Skill opportunities keeps its selected
inventory intact. Both definitions also permit selected result-content
truncation. Irreducible large input can still stop preparation. See the
[snapshot architecture](architecture.md#identity-order-and-frozen-evidence).

Current files can support a current-file mismatch, not proof of a past violation.
A recorded correction needs proof that it preceded the selected work. The request
prefix ends at the trigger; it does not place every request before every earlier
result. Skill descriptions support next-step guidance, not claims
that a skill was loaded or never used. Keep source and scope limits beside these
judgments.

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

Path patterns match whole recorded repository-relative paths with `/`
separators. `*` and `?` stay inside one segment; a whole `**` segment matches
zero or more segments. Matching is case-sensitive. Absolute paths, parent
traversal, backslashes, and other wildcard syntax are rejected. A matching
`paths-ignore` veto wins. A partial path set can prove an included path or a
veto, but cannot prove that an ignored path is absent. Known empty paths differ
from unavailable paths. Matching shares a bounded work budget and fails closed
when it runs out. See the [source limits](../session-coverage.md#captured-requester-and-operation-facts).

## Typed questions and actions

Each output name has one scalar value under `decisions.<block>.outputs`:

| Type   | Output                                                                        | Optional details under `decisions.<block>.details.<question>`                    |
| ------ | ----------------------------------------------------------------------------- | -------------------------------------------------------------------------------- |
| Choice | One declared label string                                                     | `probabilities`: raw label map; `confidence`: provider confidence                |
| Score  | Finite number from zero through the last criterion index, including fractions | `probabilities`: raw array in criterion order; `confidence`: provider confidence |
| Noul   | Finite yes-probability in `[0, 1]`, not a Boolean                             | Closed empty record `{}`; no probabilities or separate confidence                |

For example, one block can ask three questions about the same selected result:

```yaml
name: Recorded result review
on:
  tool_result:
decisions:
  review:
    input:
      result: ${{ message.tool.result }}
    output:
      verdict:
        type: choice
        question: Does this recorded result show a failure that needs review?
        criteria:
          concern: The result clearly reports a failure that needs review.
          clear: The result does not report such a failure.
          uncertain: The available evidence cannot establish the result.
      severity:
        type: score
        question: How strongly does the recorded result describe a failure?
        criteria:
          - No clear failure is reported.
          - A limited failure is reported.
          - A failure blocks the recorded operation.
      explicit_failure:
        type: noul
        question: Does the recorded result explicitly report a failed operation?
actions:
  - type: status
    if: decisions.review.outputs.verdict == 'concern' && decisions.review.outputs.severity >= 1 && decisions.review.outputs.explicit_failure >= 0.95
    input:
      result: failure
      message: Review the failure reported in this selected result.
  - type: status
    if: "!(decisions.review.outputs.verdict == 'concern' && decisions.review.outputs.severity >= 1 && decisions.review.outputs.explicit_failure >= 0.95)"
    input:
      result: success
```

The three questions travel in one request. Success means only that this authored
failure condition did not match. It does not establish whole-session correctness.
These thresholds are author policy, not model accuracy guarantees.

Use `decisions.review.details.verdict.probabilities.concern` for a Choice label
probability. Score probabilities use array access, such as
`decisions.review.details.severity.probabilities[0]`; index zero means the first
authored criterion. Provider confidence uses `.confidence` for Choice or Score.
It is separate from the selected label's probability. The selected connection
must supply each detail an action can read, including through functions,
projections, JSON conversion, or whole-object reads. Missing optional details
stay absent, not null, zero, or invented distributions. Noul has no `.confidence`
or `.probabilities` field.

Additional decision blocks use the same frozen context and captured connection.
Their inputs cannot read any decision result. The runtime evaluates and fits all
inputs before the first inference call, then executes blocks sequentially in
authored order. It saves each answer before the next block. Actions run only
after every declared answer validates. A later preparation failure sends nothing;
a later inference failure can leave incurred usage but publishes no actions.

Each decision permits at most two attempts, and only a known-safe transient
failure permits retry. Retry and restart reuse successful sibling answers and
the exact saved unfinished request. Unknown delivery blocks all further sends
for that candidate. Cooldown starts once at its first durable dispatch intent;
later blocks, retries, and resume do not extend it. Resume bypasses only that
candidate's own cooldown, not current source, configuration, or runtime gates.

## Local status checks

Omit `decisions` to evaluate status actions directly against the frozen candidate:

```yaml
name: Recorded tool error
on:
  tool_result:
    tools: [shell]
actions:
  - type: status
    input:
      result: ${{ message.tool.result.status == 'error' && 'failure' || 'success' }}
      message: "Recorded tool status: ${{ message.tool.result.status }}"
```

This check reports tool execution status, not whether the shell command passed
its tests. A completed tool can still report a nonzero program exit. Local
actions can use the supported expression functions and frozen context, but not
undeclared decision results. There is no local decision input or `fit` step.
Do not write `decisions: {}` or `decisions: null` to select local execution.

Local checks still require **Run checks** enabled and a configured
selected connection. Credential, authentication, repository pause, source,
generation, and shutdown gates still apply. A failed cached model discovery or
missing model output capability alone does not block local execution. Local
work does not run model discovery, warm-up, model preparation, transport, or
inference. It creates no provider attempt or usage record and sends no model
request. Optional **Test connection** remains a separate operation that can send
a request and incur charges.

Cooldown defaults to `0s`; an authored positive cooldown starts once at the
durable local start, including when action evaluation later fails. A filtered
top-level condition does not start it. Restart retains that start and its budget
instead of starting again. Local actions use the same claims, evidence checks,
ordered writes, atomic acceptance, and no-write rules as model checks.
An unavailable action value skips that action without blocking independent later
actions. Shared budget exhaustion instead prevents all writes, including earlier
planned writes.

The detail pane labels local evidence **Frozen local context**. These values were
available to the actions; the label does not claim that every value was read.
It is not provider input, and no model request was sent. A later no-write
assessment does not replace the earlier status message, evidence, or write time.

## Decision connections

1. Open **Settings → Checks → Model-based checks** and turn **Run checks** off
   while configuring the connection.
2. Choose **Add connection**, select a provider, and enter its required fields.
3. Select **Add connection** in the dialog. The first saved connection becomes
   selected without enabling checks. For another saved connection, choose **Use**.
4. Enable **Run checks** when ready.

The connection list keeps **Add connection** and **Use** visible. Each row's
actions menu contains **Edit** and **Remove**.
**Use** selects an existing saved connection. **Edit** opens its settings, and
**Save** updates that connection without selecting another one. Cancel and
testing do not change selection. If checks are already on, saving changes to the
selected connection or choosing **Use** can start checks immediately. Saving an
inactive connection does not change active work.

Fresh setup without a connection stays off. The master setting preserves an
existing saved preference. Malformed or unreadable saved storage keeps execution
disabled rather than selecting a default. **Remove** can delete any connection,
including the last one. Removing the selected connection turns checks off and
clears selection. It does not select another provider or erase saved results,
evidence, or usage. Shared credentials can remain in secure storage; forgetting
a saved key is a separate explicit action.

A failed operation can have saved some state before credential cleanup fails.
Use its explicit retry action to finish recovery, including when a removed
connection no longer has a row. Reopening Settings retains that action. A normal
settings read or search does not perform recovery. The editor keeps typed values
until recovery confirms the originating Save or you discard those values.
Recovery does not turn checks back on after you pause them.

YAML check cards and navigation require a discovered global or repository
definition. The Settings toggle and provider setup remain available
without definitions. If a discovered check has no configured System One provider,
its detail pane shows an actionable setup error that directs you to Settings.
No discovered definition means no placeholder check or setup error.

**Test connection** is optional. It sends synthetic input and can incur provider
charges. A successful test proves only that request, not check quality or every
output capability.

The model compatibility limits below apply to checks with decisions. Local
checks retain the selected-configuration and authentication requirements, but do
not discover or validate model capabilities.

| Route      | Required setup                                                                                                                    | Compatibility limit                                                                                                                                              |
| ---------- | --------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Jev        | TypeSafe API key; the app supplies the endpoint and `jev-latest` alias                                                            | Direct System One responses with native long context enabled. The adapter keeps a local 26-criterion Choice guard; this is not a universal TypeSafe API maximum. |
| Ollama     | Base URL and an installed decision-capable model; credential only if the server needs one                                         | Requires a compatible System One runner, verified model identity/digest, and accepted output support. An ordinary Ollama chat model does not qualify.            |
| Cloudflare | Account ID, API token, and `clef` or `clef-flash`                                                                                 | Uses the supported Workers AI System One models and Cloudflare response envelope. Other model names do not inherit this contract.                                |
| Custom     | Exact inference URL, model, and direct System One or Cloudflare-envelope response mode; credential only if the endpoint needs one | Requires a supported capability source. Unknown limits block execution. An arbitrary chat or structured-generation endpoint does not qualify.                    |

Native Jev credentials use the latest stable Jev release, currently 1.14. A new
release does not require a saved-profile migration. The saved request retains
`jev-latest`; observed served models belong to individual attempts. A safe retry
uses the same request, but the alias can resolve to a newer release. An alias
echo leaves the concrete served release and its price unknown.

The app manages model limits from supported discovery data and documented
profiles. New work replaces saved manual overrides with those automatic limits.
Historical prepared requests remain unchanged; incompatible unfinished work
stops without a new send. Ollama uses the selected installed model and does not
download updates. Its `latest` tag is not proof of the newest published model.
Cloudflare uses the selected named model route, not a guaranteed latest-release
alias. Managed provider settings do not ask for release numbers. Custom keeps
the exact model selector required by its endpoint.

For Ollama, **Find models** reads the server's installed-model list. It does not
download or run a model. Select an installed model, then use **Test connection**
to check support. An installed listing alone does not prove that an ordinary
chat model supports System One decisions.

The [provider contracts](provider-contracts.md) list independent Choice
and Score intervals. Ollama's accepted `0.40.1` System One runner profiles permit
up to 24 criteria per Choice or Score question for `tev1`, or up to 26 for
accepted `clef` and generic profiles. Other runner/version metadata does not
inherit these output claims. Verified rendering and input estimates are separate checks.

The Cloudflare adapter's accepted Clef response contract requires Choice/Score
probabilities and confidence. These are wire-format requirements, not a calibration claim.
Missing or malformed maps fail validation; the adapter does not fall back to a
label-only answer.

Both Custom modes require that strict typed System One output contract. They do
not verify a model's capacity or quality. Exact official TypeSafe and Cloudflare
routes can reuse their known numeric limits. The exact direct TypeSafe Custom
route recognizes concrete 1.13 and 1.14 with standard, header-free limits, not
native Jev long context. Other Custom endpoints remain saved but cannot run
without a supported capability source. A model name does not establish limits.
Runtime validation rejects incompatible responses, including chat-style output.
An optional credential can remain absent, but an explicit saved credential
reference must resolve. An unreadable saved credential does not authorize an
unauthenticated fallback.

There is no decision-level YAML `model` field. A supplied field fails as unknown
at its YAML location; the app does not ignore it, accept an alias, or rewrite the
file. Observed coding-agent model fields and model filters remain source facts.
They do not select the assessment model.

Language validity and connection readiness are separate. Before scheduling a
model check, the host checks every question type, its own criterion interval,
and each referenced output detail.
Label probabilities and confidence are different capabilities; neither can stand
in for the other. Score probabilities use criterion order, not Choice labels.
Noul requires yes-probability support without a separate confidence capability.
A connection mismatch pauses the
check without making its definition invalid. A compatible saved connection can
restore eligibility without a YAML edit.

A label-only Choice route needs a check that reads labels only, not
`details.<question>.probabilities.<label>`. All five canonical definitions require
label probabilities for their 0.95 gates. Missing probabilities remain
unavailable; the runtime does not fabricate a distribution or substitute
confidence. Do not lower an authored gate to make a connection fit.

Numeric limits retain their source: provider metadata, runtime metadata,
documented profiles, local guards, or unknown. Unknown required bounds block
execution; the Settings UI does not ask for guessed values. Numeric bounds do
not establish question-type, probability, or confidence support, override
authentication failure, or bypass failed discovery. Ollama's accepted runner/rendering
profiles are narrower than its minimum server-version check. The adapter checks
model identity and digest rather than inferring support from the model name.

See the [connection configuration](../../apps/desktop/src-tauri/src/jev/config.rs),
[capability types](../../crates/antiburn-local/src/analysis/jev/capabilities.rs),
[Ollama adapter](../../apps/desktop/src-tauri/src/jev_ollama.rs), and
[Cloudflare adapter](../../apps/desktop/src-tauri/src/jev_cloudflare.rs) for the
accepted contracts and local guards. These are implementation limits, not a
claim that every provider deployment has been tested live.

A model assessment captures one connection and resolves its capabilities before
input fitting. One saved preparation set binds every block request to that
connection, model/revision, response mode, capabilities, and complete decision
policy without storing credentials or the raw endpoint. A permitted retry reuses
the exact request and completed sibling answers. Incompatible saved preparation
stops without sending to a new provider. Saved complete answers can recover
publication without discovery, client construction, transport, or inference;
current configuration, authentication, source, and runtime gates still apply.

A change to the selected connection, an explicit switch, or removal of the
selected connection cancels unfinished assessments durably while preserving
definitions, accepted status, and source progress. Inactive edits and removals
leave active work unchanged. Switching back does
not revive cancelled work. A late response can still carry incurred usage, but
cannot publish a cancelled assessment. Unknown delivery does not permit an
automatic resend. Local usage stays bound to the provider and model of the
original attempt. See [routing and transitions](architecture.md#decision-routing-and-connection-transitions)
for the implementation boundaries.

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

Definition discovery remains local and active with **Run checks** off, including
at cold startup. The app reads and parses existing YAML files and watches their
installation directories to show names, invalid-file diagnostics, and paused
cards. The limits remain 256 KiB per definition and 128 files per folder, with
the existing directory-access gates. This discovery does not restore provider
configuration, discover model capabilities, catch up session sources, or run
local/model assessments. Turning checks on still requires the normal gates.

**Settings → Checks → Check files** lists enabled folders with confirmed check
files and their physical file counts, even when **Run checks** is off. The count
includes invalid and shadowed files, so it is not a count of runnable checks.
Missing, paused, empty, and uncounted folders do not appear. Reopening Checks
settings loads a fresh list. Use the folder's file-manager
button to open that exact existing container. The app does not create a missing
folder or open its parent instead.

The Checks collection and search show one row per stem. Scoped activations and
branches remain separate. Internal display/snooze identity is separate from
native detector IDs; authors do not add detector enum values.

For the repository definitions, follow the
[repository example setup](README.md#try-the-repository-dogfood-checks).
Fresh activation can seed a bounded recent tool-result window. It can assess the
latest ready candidate once; it does not assess every older window. Retained
earlier user messages can supply context with explicit history limits. Restart
resumes unchanged work; changing the effective definition creates a new generation.

Live checks attach to supported native main sessions active within 180 seconds,
under an enabled live activation for the exact environment and stored session
CWD. Remote, WSL, known forks/subagents, unknown or empty CWD, and historical-only
sources do not create new live work. Compatible admitted work still drains after
activity expires; delayed or restarted consumption does not discard it.

After the initial source receipt and session publications are consumed, startup
selects at most 40 current terminal tool results within 4 MiB in native order.
Your filters and condition select only the latest ready candidate, even without
`on.window`. Empty receipts get a durable marker. Startup coverage stays partial,
and existing observed work is not reseeded. Other attached sessions can continue
while one initializes. Both native delivery routes schedule the next page
immediately when the source reports more work. Saved claim order gives ready
sources a turn, including after restart. Active sources poll at one second; idle
attached dormant sources use a 30-second resume sweep.

Bounded recent startup is the only automatic historical seed. Later corrections
retain the original first-terminal order and do not become new attempts after activation.
This seed remains result-only for mixed message checks. User messages, assistant
messages, calls, and cycles have a separate future-only boundary; their historical
rows cannot create new triggers. Session starts and idle events use persisted host
observation boundaries. Corrections retain each event's original first eligibility
and frozen context instead of creating another attempt.

OpenCode V2 is an exception to recent snapshot seeding. For the pinned stock
`v2.0.32` source, retained rows do not prove tool completion order. The native
adapter subscribes to the registered local service before reading a SQLite
sequence boundary. It starts checks from safely admitted public events after that
boundary, not the latest 40 stored rows. Cycles also need a later observed execution
start. Request history before attachment stays
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
attachment, page, integrity, and recovery limits.

## Validate in layers

Examples use this editor modeline:

```yaml
# yaml-language-server: $schema=../schemas/burn-check.schema.json
```

When installing elsewhere, update that path or the editor's `yaml.schemas`
mapping. For `.antiburn/checks/over-exploring.yml`, use
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
node docs/burn-check/scripts/validate-checks.mjs .antiburn/checks/over-exploring.yml
```

The validator checks restricted YAML 1.2 and Draft 2020-12 structure. It accepts
standard JSON-compatible tags and rejects duplicate/non-string keys, custom
tags, anchors, aliases, merge keys, multiple documents, and non-finite values.
It does not parse function names, dynamic selectors, or expression syntax, and
does not activate a check or call a provider. The
[shared expression corpus](tests/expression-parity.json) keeps schema and
compiler expectations separate.

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

## Transform selected input

Whole expressions preserve their JSON type. Embedded expressions produce text:

```yaml
input:
  request_ids: ${{ session.requests.*.id }}
  request_text: ${{ join(session.requests.*.text, '; ') }}
  result_summary: "Tool status: ${{ message.tool.result.status }}"
  requests:
    map: ${{ session.requests }}
    as: request
    if: request.kind == 'user_message'
    value:
      id: ${{ request.id }}
      text: ${{ request.text }}
```

`.*` preserves item order and nested arrays; object projections use sorted-key
order. It does not omit missing fields. A map filters only when its explicit
condition is false. Missing collection membership, an unavailable condition, or
an unavailable selected value prevents preparation. An excluded item's unused
fields do not become required evidence. Nested maps can read outer aliases, but
aliases cannot shadow a public root, `decisions`, or an enclosing alias.

Operations on `session.requests` require `session.requests_complete: true` in the
frozen evidence. This includes indexing, length, projections, maps, and the same
operations after JSON conversion. A whole copy of available partial request
history remains usable with its quality limits. This rule does not block an
unrelated tool field or the known membership of a selected message window.
Text transforms such as `format` and `join` also retain source dependencies. When
transformed JSON text has no exact source mapping, membership operations on its
parsed value use conservative dependencies from the contributing collections.
Whole copies remain usable; conversion does not restore frozen object identity.

Dynamic brackets accept a string object key or a nonnegative integer array index.
They do not bypass closed-record validation. Use dynamic object keys only on open
JSON such as `message.tool.arguments`, `message.tool.result.content`,
or compatible `fromJSON` values. The normalized `message.tool.result` and
`message.tool.result.outcome` records still have declared fields, as does an
indexed normalized record. Functions cannot read files, environment variables,
or the network.

Use `toJSON` to embed an array or object in text. `fromJSON` keeps evidence
dependencies but creates fresh object/array identity. Two separately parsed
objects do not compare equal just because their content matches. Map aliases and
indexes retain the original selected value's identity. Missing values stay
unavailable through functions and conversions; `||` is not a missing-data fallback.
Source text containing `${{` is data and is never evaluated again.

Status values can use a whole expression, for example:

```yaml
input:
  result: ${{ decisions.review.outputs.verdict == 'yes' && 'success' || 'failure' }}
  message: "Selected answer: ${{ decisions.review.outputs.verdict }}"
```

This fragment assumes a declared `yes` Choice label. The result must be exactly
`success` or `failure`, with matching case. A Boolean, other string, or embedded
result template is invalid. An invalid status result or message does not apply
that action. Independent later actions can still apply, unless a shared
evaluation limit stops the whole assessment. See the
[expression rules](specification.md#8-expressions-and-input-preparation).

### Shared evaluation limits

Top-level `if` uses a separate bounded budget before window selection. One
assessment budget covers recursive input preparation across all decision blocks, when present, and
all status action planning, including conditions, results, and messages. It does not reset
for a new input leaf, nested map, or later action.

| Bound                         | Maximum                 |
| ----------------------------- | ----------------------- |
| Recursive depth               | 32, starting at one     |
| Evaluation steps              | 10,000                  |
| Visited items                 | 1,000                   |
| Cumulative generated bytes    | 8 MiB (8,388,608 bytes) |
| Cumulative materialized bytes | 8 MiB (8,388,608 bytes) |

Fixed container entries consume visited items before traversal. Fixed text and
container construction consume generated bytes. Source and alias copies consume
the separate cumulative materialization allowance before cloning; repeated leaves
cannot each claim a fresh allowance. These counters do not receive a refund when
temporary values are dropped. Expression
evaluation reports exhaustion as `EvaluationLimitExceeded`; the input preparation
boundary records `PreparationResourceLimit`. Exhaustion stays in effect, and no
action writes status for that assessment. An action-planning limit can occur
after a provider attempt and does not undo its usage. Saved preparation carries
the consumed budget into recovery and action planning instead of granting a fresh
budget. Action replanning restores that saved snapshot rather than charging
abandoned plans again. These limits are not YAML fields, source parser bounds, fitting limits,
or provider request limits.

Local checks use the same action budget and completeness rules without a
decision-input or provider preparation step. Their saved local start must contain
a valid budget; missing or invalid local state stops that candidate without a
provider call or a fresh budget.

## Fit large inputs

A small result window can still contain large tool output. Omitting `fit` is
valid and does not trigger a warning. The app explains input-size failures only
after an actual preparation or provider failure; it does not predict one from
missing rules. Use `fit` only where
the question can tolerate missing evidence. Rules select the evaluated `input`
values, not context roots or YAML source paths. For example, this fragment
permits only result-text shortening:

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
omitted activity. The five canonical definitions do not use `recent`; they keep
the selected result records and permit loss only at their declared string paths.

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
fits. This fixed algorithm has no expressions or author-defined priorities. It
adds no automatic loss or model summary calls and cannot change fixed questions or criteria.
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

If local request validation still fails after the permitted changes, preparation
stops with typed `NoFit` and writes no new status. Any earlier saved result stays
unchanged. The detail distinguishes no rules, no matching targets, targets at
retention/marker limits, and exhausted reductions. A separate
`PreparationResourceLimit` means preparation stops at a
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

| Bound                                 | Maximum                                                                                              | Implementation constant      |
| ------------------------------------- | ---------------------------------------------------------------------------------------------------- | ---------------------------- |
| Original input bytes                  | 8 MiB (8,388,608 bytes), checked for raw keys/text and exact serialized JSON                         | `MAX_FIT_INPUT_BYTES`        |
| Original input nodes                  | 16,384 JSON values, including containers                                                             | `MAX_FIT_INPUT_NODES`        |
| Original input depth                  | 64, with the input root at depth zero                                                                | `MAX_FIT_INPUT_DEPTH`        |
| Authored fit rules                    | 128                                                                                                  | `MAX_FIT_RULES`              |
| Segments per fit path                 | 64, counting each field and `[*]`, including the root field                                          | `MAX_FIT_PATH_SEGMENTS`      |
| Bytes per fit path                    | 2,048                                                                                                | `MAX_FIT_PATH_BYTES`         |
| Expanded targets                      | 4,096 matched leaves, including targets already at their floor                                       | `MAX_FIT_TARGETS`            |
| Expansion traversal                   | 32,768 node visits across all rules                                                                  | `MAX_FIT_TRAVERSAL_NODES`    |
| Reduction turns                       | 4,096 attempted turns on non-exhausted targets                                                       | `MAX_FIT_TURNS`              |
| Shared fitting work across all blocks | 64 MiB (67,108,864 bytes), including input measurements and complete-request construction/validation | `MAX_PREPARATION_WORK_BYTES` |

Original input admission runs before expansion. Object keys count toward bytes,
not additional JSON-value nodes. Missing optional fields and empty traversed
arrays do not become targets. The compiler reports excess authored rule/path
limits at their YAML location. Evaluated admission, expansion, turn, and work
limits produce `PreparationResourceLimit`. Exhausted targets take no more turns;
an attempted turn can consume the budget without changing a target.

The shared 64 MiB budget counts every block, every reduction turn, and each
initial, repeated, and accepted request. Its consumed count is saved and immutable.
The bounded writer stops at the crossing write, so a reported observed count is
not necessarily the full request size. This budget is not retained evidence size,
whole-process memory, provider usage, or a new shared retention cap.

Validation of saved immutable requests has a separate 64 MiB pass shared by all
blocks. It does not fit, change, or replace requests. It is not another fitting
allowance and does not consume the saved fitting remainder.

The complete assessment also has aggregate retained-state limits:

| State                         | Maximum                                                                                                                       |
| ----------------------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| Full ready preparation record | 8 MiB (8,388,608 bytes), including the tag, profile, names, original inputs, fitted inputs, all requests, reports, and budget |
| Ready-record structure        | 65,536 JSON values; depth 64 with the complete root at one                                                                    |
| Complete typed results        | 512 KiB (524,288 bytes); 65,536 JSON values; depth 64 with the complete root at one                                           |

Admission checks each block before retaining it. Before inference, the runtime
checks worst-case result capacity for exact names, the longest Choice label,
bounded numeric encodings, and all optional details. It checks actual results
again before saving. These bounds can stop many valid small blocks without
adding a syntax-only block limit. Strict saved decoders reject duplicate keys,
unknown versions, invalid budgets, and oversized records. Incompatible pending
records stop locally; a bounded read-only decoder preserves accepted historical
input and provenance without authorizing a send.

The selected route separately checks its complete serialized request, token
estimate, response, and resident-memory bounds. Native Jev sends
`TypeSafe-Danger-Zone-Long-Context: true` on each request. Its total and
state-plus-longest-question ceilings are both 262,000 estimated tokens, with a
4,096-token reserve, leaving 257,904 estimated tokens. It uses a 384 KiB
serialized request-body guard and an 832 KiB resident request-charge guard.
The shared request-charge budget remains 4 MiB, not a whole-process memory cap.
Admission and transport each have a separate 60-second deadline.
The ASCII-weighted UTF-8 estimator charges 0.75 tokens per ASCII-letter byte and
one token per other byte, rounded up per serialized component. Framing, question,
criteria, and omission metadata count. This is a local heuristic, not a tokenizer
or a guarantee that Jev will accept the request. Other routes use their own
resolved capabilities and reserves; they do not inherit these native Jev limits.
The header does not bypass local original-input, fitting-work, or
saved-preparation limits. Settings does not expose manual limit overrides.
Deterministic request-shape failures use `InvalidRequest`, with a typed criterion
count/limit when applicable. They are not no-fit outcomes or evidence that the
provider connection is unavailable. A selected-route criterion mismatch directs
the user to a compatible connection in Settings; it does not make otherwise
valid YAML invalid. Fitting cannot change a question or its criteria.

## Diagnose and interpret results

Provider setup errors belong to discovered checks. With no configured System One
provider, open that check's detail and follow its instruction to configure
**Settings → Checks → Model-based checks**. The app does not create a placeholder
error when no definition is discovered.

Open the check's detail pane for an actionable error title, explanation, and
available YAML path and line/column. Definition, fitting, source, provider, and
storage errors use structured diagnostics; card statuses stay short. Errors
appear before empty or passing states, while earlier valid findings remain
visible. Repeated source errors appear once, while distinct files and decisions
keep their own diagnostics. A healthy capture does not show an alert for an older
history gap. That gap still limits the evidence available to a check. File-backed
agents use bounded file updates; they do not need an OpenCode service connection.
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
`installed_check_invalid` ERROR logs with the diagnostic code and available numeric
line/column. Repeated unchanged errors are deduplicated. Open the check detail for
its file and configuration location. Logs omit paths, authored names and values,
and selected work input.

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
Reading an unavailable required decision-input expression makes that candidate
not assessable without a provider call; later candidates remain eligible. A model
answer cannot repair unsafe bindings. Partial or complete input can yield an
uncertain answer when the available evidence cannot establish the behavior or
current outcome. Relevant unfinished corrective work limits any unresolved-failure
claim.
See [check coverage](../check-coverage.md#burn-check-definitions) for evidence limits.

Finding evidence is bound to the assessment that wrote the displayed status.
For each decision with saved model preparation, **Original input** shows the
evaluated input. **Assessed input** shows the exact fitted input. **Input omissions**
shows the omission report when it is nonempty. A later assessment that writes no
status does not replace this evidence. Original full tool output is not
substituted for shortened input. Historical accepted single-question preparation
retains its saved fitted input, omissions, and available provenance without
inventing an unsaved original input. Legacy assessments without saved preparation
use explicitly labeled **Original source** evidence with a limitation: it does
not prove which values were sent to the model.

Local assessments instead show **Frozen local context** with **Raw local
context** for its structured data. The frozen candidate remains bound to the
exact status-writing assessment. This is available context, not a record of every
field read, and not model input. Local evidence has no input-omission report,
decision connection, or missing-model-input warning.

When saved connection provenance is available, **Decision connection** identifies
the provider, model, and available model/connection revisions for that assessment.
It does not read the current Settings connection or expose its endpoint or
credential. A coding-agent model recorded in session evidence is a separate fact.

A status message explains the result to the user; it is not a fix prompt or an
agent instruction. Over-exploring fails only for `likely_excess` with label
probability at least 0.95. Its complementary success action accepts every other
valid answer, including `uncertain` and below-threshold answers. Ignored
instructions uses separate gates: its two conflict labels can write failure,
while only `no_conflict` at probability 0.95 or above writes success. Unmatched
answers preserve the saved status. A success can clear an earlier failure on the
same branch, but cannot clear another branch's valid failure. Explicit success
covers the assessed work only; it does not prove repair, a clean whole session,
or savings.

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

Select only what the question needs. Arguments, results, public messages, paths,
and instruction or skill inventories can contain private project data. A model
check sends its selected inputs to the configured provider. Local status checks
send no model request. **Test connection** is separate and can send chargeable
synthetic input.

Collect declared skill descriptions, not skill bodies. A selected tool result
can still contain file text. A message window does not bound the separate
`session.requests` collection. Do not commit real transcripts, credentials, or
private captures as examples. Do not put private reasoning in model inputs. Keep
provider inputs and responses, credentials, findings, evidence, and native
record/call IDs out of first-party analytics.

A new public or host event and its published source context must fit within
4 MiB, 262,144 JSON values, and depth 64. A cycle has at most 4,096 messages and
4,096 exact member bindings, within the same byte bound. Smaller native source
bounds still apply. These admission limits are separate from expression,
preparation, and provider limits; schema validation does not check them. See
[public-event admission](specification.md#52-public-event-admission-bounds).
