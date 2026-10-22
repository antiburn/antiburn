# Burn Checks specification — v1-prealpha

**Audience:** authors, parser/compiler implementers, and source-adapter owners.
**Scope:** the current draft language for authored Burn Checks. MUST, MUST NOT,
and SHOULD define precise requirements within this draft; they do not promise
stability or support for every capability.

> **Experimental draft: `v1-prealpha`.** Burn Checks are not yet standardized.
> Syntax, semantics, and capabilities may change, including breaking changes.
> There is no backward compatibility promise. A schema-valid definition can use
> features the runtime or a source adapter cannot execute. See the
> [supported subset](authoring.md#supported-subset).

## 1. Version and compatibility

The specification label is exactly **`v1-prealpha`**. It identifies this draft,
not a YAML field or an internal serialization version. A check MUST NOT add a
YAML `version` or `$schema` instance field. The JSON Schema dialect is
Draft 2020-12; it is separate from the specification label.

The schema is local at
[`schemas/burn-check.schema.json`](schemas/burn-check.schema.json). It has no
hosted `$id`; authors and editors MUST use the checked-in asset or a verified
equivalent. A runtime MUST use this asset or a verified equivalent and test
structural parity. Editor validation does not activate a definition.

The [authoring guide](authoring.md#supported-subset) states implemented capabilities. Runtimes
MUST reject unsupported definitions before activation and MUST NOT silently
remove unsupported fields or actions.

Before changing the contract, maintainers MUST review the impact on authored
definitions, source/provider capabilities, execution, and persisted state. They
MUST coordinate updates to this specification, schema, code, examples, and
coverage documents when affected. Decide any migration or version-selection
work from that impact review. This draft is not a frozen standard and makes no
backward compatibility promise. Its label does not change internal serialization
versions.

This specification distinguishes four validation responsibilities:

1. YAML parsing enforces the restricted YAML data model.
2. JSON Schema validates document structure.
3. The compiler validates expressions, references, types, relationships, and
   release capabilities.
4. Runtime execution validates evidence, limits, model results, and state
   transitions.

Passing one layer MUST NOT be represented as passing a later layer.

## 2. Discovery and identity

The runtime discovers immediate `~/.antiburn/checks/*.yml` or `*.yaml` files
and immediate YAML files in a known native repository's `.antiburn/checks/`
folder. For a nested session CWD, use the deepest matching known repository
root for definition lookup. Keep the activation bound to the exact stored
environment/session CWD. Discovery MUST NOT search recursively or follow an
agent's changed working directory.

The filename stem is the logical ID. It MUST match `[a-z][a-z0-9-]*`. The YAML
`name` is display text, not identity. A project definition replaces a global
definition with the same ID; definitions MUST NOT merge. Two files with the
same stem in one scope, such as `.yml` and `.yaml`, invalidate that ID.

An invalid or paused project override remains selected and MUST NOT activate a
shadowed global definition. A valid project override can replace an invalid
global definition. With no known project root, only global definitions are
eligible. A selected unavailable project value is unavailable, not an empty
value.

An effective definition has a content revision and a durable activation
generation. Any observed effective definition or enabled-state transition
creates a new generation, including restoring older content. Revisions apply
to future triggers. A generation change cancels old pending work and prevents
stale results from writing status or claiming notices. Removing an override
may expose a global definition as a new generation. Removing all effective
definitions makes the check inactive; it does not write success. Historical
assessments remain available.

File presence MUST NOT bypass provider configuration, user pause settings,
source availability, or shared resource limits.

## 3. YAML and document structure

Authors MUST provide one mapping document containing JSON-compatible values.
The parser MUST use YAML 1.2 core scalar resolution. Mapping keys MUST be
strings. The parser MUST reject duplicate keys, custom tags, anchors, aliases,
merge keys, multiple documents, and non-finite numbers. It MUST reject
non-JSON-compatible scalar or collection values. Explicit standard YAML tags
are distinct from custom tags; a standard non-specific `!` tag is not a custom
tag.

In YAML 1.2, `on` is a string key. Parsers that resolve it as a Boolean MUST
NOT be used for this contract.

Root fields:

| Field         | Required | Meaning                                                                       |
| ------------- | -------- | ----------------------------------------------------------------------------- |
| `name`        | Yes      | Nonblank display name                                                         |
| `description` | No       | Nonblank fixed description                                                    |
| `on`          | Yes      | Event-family mapping with at least one event and optional `window`/`cooldown` |
| `if`          | No       | Pre-scheduling Boolean or expression condition                                |
| `decisions`   | No       | One or more named model decisions when present                                |
| `actions`     | Yes      | Nonempty ordered list of status and/or message actions                        |

Unknown keys MUST be errors at every defined level. Names for decisions,
inputs, outputs, Choice labels, and aliases MUST match
`[a-z][a-z0-9_]*`. Fixed text fields MUST be nonblank. Fixed text fields MUST
NOT contain `${{` interpolation.

Decision entries contain `model`, `input`, and `output`, with an optional `fit`
mapping. Inputs are nonempty maps. `fit` authorizes loss only at selected paths
in that decision's evaluated input; see [input fitting](#85-input-fitting).
Outputs are nonempty maps with at most eight questions per decision.
There is no fixed limit on the number of independent decision blocks. Actions
may repeat and may be identical.

The schema's `default` annotations are documentation. A parser or compiler
MUST apply the specified defaults; JSON Schema validation MUST NOT mutate the
runtime configuration.

## 4. Events and filters

`on` is a mapping, not a string or array. An event entry accepts `null` or an
object with only that event's defined filters. An event with no filters may be
written as `event: {}` or `event:`.

One check selects events from exactly one family:

| Family         | Event keys                                                      |
| -------------- | --------------------------------------------------------------- |
| Message        | `user_message`, `assistant_message`, `tool_call`, `tool_result` |
| Response cycle | `turn_end`                                                      |
| Session        | `session_start`, `session_idle`                                 |

Different message event entries are alternatives. Multiple session event
entries are alternatives. If an entry has multiple filters, all filters in
that entry MUST match. Array values are nonempty unique alternative lists.

| Event               | Meaning                                                | Direct context                                                                        | Specific filters                           |
| ------------------- | ------------------------------------------------------ | ------------------------------------------------------------------------------------- | ------------------------------------------ |
| `session_start`     | A new, resumed, or attached observation starts         | `session.start_reason`                                                                | `reason: [new, resumed, attached]`         |
| `session_idle`      | The configured observation policy emits an idle period | `session.idle_seconds`, optional `session.last_activity_at`, optional measured period | None                                       |
| `user_message`      | The host accepts a distinct message in the user slot   | `message`                                                                             | None                                       |
| `assistant_message` | A public assistant message completes                   | `message`                                                                             | None                                       |
| `tool_call`         | The agent requests a tool operation                    | `message`                                                                             | `tools`, `paths`, `paths-ignore`           |
| `tool_result`       | A tool operation has a final observed result           | `message`                                                                             | `tools`, `paths`, `paths-ignore`, `status` |
| `turn_end`          | The host proves the end of a response cycle            | `turn`                                                                                | `status`                                   |

`tool_result.status` reads `message.tool.result.status` and accepts
`completed`, `error`, `cancelled`, and `unknown`. `turn_end.status` reads
`turn.status` and accepts `completed`, `cancelled`, and `error`. A completed
command does not prove its tests passed.

### 4.1 Common model and mode filters

Every event may use `models` and `fast_mode`. They read the selected trigger's
scope: `session` for session events, `message` for message events, or `turn`
for `turn_end`. `models` contains exact, case-sensitive `provider/model`
identifiers. The first slash separates the provider; later namespace segments
are allowed. Segments MUST be nonempty and contain no whitespace. `fast_mode`
is a literal Boolean.

An absent, mixed, unsupported, or unknown value does not match either
`fast_mode: true` or `fast_mode: false`, and does not match a model list. The
runtime MUST report the reason. It MUST NOT retry matching against a different
scope or use current settings as historical attribution. A user message has
no generating model and does not match these filters. Authors can use an
explicit top-level condition to test a current session setting.

Use only observed provider/model and service-tier evidence. A provider may be
derived from a reviewed fixed route. A configured tier is not proof of the
effective tier if the provider can override it. Missing is never `false`.

### 4.2 Tool and path filters

`tools` accepts normalized categories: `shell`, `read`, `search`, `edit`,
`write`, `delegate`, `skill`, `mcp`, and `other`. The original host tool name
is separate.

The current supported subset accepts only `shell`, `read`, `search`, `edit`,
and `other`. The compiler MUST reject `write`, `delegate`, `skill`, and `mcp`
before activation, including behind a false condition, because the production
adapter does not emit them. The draft schema retains these categories; schema
validation alone does not establish runtime support. The adapter maps native
`Bash`/`Test` to `shell`, `Edit` to `edit`, `Read` to `read`, `Search` to `search`,
and `Other` to `other`. It does not infer additional categories from tool text.

Path patterns match recorded repository-relative paths with `/` separators,
without a leading `./`. Matching is case-sensitive on every platform. `*`
matches within one path segment; `?` matches one character within a segment;
`**` is a whole segment matching zero or more segments. The entire path must
match. Other characters are literal. Reject absolute paths, `..` segments,
backslashes, and unsupported wildcard syntax.

`paths` requires at least one recorded path to match. `paths-ignore` vetoes an
event if any recorded path matches; the veto wins when both filters are set.
When a path filter is set and paths are unavailable, the event does not match
and the runtime records why. Paths outside the repository are not repo-relative
and MUST NOT be inferred from command text. A known empty path list cannot
match `paths` and does not trigger a `paths-ignore` veto.

### 4.3 Authority and response-cycle boundaries

A `user_message` is content accepted in the user slot. A literal human,
extension, or API can send it; admission does not prove literal-human origin.
OpenCode parts marked `synthetic: true` MUST NOT enter `session.requests` in
either snapshot or journal reads. Keep real user text in mixed messages. If a
generated-only message or replacement leaves no user request, omit that message
and mark request history partial. Tool results transported in a user-role envelope
do not create a `user_message`. A question-tool answer enters
`session.requests`; it creates a `user_message` only when the host reports a
distinct user-slot message. Preserve identity so one answer is not recorded
twice. User-slot admission does not establish a linked approval or relax the
existing native detectors' human-proof requirements.

A `turn_end` payload contains the initiating input, accepted corrections,
public assistant messages, and tool call/result records in source order. It
contains no nested turn-end or session events. The adapter MUST use a proven
host response-cycle boundary. It MUST NOT infer completion from inactivity, a
stored transcript row, file modification time, or a similarly named event with
different semantics. Incomplete cycles are unavailable and prevent
assessment.

`session_idle` is a persisted observation, not a claim that a session remains
idle. Activity resets its timer. UI actions, heartbeats, and Antiburn notices
do not reset it. A queued idle candidate is cancelled when activity resumes;
unclaimed actions are suppressed. Restart MUST NOT duplicate an idle period.
The draft uses a 120-second observation policy, not a universal host idle signal.

## 5. Windows and scheduling

Only message-family checks can configure `on.window`. It selects the latest
matching normalized messages, including the triggering message. User messages
and nonmatching tool categories do not count unless selected by the event
filters.

`window: N` means `{size: N, min: 1}`. The object form requires positive
integer `size`; `min` is a positive integer defaulting to one. The runtime
compiler MUST require `min <= size`. The current runtime caps a window at
40 messages; this is a release resource limit, not a syntax reinterpretation.
Turn and session checks do not support windows.

Selection order:

1. Normalize and deduplicate source events within a session branch.
2. Apply event filters and top-level `if` before counting.
3. Append matching messages in proven source order and retain at most `size`.
4. Once `min` matches are available, freeze a candidate ending at the trigger.
5. Dispatch only when cooldown, single-flight, queue, input-size, provider, and
   user resource limits permit.

Windows roll; they do not reset after assessment. `window.messages[0]` is the
earliest retained message; `.length` is the retained count. The final record is
the triggering message. A configured window bounds only `window.messages`; it
does not bound `session.requests`, a full turn, or an instruction/skill
inventory.

Source quality is evidence for the model, not a whole-session admission gate.
The runtime preserves usable results and request text when a source omits fields,
truncates content, or reports a history gap. Known limitations appear in
`message.quality` fields, including `source_complete`, `tool_name_available`,
`category_available`, `arguments_available`, `result_content_available`,
`request_history_complete`, and `main_session`. Session context includes
`session.requests_complete` and `session.source_coverage`.
Missing values remain unavailable to expressions. A required missing expression
can make that candidate not assessable; it does not suppress later candidates
from the session. Unknown scope or identity and unusable ordering remain unsafe
to select.

A source can report a selection-segment reset when it loses continuity. The
runtime clears that branch's current window and pending candidates before it
selects later results. It keeps partial-history quality, result identity history,
cooldowns, saved assessments, and uncertain provider attempts. A reset does not
write success, authorize a provider resend, or clear unrelated branches. A source
quality gap alone is not a segment reset. It preserves frozen candidates; later
candidates retain the partial-coverage limit. An update with unsafe identity,
scope, or order revokes the affected branch's saved assessments and pending
candidates and resets its selection window. These are adapter/runtime signals,
not YAML fields or new trigger types.

During cooldown, a message window replaces its pending candidate with the
latest ready candidate. Replaced candidates are recorded as coalesced. Without
a window, each event is a separate bounded source-order candidate. Overflow is
recorded as skipped. A timer may dispatch an already pending candidate when
cooldown ends; it does not create a new observation. Waiting for a minimum does
not consume a model attempt.

Durations use integer strings with `s`, `m`, or `h`, such as `0s`, `60s`,
`5m`, or `1h`, without leading zeros. Model-run cooldown defaults to `60s`;
local checks default to `0s`. Cooldown starts at the first provider attempt
(or local evaluation start). Retries do not reset it. Zero cooldown does not
disable single-flight execution.

Source adapters order native records before assigning durable observation
positions. Timestamps are not a sort key. Late earlier records remain in
history with a diagnostic and do not enter frozen windows or create retroactive
actions. Deduplication, selector progress, frozen candidates, and resume
progress must recover together so replay does not publish duplicate work.

### 5.1 Live attachment and replay

Automatic live review attaches to supported active native main sessions when
checks are enabled and configured. Fresh attachment may seed a bounded recent
window and assess only its latest ready candidate once. It MUST NOT replay each
older window or traverse the complete historical backlog. The attachment marker,
including an empty seed, MUST persist with selector progress. Restart of unchanged
work MUST resume that progress without repeating the startup assessment. A new
effective definition generation establishes a new attachment boundary.

The current runtime requires an enabled live activation for the exact native
environment and stored session CWD. New scopes and cold source bindings require
a supported main session active within 180 seconds. Remote, WSL, known forks or
subagents, unknown/empty CWD, and historical-only sources do not create new live
work. Compatible unfinished admitted work MUST continue after activity expires.
Delayed consumption MUST NOT reclassify admitted publications by a new activity
cutoff.

Initial publications advance source delivery without creating candidates until
the branch attaches. After its initial source receipt and publications are
consumed, startup selects at most 40 current terminal observations within 4 MiB
in native order. Authored filters and conditions apply before freezing the latest
ready candidate. A check without `on.window` MUST also create only this latest
startup candidate; later non-window live candidates remain separate. Startup
coverage MUST remain partial. Existing observed work MUST NOT be reseeded.

After attachment, only new safely admitted first-terminal results trigger work.
Pending/running mutations can advance their source position. Their first terminal
position fixes logical result order; later corrections retain it and the frozen
request context. A correction MUST NOT become a new attempt merely because its
publication follows activation. Deduplication and stale-removal checks apply
across pages and restart. An unknown historical membership cannot be recovered
by guessing from the current rolling window.

Both native delivery routes schedule productive continuation immediately and
use durable fair rotation across restart. Active sessions and changed cursors
get a one-second poll; idle attached dormant sources use a 30-second resume sweep.
Disabled and ineligible cold sources MUST NOT create claims or idle wake loops.

Bounded startup and incremental live reads can omit history. Their source-quality
fields MUST disclose that limit. A live cursor's bounded identity and boundary
checks MUST NOT be described as a full consumed-prefix integrity audit. The
strict replay path can validate historical evidence under its own source
contract; it MUST use isolated state and MUST NOT alter live status. See
[source delivery](../session-coverage.md#burn-check-source-facts) for the exact
reader bounds and integrity limits. Neither path proves unrecorded activity or
extends accepted producer versions.

## 6. Context and data model

Values are JSON-compatible: null, Boolean, finite number, string, array, or
string-keyed object. Timestamps are RFC 3339 strings. Optional values are
absent when not observed; they are not represented by guessed strings, zero,
false, or fabricated nulls.

### 6.1 Trigger and collection roots

| Root or field                          | Type and meaning                                                                                                         |
| -------------------------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| `message`                              | Current normalized record for a message event                                                                            |
| `turn`                                 | Current response-cycle record for `turn_end`                                                                             |
| `window.messages`                      | Flat array of retained matching records; available only to decision input and actions on message checks with `on.window` |
| `session.id`                           | Required session identity                                                                                                |
| `session.requests`                     | Ordered user messages and supported request/answer records through the trigger, with collection completeness tracked     |
| `session.requests_complete`            | Whether the source proves request history complete through the trigger                                                   |
| `session.source_coverage`              | `complete` or `partial` for published source membership at the trigger                                                   |
| `session.start_reason`                 | `new`, `resumed`, or `attached` on `session_start`                                                                       |
| `session.idle_seconds`                 | Observed idle duration, at least 120, on `session_idle`                                                                  |
| `session.last_activity_at`             | Optional timestamp of observed activity                                                                                  |
| `session.model`                        | Current observed model selection, not proof of past execution                                                            |
| `session.fast_mode`                    | Current observed effective mode setting when supported                                                                   |
| `session.last_activity_period`         | Optional finished measured interval associated with the idle event                                                       |
| `global.instructions`, `global.skills` | Observed instruction or skill snapshots                                                                                  |
| `repo.root`, `repo.branch`             | Observed project root and branch; either may be absent when unknown                                                      |
| `repo.instructions`, `repo.skills`     | Observed project instruction or skill snapshots                                                                          |
| `agent.name`, `agent.skills`           | Host agent name and effective available skill snapshot                                                                   |

The four context roots `session`, `global`, `repo`, and `agent` exist in every
phase. Individual values and inventories can still be unavailable. With no
known project, repo collections are unavailable. A known project with a
completed supported inventory containing no entries has a known empty array.
An unsupported or unfinished inventory is unavailable. Do not infer an
effective skill inventory by merging generic lists.

Global check location and global input context are separate. A global check
may select repo instructions; a project check may select global instructions.

### 6.2 Message and tool records

Message records are flat. A host record combining text and multiple tool calls
is split into distinct records with stable IDs. Public text and tool operations
are separate. Private reasoning and streaming chunks are excluded. If meaningful
non-text content cannot be represented, preserve available text and mark the
selected payload partial. Partial evidence remains available to the decision;
the runtime does not treat missing fields as empty values or block the full
session by default.

| Field                                | Type and meaning                                                                                         |
| ------------------------------------ | -------------------------------------------------------------------------------------------------------- |
| `message.id`, `message.source`       | Required stable identity in a source namespace                                                           |
| `message.occurred_at`                | Optional source time for acceptance, message completion, tool request, or final result according to kind |
| `message.kind`                       | `user_message`, `assistant_message`, `tool_call`, or `tool_result`                                       |
| `message.role`                       | `user` for user messages, `assistant` for assistant messages/calls, `tool` for results                   |
| `message.model`, `message.fast_mode` | Optional proven generating/requesting model and effective mode; absent for user messages                 |
| `message.content`                    | Public user/assistant text; absent on tool messages                                                      |
| `message.tool`                       | Required on tool calls/results; absent on user/assistant messages                                        |
| `message.tool.call_id`               | Required on calls; on results only with a proven call link                                               |
| `message.tool.name`                  | Original host tool name; required on calls and optional on results                                       |
| `message.tool.category`              | Normalized tool category; absent when unknown                                                            |
| `message.tool.arguments`             | Call arguments; on results only when the source proves the matching request                              |
| `message.tool.paths`                 | Optional known repo-relative operation paths                                                             |
| `message.tool.result.status`         | Required on results: `completed`, `error`, `cancelled`, or `unknown`                                     |
| `message.tool.result.content`        | Observed result value; absent if the host does not expose it                                             |
| `message.tool.result.outcome`        | Optional source-reported program or child-work outcome; separate from tool execution status             |
| `message.tool.provider_executed`     | Optional source flag that the provider executed the tool rather than the local host                     |
| `message.tool.result.output_bytes`   | Optional observed result size after host truncation; does not prove completeness                         |
| `message.quality`                    | Optional source-quality facts, including missing arguments, result content, or source gaps               |

Arguments unavailable from a host are absent, not `{}`. Missing call
arguments or result content make the selected tool message partial. A proven
linked result with unavailable arguments remains eligible for assessment, and
the model receives the available fields and quality facts.
An orphan result without a proven call link omits `call_id` and `arguments` by
design. Empty known output differs from unavailable output. Host truncation is
recorded as input-quality metadata. The schema does not promise diffs, parsed
test outcomes, filesystem state, or child-session trees.
An unavailable category stays absent, not `other`. An unfiltered result trigger
can use the remaining fields; a category filter cannot match an unavailable
value. A recorded tool name can normalize to native `Other`, including when
the normalizer has no more specific class. This is distinct from missing category
evidence and does not prove a draft `write`, `delegate`, `skill`, or `mcp` class.

When OpenCode V2 records them, `result.outcome` carries a `kind` and native facts.
`shell` can include `state`, `exit`, `signal`, `timed_out`, and `shell_id`.
`code_mode` can include `error`. `subagent` includes `session_id` and a recorded
work state. A completed tool with shell exit `7` still has result status
`completed`. A background launch can complete while its work remains `running`.
Missing facts stay unavailable. These fields do not prove task correctness or
create separate results for nested Code Mode activity. They add source facts;
they do not add YAML syntax, triggers, or status values.

### 6.3 Requests, instructions, skills, and attribution

Request records have `id`, `source`, `kind` (`user_message`, `question_answer`,
or `approval`), and `text`; `in_reply_to` is optional and requires a proven
link. Assistant and tool text do not become user messages. An approval without a
proven linked question cannot authorize an inferred proposal.

The current runtime supplies available user-slot text through the trigger in
`session.requests`, including extension/API input. It does not require
literal-human origin. OpenCode synthetic parts remain excluded under section 4.3.
Source-specific prefixes, main-session membership, order,
bindings, and completeness metadata still apply; incomplete history does not
discard known text or block an otherwise useful tool-result review. See
[session coverage](../session-coverage.md#burn-check-source-facts). Broader
request/answer forms in this draft do not imply runtime support.

Instruction records contain an absolute `path`, `content`, `scope`, and
`observed_at`. Scope has an absolute root or `null`, and `globs`. A null root
means global scope and requires an empty glob list. Current instruction files
do not prove which rules applied to past actions. If scope cannot be
represented, the instruction record is incomplete.

Skill records contain `name`, declared `description`, and absolute `path`.
Missing declared descriptions are empty strings. Selecting a skill list does
not read its files. Current availability does not prove past availability.

Model and mode fields retain the same type at every scope. User-message records
have no generating model. Tool results use proven requester attribution, not a
model named in tool output. A turn or activity period exposes a singular model
or mode only when its complete nonempty request set has one proven uniform
value for that field. Derive model and mode independently. Never fill
historical attribution from current session settings.

### 6.4 Turn and measured activity records

A turn has `id`, `source`, required `status` (`completed`, `cancelled`, or
`error`), ordered `messages`, and optional proven `started_at` and `ended_at`.
It may have `model` or `fast_mode` only under the uniform-attribution rule.
Turn messages use the same record shape as current messages and windows.

`session.last_activity_period` is a bounded historical measurement, not
whole-session totals or a live timer. Its fields may include `started_at`,
`ended_at`, uniform optional `model`/`fast_mode`, optional `completed_turns`,
and nonnegative finite durations `active_seconds`,
`model_in_flight_seconds`, `tool_only_seconds`, `other_seconds`, and
`unknown_seconds`.

Measurement requires complete operation membership and interval coverage.
Count overlapping operations once; model-in-flight time takes precedence over
tool-only time. Exclude inactive waits and proven human-wait suspension. An
unfinished operation, missing boundary, clock discontinuity, lost membership,
or possible hidden active gap makes affected duration data unavailable, not
zero. A known active interval with unknown cause belongs in
`unknown_seconds`. Transcript timestamp gaps and token counts cannot establish
this partition. Tool-only time is not proof of a bottleneck or counterfactual
speedup.

## 7. Decisions and provider results

Each decision has a literal `provider/model` identifier, nonempty `input`, and
nonempty `output`. A question has one of these forms:

| Type   | Configuration                                                                                    | Result                                              |
| ------ | ------------------------------------------------------------------------------------------------ | --------------------------------------------------- |
| Choice | Nonblank `question` and `criteria` mapping of at least two named labels to nonblank descriptions | One declared label string                           |
| Score  | Nonblank `question` and ordered `criteria` array of at least two nonblank descriptions           | Finite number from zero to the last criterion index |
| Noul   | Nonblank `question`                                                                              | Finite yes/no probability from zero to one          |

Every decision block uses the same frozen trigger/context. Decision inputs
cannot read other decision outputs. Mapping order creates no dependency.
Prepare every block before provider calls and complete all blocks before
actions. If any block is unavailable, invalid, or fails, the check is not
assessed and no actions run. Independent calls may execute concurrently within
shared resource limits.

The currently supported `typesafe/jev-1.13.0` route accepts 2–26 Choice criteria.
The compiler rejects excess criteria before activation with a located diagnostic.
This is a route limit, not a general draft-language maximum.

Provider capabilities declare whether Choice probabilities, Choice/Score
confidence, or Score distributions are available. Reject references to
unsupported details before scheduling. Never invent probabilities from a
label or substitute self-reported confidence. Choice/Score probability arrays
must have exactly the expected labels/length, finite values in `[0,1]`, and
sum to one within a documented, tested adapter tolerance. Reject extra/missing
results and invalid scores. Noul requires a supported yes/no probability
result, not a converted Boolean.

An `uncertain` Choice label is ordinary authored output. It is not an
execution state and has no implicit action behavior. A probability threshold
is an author policy, not an accuracy claim.

## 8. Expressions and input preparation

Expressions use a documented subset inspired by GitHub Actions. This is not
JavaScript and not a complete GitHub Actions runtime. Expression syntax is
validated by the runtime compiler; the local structural validator checks
wrappers and schema shape only.

### 8.1 Templates and phases

A scalar containing exactly one `${{ expression }}` preserves the expression's
type. An expression embedded in text produces a string. Nested YAML objects
and arrays evaluate recursively. Object keys are fixed strings, not templates.
Evaluate templates once; `${{` inside source data is never evaluated again.

In text, null becomes an empty string; Booleans become `true` or `false`; and
finite numbers use decimal/exponent text. Arrays and objects require `toJSON`.
Unavailable values are errors, not null or empty strings. A complete message
template must be a nonblank string.

Only decision input values and action message text support substitution. Names,
descriptions, models, filters, questions, and criteria are fixed text.

| Phase                                              | Available roots                                                                                       |
| -------------------------------------------------- | ----------------------------------------------------------------------------------------------------- |
| Top-level `if`                                     | `session`, `global`, `repo`, `agent`; current `message` for message triggers or `turn` for `turn_end` |
| Decision input                                     | Same roots; `window` only for message checks with `on.window`; local map aliases                      |
| Action `if`, `input.result`, and message templates | Trigger roots; eligible `window`; `decisions` when configured and valid                               |

Top-level `if` runs before window history is built and cannot read `window`.
Session triggers expose neither `message` nor `turn`. A condition is a YAML
Boolean, a whole expression, or a bare expression string. Quote a bare
expression beginning with `!` so YAML does not parse it as a tag. A false or
unavailable top-level condition filters the event with a diagnostic. An
unavailable decision input prevents the decision. An unavailable action
condition skips that action without skipping independent valid actions.

### 8.2 Access, operators, and comparison

Support property access `.name`, literal bracket access `['name']`, array
index `[0]`, parentheses, unary `!`, `<`, `<=`, `>`, `>=`, `==`, `!=`, `&&`,
and `||`. Dynamic brackets accept a string object key or nonnegative integer
array index. Read own JSON keys only. Missing fields, out-of-range indexes,
negative indexes, slices, and prototype access are unsupported or unavailable
as appropriate. Array `.length` is a nonnegative integer; object `.length`
reads an actual key; strings have no `.length` extension.

String literals use single quotes. Double quotes may delimit YAML but not an
expression string. Escape a single quote by doubling it. Boolean, null, and
finite JSON number literals are supported; hexadecimal expression numbers are
not.

Precedence, highest first: property/index access and function calls, `!`,
ordered comparisons, equality, `&&`, then `||`. Binary operators associate
left to right. There is no arithmetic, assignment, arbitrary code, or method
call.

Comparison behavior:

- False, null, zero, and the empty string are false-like; other available
  values are true-like.
- `&&` and `||` short-circuit and return the selected operand.
- String comparison is case-insensitive. Property and field names are
  case-sensitive.
- Equality between different types converts to numbers: null/false/empty
  string become zero; true becomes one; numeric strings parse as JSON numbers.
  Other strings, arrays, and objects convert to a non-comparable value. Equality
  with that value is false and inequality is true. Ordered comparisons are
  false.
- Same-type strings use case-insensitive lexical order; numbers and Booleans
  compare by value; null equals null. Frozen arrays/objects compare equal only
  when they refer to the same frozen value. Ordered comparisons of
  same-type arrays or objects are false.
- An unavailable value remains unavailable when evaluated. Comparing it with
  null or using `||` does not erase the missing-data state.

Unknown fields on closed normalized records are compiler errors, including
literal bracket keys. Tool arguments/results and `fromJSON` values are open
JSON subtrees; their keys are checked at runtime. Optional known fields and
dynamic source-data keys may be unavailable. Check variants before reading
variant-specific fields.

### 8.3 Functions and projections

Function names are case-insensitive. Supported pure functions are:

| Function                                             | Behavior                                                                                                                 |
| ---------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| `strictEquals(left, right)`                          | Exact primitive comparison; strings are case-sensitive; different primitive types are unequal; objects/arrays are errors |
| `contains(array, item)`                              | Membership using comparison rules above                                                                                  |
| `contains(text, value)`                              | Case-insensitive substring check                                                                                         |
| `startsWith(text, prefix)`, `endsWith(text, suffix)` | Case-insensitive string checks                                                                                           |
| `join(array, separator)`                             | Join scalar values; separator defaults to `,`; a string input is returned unchanged                                      |
| `format(template, value0, ...)`                      | Replace `{0}`, `{1}`, and so on; `{{` and `}}` escape braces                                                             |
| `toJSON(value)`                                      | JSON serialization with deterministic object-key order                                                                   |
| `fromJSON(text)`                                     | Parse finite JSON into a typed value; malformed JSON is an error                                                         |

Scalar function arguments use text conversion rules. Objects/arrays require
explicit serialization except as the array argument to `contains` or `join`.
Missing arguments and wrong types are errors. `format` requires a template and
at least one value; unmatched placeholders are errors. `fromJSON` requires a
string. Functions have no filesystem, environment, network, or process access.
GitHub-specific status functions, `hashFiles`, job matrices, and general loops
are not supported.

The `.*` projection maps the remaining access path over array items in order
and object values in sorted-key order. Nested projections retain nested arrays;
they do not flatten. A missing selected field fails the projection rather
than dropping an item. Use `map` with an explicit `if` for intentional
filtering.

### 8.4 Bounded `map` input directive

Within decision input values only, an object containing `map` is a directive
with exactly these keys:

| Key     | Requirement                                             |
| ------- | ------------------------------------------------------- |
| `map`   | Required whole expression resolving to an array         |
| `as`    | Required identifier for the current item                |
| `if`    | Optional Boolean or expression evaluated before `value` |
| `value` | Required nested input template                          |

The alias is scoped to that directive's `if` and `value`, not its collection
expression or siblings. Nested directives can read outer aliases and preserve
nested arrays. Aliases MUST NOT shadow public roots (`session`, `turn`,
`message`, `window`, `global`, `repo`, `agent`), `decisions`, or an enclosing
alias, even when a root is unavailable in the current phase. An alias named
`messages` is allowed unless it shadows an enclosing alias.

An object with `map` is always a directive and accepts no extra keys. To
represent a literal object with that key, use a whole `fromJSON` expression.
An intentionally false item condition excludes an item. An unavailable
condition/value fails preparation; it does not silently skip the item. An
empty known collection yields `[]`.

Collection membership completeness is separate from item-field completeness.
Projection, indexing, length, and mapping require known membership. A map
depends on membership, each evaluated condition, and each emitted value. An
excluded item's unused fields do not affect completeness. Included values
depend on fields read or copied; copying a whole record selects its whole
subtree. Preserve source references, original indexes, conditions, and
completeness through conversion. Mapping cannot erase a missing dependency.

The runtime enforces shared per-run expression depth, step, iteration, and
generated-byte limits. The draft's initial engineering values are depth 32,
10,000 evaluation steps, and 1,000 visited iteration items; these are runtime
limits, not schema syntax. A release MUST document its actual limits and
diagnostics. Exceeding a limit prevents assessment.

### 8.5 Input fitting

An optional decision-level `fit` mapping authorizes bounded loss from the
evaluated decision input when the complete provider request exceeds its limits.
Without `fit`, or with `fit: {}`, the runtime MUST preserve every input value.
Fitting MUST NOT change source evidence, the frozen original input, questions,
criteria, or values outside an authorized target.

For example, this decision fragment permits shortening only the result text:

```yaml
input:
  requests: ${{ session.requests }}
  activity: ${{ window.messages }}
fit:
  "activity[*].tool.result.content": truncate
```

Each key is a path rooted at a declared top-level `input` name. A path can
continue with object fields (`.field`) and array traversal (`[*]`), including
nested traversal. The top-level name uses the input identifier grammar; each
subsequent field matches `[A-Za-z_][A-Za-z0-9_]*`. Field names are case-sensitive.
For example, `activity`, `activity[*].tool.result.content`,
and `groups[*].results[*].text` select an input array, its result text, and
nested result text respectively. Paths select evaluated values, not expression
roots such as `window` or `session`. They are not expressions: interpolation,
functions, conditions, numeric indexes, quoted bracket keys, slices, recursive
descent, and object wildcards are not supported.

| Rule       | Target type | One turn                                                                                                                                                                      |
| ---------- | ----------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `recent`   | Array       | Remove one oldest item. Preserve the original order of the remaining items and retain at least one item from an originally nonempty array.                                    |
| `truncate` | String      | Remove up to 1,024 bytes of original middle text. Retain at least 256 bytes of original text, preserve a head and tail at Unicode boundaries, and insert one omission marker. |

An empty array requires no change. A string at or below its retention floor
requires no change. Retention rounds upward to Unicode boundaries, so the
remaining original text can exceed 256 bytes and a turn can remove fewer than
1,024 bytes. For example, a 1,300-byte string of four-byte emoji retains 280
original bytes after its first turn and omits 1,020 bytes.

The omission marker is exactly `\n[... omitted ...]\n`. A first `truncate` turn
MUST leave a target unchanged when the omitted substring's JSON-serialized size
is no greater than the marker's JSON-serialized size. Include JSON escaping in
this comparison. A target can therefore reach its effective limit above the
256-byte floor. Repeated `truncate` turns MUST operate on the original text,
increase the omitted middle range, and keep a single marker; they MUST NOT
truncate marker text or add nested markers. Retention and omission byte counts
refer to UTF-8 source text, not characters, JSON escaping, or marker bytes. These
rules do not authorize recursive truncation of strings inside an object or array. To shorten
such strings, select their concrete string fields with `[*]` as needed.

The compiler MUST reject an unknown top-level input, unsupported path syntax,
an unknown rule, and overlapping targets. Overlap includes the same target and
an ancestor/descendant pair, such as `activity: recent` together with
`activity[*].tool.result.content: truncate`. The runtime MUST reject incompatible
present values, including a non-object field traversal, a non-array `[*]`, a
non-array `recent` target, or a non-string `truncate` target. A present null is
not an absent property. Absent optional properties and empty traversed arrays
are skipped; they MUST NOT be fabricated or treated as invalid present values.
Path/type validation applies even when the original request already fits.

Fitting is deterministic:

1. Evaluate the input once against the frozen candidate and apply the original
   input admission limits before target expansion.
2. Validate the rules and expand their targets once, in authored YAML rule order
   and original array order. Preserve each target's original binding.
3. Validate the complete original provider request. If it fits, use it unchanged.
4. Give each concrete reducible array or string one turn in strict round-robin
   order. A rule with many matches does not turn them into one combined target.
   Exhausted targets take no further turns.
5. After each turn that changes a target, validate the complete request, including
   framing, questions, criteria, omission metadata, serialization, and provider
   resource limits.
   Stop immediately when it fits; do not finish the round or shorten another
   target.
6. If all permitted reductions are exhausted and the request still fails local
   request validation, report `NoFit`. If an admission, traversal, turn, or
   repeated-validation work limit stops preparation, report
   `PreparationResourceLimit` instead. Neither outcome authorizes additional loss.

The provider MUST receive explicit omission metadata with each changed target's
path, rule, original and retained item counts or text-byte counts. Omitted
evidence MUST NOT be represented as complete or as proof that an event did not
happen. The assessment details MUST identify the shortening and its limits.
Source-quality facts remain separate from fitting omissions.

For the supported Jev route, fitted input is at `state.evidence`. When omissions
exist, `state.input_omissions` contains this framing and a `targets` array:

```json
{
  "framing": "The listed evidence was shortened with author permission. Omitted records and text are unavailable, not negative evidence.",
  "targets": [
    {
      "path": "activity[0].tool.result.content",
      "strategy": "truncate",
      "original_count": 4096,
      "retained_count": 3072
    }
  ]
}
```

Each entry uses a concrete path with original array indexes, a `strategy` of
`recent` or `truncate`, and nonnegative integer counts. Counts mean array items
for `recent` and original UTF-8 bytes for `truncate`; retained bytes exclude the
marker. Only changed targets appear, in expansion order. With no omissions, the
provider request omits `state.input_omissions`. The persisted `FitReport` and
assessment evidence use `{"omissions": [...]}` with the same entries; they do
not use the provider's `targets` wrapper.

If no rule matches a reducible value, the runtime MUST explain that the selected
paths cannot reduce the request. It MUST NOT fall back to unrelated fields.
Initial no-fit, resource-limit, and path/type failures MUST settle before the
first provider dispatch with zero provider attempts, no new model cooldown, no
actions, and no status write. They MUST NOT immediately retry the same unchanged
candidate. Later candidates remain eligible; an earlier status remains unchanged.

The runtime MUST preserve the frozen original input separately from the prepared
provider input. A permitted known-safe provider retry MUST use the identical
prepared input, omissions, question, criteria, and request identity. Restart
recovery MUST bind the preparation profile to the complete compiled decision
policy and provider capabilities, and validate the saved request against its
input, omission report, and fixed construction. It MUST NOT silently refit an
already dispatched request under changed limits or policy. An incompatible
profile settles with zero additional attempts; an earlier safe attempt remains
in the durable attempt count.
Unknown delivery still does not authorize a resend.

### 8.6 Preparation limits and local outcomes

The current release applies the [deployed preparation limits](authoring.md#preparation-limits)
to original input size, nodes and depth, rule/path size, target expansion,
traversal, attempted turns, and cumulative serialized-request validation work.
These are release resource limits, not new YAML fields or a shared evidence
retention cap. Admission can stop before checking whether the request fits.

`NoFit` means local request validation still fails with the current permissions
and effective retention/marker limits. Its typed `no_fit_reason` is one of:

| Value                  | Meaning                                                                       |
| ---------------------- | ----------------------------------------------------------------------------- |
| `no_rules`             | No fitting rules authorize a reduction.                                       |
| `no_matching_targets`  | The configured paths match no present target values.                          |
| `targets_at_minimum`   | No target changes: retention or marker break-even limits prevent a reduction. |
| `reductions_exhausted` | At least one reduction occurs, but all permitted reductions are exhausted.    |

`PreparationResourceLimit` means local preparation stops before establishing a
fitting result. It MUST NOT be reported as exhausted fitting or proof that
further authorized reductions could not fit. Request/token/resident-size limits
are separate from these preparation-work limits. Local token counts are estimates,
not verified provider tokenizer counts, billing usage, or a provider acceptance
guarantee.

The stored failure code uses snake_case (`no_fit` or
`preparation_resource_limit`). Optional typed context identifies `decision`,
`field`, `input_path`, `no_fit_reason`, and `budget`. A budget contains `kind`,
`observed`, and `maximum`; it describes a local observation or estimate. A bounded
writer can report the crossing write count rather than the full serialized size.
Consumers MUST use these fields, not parse the failure's detail text. Old stored
no-fit records without a typed reason do not establish reductions exhausted;
the desktop reports them as `preparation-incomplete`.
Deterministic request-shape failures use `InvalidRequest` (`invalid_request`),
not `NoFit` or `ProviderUnavailable`. Criterion-count context uses budget kind
`criteria` with the observed count and route maximum. Such a failure does not
authorize fitting fixed questions or criteria.

## 9. Actions and status

| Action    | Fields                                                           | Meaning                                                                     |
| --------- | ---------------------------------------------------------------- | --------------------------------------------------------------------------- |
| `status`  | Optional `if`; required `input.result`; optional `input.message` | Write `success` or `failure`; optional text explains the status to the user |
| `message` | Optional `if` and `cooldown`; required `input.message`           | Plan user-facing text for an Antiburn or supported host UI                  |

Action conditions are independent and evaluate against the same frozen context
and decision results. Actions cannot read another action's outcome or prior
status. Apply valid status writes in list order; the last valid write sets
current status. A skipped or invalid action does not block independent later
actions. Persist every action outcome atomically. Model checks run no actions
if their decision set is invalid or incomplete. Local checks without a model
decision are valid draft syntax but require compiler/runtime support.

`input.result` is exactly `success` or `failure`, or a whole expression that
returns exactly one of those case-sensitive strings. It does not coerce
Booleans or other values. Separate actions are clearer when failure and clear
status use different probability gates. An uncertain label has no automatic
meaning; actions must explicitly leave it unmatched if that is intended.

Status belongs to a logical check ID, session branch, and activation
generation. It begins unassessed. A valid action writes success or failure.
No action, invalid action, unavailable data, or provider failure leaves the
prior status unchanged. A valid accepted assessment advances its watermark
even if all actions skip; a not-assessed result does not. An older assessment
cannot replace a newer accepted status or claim a later notice. Keep history
when status changes. Display success as **No concern in this assessment**,
not “Resolved.”

The maintained Failed retry loop example fails only for `stuck` at or above
0.95 confidence. Its success action uses the exact complement of that condition,
so uncertain and below-threshold answers explicitly write success. This is an
authored policy, not an engine fallback. For definitions with no matching action,
acceptance MUST retain the prior status-writing assessment, explanation, evidence,
and write time while advancing the accepted watermark. Without a prior write,
engine status remains unassessed. The runtime MUST NOT synthesize a success action
when no condition matches.

The desktop intentionally displays an eligible YAML check as provisionally
passing once its first assessment is queued. It keeps the card in Failed, Passed,
or Not assessed and appends Checking or a known event countdown to the result.
This display policy does not create a status write or increase clean-session
counts. A saved failure remains visible during later checking.

Message text is separate from status text. `status.input.message` explains a
status to the user; it is not an in-agent notice. Each applied status action
replaces its previous message, even when the status result is unchanged. Omitting
`input.message` clears the previous message. Skipped status actions retain it.
A message action does not
insert text into model input, steer the agent, or block work. The current runtime
rejects message actions. An implementation that supports delivery MUST persist the message
plan and handled-trigger marker with the assessment, claim before sending,
and never resend an uncertain attempt. Its contract is at-most-one send
attempt, not exactly-once display.

The draft's message cooldown defaults to `5m` per check, branch, activation
generation, and zero-based action index. It is separate from run cooldown.
Attempt claims consume cooldown even on failed or uncertain delivery. A new
generation starts new cooldown slots; handled triggers still prevent replay
delivery.

## 10. Examples, support, and glossary

The [examples](README.md#examples) state their current support and missing
capabilities. Message-window examples illustrate selected-message claims, not
complete turns; Fast Mode Value requires a measured activity period.
Example text MUST NOT imply that current instruction files prove past
applicability, that a skill was never used, or that timing occupancy proves
counterfactual savings.

| Term                  | Definition                                                                                          |
| --------------------- | --------------------------------------------------------------------------------------------------- |
| Activation generation | Durable identity for one effective definition and enabled state                                     |
| Assessment            | One accepted or rejected evaluation of a frozen candidate                                           |
| Candidate             | Trigger and selected evidence frozen for a possible assessment                                      |
| Completeness          | Whether the source proves selected membership and required content                                  |
| Current status        | Latest accepted status write for one check/branch/generation                                        |
| Native order          | Source-proven position within a producer's data, distinct from timestamps and delivery order        |
| Not assessed          | No valid decision could be made because evidence, capability, limits, or execution were unavailable |
| Response cycle        | Host interaction boundary that can include automatic continuation and tools                         |
| Uncertain answer      | A valid authored model result whose meaning is uncertain; not an execution failure                  |
| Window                | Rolling array of selected matching messages ending at a trigger                                     |

Runtime capabilities belong in [authoring](authoring.md#supported-subset).
Source and result limits belong in [session coverage](../session-coverage.md)
and [check coverage](../check-coverage.md). Draft syntax does not imply that
every source, release, or provider supplies every field or result capability.
