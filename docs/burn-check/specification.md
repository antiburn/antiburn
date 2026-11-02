# Burn Checks specification: v1-alpha

Use this reference to write definitions, build parsers and compilers, or implement
source adapters. MUST states a requirement. MUST NOT states a prohibition.
SHOULD states a recommendation. These terms apply within this draft. They do not
promise language stability or support for every capability.

> **Experimental draft: `v1-alpha`.** Burn Checks are not yet standardized.
> Syntax, semantics, and capabilities may change, including breaking changes.
> There is no backward compatibility promise. A schema-valid definition can use
> features the runtime or a source adapter cannot execute. See the
> [supported subset](authoring.md#supported-subset).

## Current implementation

The runtime supports local status checks and independent model decision blocks
with Choice, Score, and Noul questions. It accepts seven events in three
families: public messages and tool calls/results, proven response-cycle ends,
and host session observations. Each source must prove its event and bindings.
The desktop emits `session_start` reason `attached` only. `session_idle` records
a 120-second observation boundary, not real idle time, model occupancy, or savings.

Cursor Desktop has a separate ordered snapshot tool-result route. Its accepted
user-slot prefix supplies result context, not a separate public user event. It
does not add public assistant/call, response-cycle, or host-session events. See
the [source contract](../session-coverage.md#cursor-desktop-yaml-tool-evidence).

Message actions are not supported, including behind `if: false`. Effective
`agent.skills`, `repo.branch`, historical instruction activation, and historical
effective skill inventories are also unavailable. Remote, WSL, child, and
historical-only sources do not start new live checks. Native fix prompts, Auto
Fix, verification watches, burn estimates, and savings are not Burn Check
capabilities. Broader draft forms do not promise product support.

Missing capabilities are unassigned future work. Beta and stable scope are not
set, and missing features are not usable today. Use the
[authoring guide](authoring.md#supported-subset) for the implemented subset,
[session coverage](../session-coverage.md#burn-check-source-facts) for accepted
source shapes and producer versions, and [provider contracts](provider-contracts.md)
for typed output and connection limits. Schema acceptance, compiler admission,
source proof, and provider compatibility are separate requirements.

## 1. Version and compatibility

The specification label is exactly **`v1-alpha`**. It identifies this draft,
not a YAML field or an internal serialization version. A check MUST NOT add a
YAML `version` or `$schema` instance field. The JSON Schema dialect is
Draft 2020-12; it is separate from the specification label.

The schema is local at
[`schemas/burn-check.schema.json`](schemas/burn-check.schema.json). It has no
hosted `$id`; authors and editors MUST use the checked-in asset or a verified
equivalent. A runtime MUST use this asset or a verified equivalent and test
structural parity. Editor validation does not activate a definition.

The [authoring guide](authoring.md#supported-subset) states implemented
capabilities. Runtimes MUST reject unsupported definitions before activation and
MUST NOT silently remove unsupported fields or actions.

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

The desktop MUST keep a fresh setup without a connection disabled. It MUST
preserve explicit saved off and disable execution when saved storage is malformed
or unreadable. Saving an inactive connection MUST NOT select it. Creating the
first connection selects it without enabling checks. Removing the selected
connection MUST disable checks and clear selection without choosing a replacement
or deleting accepted results, evidence, or usage. Empty connection settings MUST
remain empty across restart. YAML check cards and navigation MUST require
a discovered global or repository definition. Settings MUST keep the master
toggle and decision-model setup available without definitions. A discovered
check without a configured System One provider MUST show an actionable setup
error in its detail; absence of definitions MUST NOT create a placeholder check
or error.

Turning the master setting off stops execution, not local definition discovery.
The desktop can read and parse bounded YAML definitions and watch their
installation directories, including at cold startup, to show names, diagnostics,
and paused cards. Current limits are 256 KiB per definition and 128 files per
folder, with the existing directory-access gates. This discovery MUST NOT restore
provider configuration, discover model capabilities, catch up session sources,
or run assessments. File presence MUST NOT enable checks or bypass execution
gates.

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

Omitting `decisions` selects local execution. A present `decisions` field MUST be
a nonempty mapping; null, an empty mapping, or an invalid declaration MUST NOT
fall back to local execution. The runtime supports named independent blocks with
mixed Choice, Score, and Noul questions.

Unknown keys MUST be errors at every defined level. Names for decisions,
inputs, outputs, Choice labels, and aliases MUST match
`[a-z][a-z0-9_]*`. Fixed text fields MUST be nonblank. Fixed text fields MUST
NOT contain `${{` interpolation.

Decision entries contain `input` and `output`, with an optional `fit`
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

| Event               | Meaning                                                | Direct context                                              | Specific filters                           |
| ------------------- | ------------------------------------------------------ | ----------------------------------------------------------- | ------------------------------------------ |
| `session_start`     | A new, resumed, or attached observation starts         | `session.start_reason`                                      | `reason: [new, resumed, attached]`         |
| `session_idle`      | The configured observation policy emits an idle period | `session.idle_seconds`, optional `session.last_activity_at` | None                                       |
| `user_message`      | The host accepts a distinct message in the user slot   | `message`                                                   | None                                       |
| `assistant_message` | A public assistant message completes                   | `message`                                                   | None                                       |
| `tool_call`         | The agent requests a tool operation                    | `message`                                                   | `tools`, `paths`, `paths-ignore`           |
| `tool_result`       | A tool operation has a final observed result           | `message`                                                   | `tools`, `paths`, `paths-ignore`, `status` |
| `turn_end`          | The host proves the end of a response cycle            | `turn`                                                      | `status`                                   |

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

Use recorded provider/model and mode evidence. A provider may be derived from
a reviewed fixed route. Mode evidence may report speed, a selected catalog mode,
a recorded requested-tier preference, or a low-confidence requester model-ID
suffix hint. It need not prove the tier actually served. Current settings and
unbound display labels do not establish past requester mode. State the evidence
kind and each source's limits. Do not claim savings from a mode hint. Missing is
never `false`.

### 4.2 Tool and path filters

`tools` accepts normalized categories: `shell`, `read`, `search`, `edit`,
`write`, `delegate`, `skill`, `mcp`, and `other`. The original host tool name
is separate.

The adapter MUST derive check-facing categories from reviewed native tool
identities. A metric category that combines several tools does not establish a
more specific category. In particular, the adapter MUST NOT recover `write`,
`delegate`, `skill`, or `mcp` from a collapsed `other` metric category or infer
them from result text. These check-facing categories MUST NOT change the metric
categories used by native deterministic checks. Schema acceptance alone does
not establish source support.

Path patterns match recorded repository-relative paths with `/` separators,
without a leading `./`. Matching is case-sensitive on every platform. `*`
matches within one path segment; `?` matches one character within a segment;
`**` is a whole segment matching zero or more segments. The entire path must
match. Other characters are literal. Reject absolute paths, `..` segments,
backslashes, and unsupported wildcard syntax.

`paths` requires at least one recorded path to match. `paths-ignore` vetoes an
event if any recorded path matches; the veto wins when both filters are set.
When a path filter is set and paths are unavailable, the event does not match
and the runtime records why. A known included path can match a partial set when
no ignore filter requires proof that a veto path is absent. A partial set cannot
pass `paths-ignore`; a known ignored path always vetoes it. Paths outside the repository are not repo-relative
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
assessment. Every member MUST have a unique public identity and one exact current
source-version binding. A member correction or removal invalidates dependent
unfinished work and stale accepted evidence. Standalone usable messages remain
eligible when a cycle is unavailable. See
[session coverage](../session-coverage.md#burn-check-source-facts) for admitted
native boundaries, producer pins, and source-specific limits.

`session_start` describes an observation attachment, not necessarily native
session creation. The desktop persists one start with reason `attached` for a new
attachment, including an empty source. It also initializes an existing admitted
attachment once after successful upgrade validation, without reseeding results.
The draft and compiler accept `new` and `resumed`, but the desktop currently emits
only `attached`.

`session_idle` is a persisted observation, not a claim that a session remains
idle. Genuine admitted native activity resets the 120-second interval using local
observation time. Exact replay, historical baseline facts, recorded selection
changes, UI actions, heartbeats, and Antiburn notices do not reset it. Publication
requires a usable source frontier and the exact current source, attachment epoch,
continuity, and activity revision. Recovery after lost continuity starts a fresh
observation interval.

New activity MUST cancel unfinished idle candidates. Claim, local start, provider
dispatch, and acceptance MUST recheck attachment and activity dependencies. An
already accepted idle assessment remains valid at its recorded boundary. Start
candidates bind the exact attachment; they do not require unchanged later
activity. Restart MUST preserve the deadline and emitted revision without
duplicating an idle observation. The 120-second policy is not a universal host
idle signal or a measured activity partition.

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

All selected message alternatives share one window. Each admitted public text,
call, or result record counts once under its own identity. A call and its result
MUST NOT deduplicate each other. The window contains only matching records, not
every record between its first and last members.

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
selects later messages. It keeps partial-history quality, identity history,
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
local checks default to `0s`. Cooldown starts at the first durable provider dispatch intent
or durable local evaluation start. Local start and its cooldown MUST save
together, once; a later action failure does not reset or remove that cooldown.
A filtered top-level condition does not start local evaluation. Retries, later
decision blocks, and resumed work MUST NOT reset or extend cooldown. A resumed
candidate bypasses only its own already-started cooldown; unrelated candidates
and runtime gates still apply. Zero cooldown does not
disable single-flight execution.

Source adapters order native records before assigning durable observation
positions. Timestamps are not a sort key. Late earlier records remain in
history with a diagnostic and do not enter frozen windows or create retroactive
actions. Deduplication, selector progress, frozen candidates, and resume
progress must recover together so replay does not publish duplicate work.

### 5.1 Live attachment and replay

Automatic live review attaches to supported active native main sessions when
checks are enabled and configured. Fresh attachment may seed a bounded recent
tool-result window and assess only its latest ready candidate once. It MUST NOT
replay each older window or traverse the complete historical backlog. The attachment marker,
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

The recent seed remains result-only, including for a mixed message check. New
user messages, assistant messages, calls, and cycles use a separate future-only
attachment boundary; retained old records do not become live triggers. An adapter
may retain a proven cycle start for a later terminal under its source contract.
Session observations use their persisted attachment and activity boundaries.

After attachment, new safely admitted first-terminal results and first-eligible
public events can trigger work. Pending/running mutations can advance their source
position. A result's first terminal position or another event's first eligible
position fixes its logical order. Corrections retain that position and frozen
event context. A correction MUST NOT become a new attempt merely because its
publication follows activation. Separate per-kind identities, first eligibility,
deduplication, and stale-removal checks persist across pages and restart. Page
absence MUST NOT imply removal. An unknown historical membership cannot be
recovered by guessing from the current rolling window.

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

### 5.2 Public-event admission bounds

The current runtime admits a new public or host event and its published source
context within 4 MiB (4,194,304 bytes), 262,144 JSON values, and depth 64 with the
combined root at one. A complete cycle contains at most 4,096 messages and 4,096
exact member bindings. The event envelope, copied messages, and bindings share
the byte bound; a smaller native source bound still applies. These are runtime
admission limits, not YAML fields or provider input limits.

Unsafe identity, ownership, or order MUST produce an explicit unsafe update.
Missing cycle proof or an exceeded cycle bound MUST NOT create a partial
`turn_end`. An adapter can continue publishing independent bounded messages. A
bound ineligible correction can withdraw an earlier event without exposing the
new payload. Source delivery MUST preserve its cursor when required bounded
continuation has not committed; it MUST NOT silently discard output or treat a
bounded page as a complete current inventory.

## 6. Context and data model

Values are JSON-compatible: null, Boolean, finite number, string, array, or
string-keyed object. Instruction `observed_at` and message `occurred_at` use
RFC 3339 strings when supplied. Turn `started_at` and `ended_at`, and
`session.last_activity_at`, use recorded integer Unix milliseconds. Timestamps
MUST NOT establish source order or drive the local idle interval. Optional values
are absent when not observed; they are not represented by guessed strings, zero,
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
| `session.last_activity_at`             | Optional recorded Unix-millisecond activity time on `session_idle`                                                       |
| `session.model`                        | Current observed model selection, not proof of past execution                                                            |
| `session.fast_mode`                    | Current recorded mode selection when supported; not proof of past execution or served tier                               |
| `global.instructions`, `global.skills` | Observed instruction or skill snapshots                                                                                  |
| `repo.root`, `repo.branch`             | Observed project root and branch; either may be absent when unknown                                                      |
| `repo.instructions`, `repo.skills`     | Observed project instruction or skill snapshots                                                                          |
| `agent.name`, `agent.skills`           | Host agent name and effective available skill snapshot                                                                   |

The four context roots `session`, `global`, `repo`, and `agent` exist in every
phase. Individual values and inventories can still be unavailable. With no
known project, repo collections are unavailable. A known project with a
completed supported inventory containing no entries has a known empty array.
An absent collection is unavailable. A partial supported scan can retain known
entries with explicit completeness and limit fields. Do not infer an effective
skill inventory by merging generic lists.

Global check location and global input context are separate. A global check
may select repo instructions; a project check may select global instructions.

`session.model` and `session.fast_mode` are optional on every trigger family.
They require explicit native selection evidence, not the latest requester,
mutable metrics, or current Settings. The host freezes the last proven selection
through the event. It MUST NOT use it to fill message or turn attribution.
`session.start_reason` requires a configured `session_start` alternative;
`session.idle_seconds` and `session.last_activity_at` require `session_idle`.
These fields remain absent on the other session alternative. Whole-record reads,
literal brackets, projections, and dynamic access follow the same closed shapes
and phase rules as direct access.

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
| `message.model`, `message.fast_mode` | Optional recorded generating/requesting model and fast-mode evidence; absent for user messages           |
| `message.content`                    | Public user/assistant text; absent on tool messages                                                      |
| `message.tool`                       | Required on tool calls/results; absent on user/assistant messages                                        |
| `message.tool.call_id`               | Required on calls; on results only with a proven call link                                               |
| `message.tool.name`                  | Original host tool name; required on calls and optional on results                                       |
| `message.tool.category`              | Normalized tool category; absent when unknown                                                            |
| `message.tool.arguments`             | Call arguments; on results only when the source proves the matching request                              |
| `message.tool.paths`                 | Optional known repo-relative operation paths                                                             |
| `message.tool.result.status`         | Required on results: `completed`, `error`, `cancelled`, or `unknown`                                     |
| `message.tool.result.content`        | Observed result value; absent if the host does not expose it                                             |
| `message.tool.result.outcome`        | Optional source-reported program or child-work outcome; separate from tool execution status              |
| `message.tool.provider_executed`     | Optional source flag that the provider executed the tool rather than the local host                      |
| `message.tool.result.output_bytes`   | Optional observed result size after host truncation; does not prove completeness                         |
| `message.quality`                    | Optional source-quality facts, including missing arguments, result content, or source gaps               |

Arguments unavailable from a host are absent, not `{}`. Missing call
arguments or result content make the selected tool message partial. A proven
linked result with unavailable arguments remains eligible for assessment, and
the model receives the available fields and quality facts.
Native text input remains an exact string in `arguments`, including JSON-looking
text. The adapter MUST NOT parse that string into an object or infer paths from it.
An orphan result without a proven call link omits `call_id` and `arguments` by
design. Empty known output differs from unavailable output. Host truncation is
recorded as input-quality metadata. The schema does not promise diffs, parsed
test outcomes, filesystem state, or child-session trees.
An unavailable category stays absent, not `other`. An unfiltered result trigger
can use the remaining fields; a category filter cannot match an unavailable
value. A recorded tool name can normalize to native `Other`, including when
the normalizer has no more specific class. This is distinct from missing category
evidence and does not prove a draft `write`, `delegate`, `skill`, or `mcp` class.

When `message.fast_mode` is known, `message.quality.fast_mode_evidence` can state
the source of that Boolean:

| Value                       | Meaning                                                                                              |
| --------------------------- | ---------------------------------------------------------------------------------------------------- |
| `reported-speed`            | Claude's accepted assistant record reports fast or standard speed.                                   |
| `selected-catalog-model`    | OpenCode selects a reviewed Fast/Ultrafast catalog ID.                                               |
| `requested-tier-preference` | Codex records a matching owned tier preference before the request context.                           |
| `model-id-suffix-hint`      | Pi's recorded requester model ID ends in a fast-mode-like token. This is a low-confidence name hint. |

These qualifiers are not numeric confidence or provider guarantees. A positive
hint can match `fast_mode: true` without endpoint or served-tier proof. A valid
judgment's label probability does not make its mode evidence more certain.

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
do not prove which rules applied to past actions. Omit a rule with scope that
cannot be represented and retain an explicit collection limit. Do not make it
unconditional.

Skill records contain `name`, declared `description`, and absolute `path`.
Missing declared descriptions are empty strings. The collector reads declared
metadata without copying skill bodies into inventory input. Selected tool
results can separately contain file text. Using a saved skill list does
not reread its files. Current availability does not prove past availability.

Each current scope has optional `instructions` and `skills` arrays, separate
`instructions_complete` and `skills_complete` Booleans, matching `*_limits`
arrays, and an optional `snapshot_revision`. Omit an unavailable array. Use `[]`
only for a known scan with no observed entries. Keep useful known entries beside
limits; one unavailable scope must not erase another scope or recorded correction.

Capture current inventories only when the authored condition or candidate needs
them, on every trigger family. Capture condition reads before evaluating that
condition. Capture candidate-only reads after filters, conditions, and any window
minimum pass. The collection time is not the native event's historical time. Merge
the snapshot into candidate context before its fingerprint and save it atomically
with the candidate. Do not change the native observation or its evidence version.
Delayed decisions, safe retries, restart, and acceptance reuse the saved snapshot
without reading current files again.

Model and mode fields retain the same type at every scope. User-message records
have no generating model. Tool results use proven requester attribution, not a
model named in tool output. A turn exposes a singular model
or mode only when its complete nonempty request set has one proven uniform
value for that field. Include generating requests without public text; do not
derive this set from the public message list alone. Derive model and mode
independently. Never fill historical attribution from current session settings.

### 6.4 Turn records

A turn has `id`, `source`, required `status` (`completed`, `cancelled`, or
`error`), ordered `messages`, and optional recorded Unix-millisecond `started_at`
and `ended_at`.
It may have `model` or `fast_mode` only under the uniform-attribution rule.
Turn messages use the same record shape as current messages and windows.
Native terminal reasons and error codes do not enter the closed `turn` root.

The context does not split elapsed time into model-active and tool-only periods.
Transcript gaps, token counts, and tool durations cannot prove those periods, how
long another setup would take, or savings. The separate 120-second `session_idle`
policy records an observation; it does not measure these durations.

## 7. Decisions and provider results

The runtime supports local checks without `decisions` and model checks with one
or more independent named decision blocks. Each block contains one to eight
named questions, which may mix the supported types.

Each decision has nonempty `input` and `output`. The host's selected Settings
connection supplies the decision provider and model. A decision-level `model`
field is an unknown-field error. Observed coding-agent fields (`on.*.models`,
`message.model`, `turn.model`, and `session.model`) describe source facts only;
they do not select the decision connection. A question has one of these forms:

| Type   | Configuration                                                                                    | Result                                              |
| ------ | ------------------------------------------------------------------------------------------------ | --------------------------------------------------- |
| Choice | Nonblank `question` and `criteria` mapping of at least two named labels to nonblank descriptions | One declared label string                           |
| Score  | Nonblank `question` and ordered `criteria` array of at least two nonblank descriptions           | Finite number from zero to the last criterion index |
| Noul   | Nonblank `question`; no authored `criteria`                                                      | Finite yes-probability from zero to one             |

Every decision block uses the same frozen trigger/context and one captured
connection snapshot. Decision inputs MUST NOT read `decisions`, including their
own outputs or details. Mapping order creates no input dependency. The runtime
MUST prepare and fit every block before the first inference call. It MUST send
one complete request per block with all that block's questions together, then
execute blocks sequentially in authored order under candidate concurrency and
branch single flight. It MUST NOT split questions into separate requests.
Complete and validate all blocks before actions. If any block is unavailable,
invalid, terminally failed, or has unknown delivery, the check is not assessed
and no actions run. Preserve incurred usage and successful sibling answers.

Choice and Score questions require at least two criteria. Preserve each Choice
label/description binding and Score criterion order. Choice labels have no
positional meaning; provider requests do not promise authored map order. The
language has no fixed criteria maximum. The host MUST check the selected
connection's criteria limit and output capabilities before scheduling, separately
from language validation.
It MUST repeat this check when the selected connection or its capabilities
change. A capability mismatch pauses scheduling without invalidating the
definition; selecting a compatible connection restores eligibility without a
YAML edit.

Provider capabilities declare each question type's support, criterion interval,
probabilities, and confidence separately. Reject references to unsupported
details before scheduling, including indirect reads, projections, function
arguments, and whole-object reads. A whole details object requires the optional
details it exposes. Numeric input overrides MUST NOT establish output support.
See the [pinned provider contracts](provider-contracts.md); these limits do not
imply support for other models or runner versions.

Actions read these closed result records, where `review` and the question names
stand for authored identifiers:

| Expression                                        | Type and meaning                                                                |
| ------------------------------------------------- | ------------------------------------------------------------------------------- |
| `decisions.review.outputs.verdict`                | Choice: declared label string                                                   |
| `decisions.review.details.verdict.probabilities`  | Optional Choice map of declared labels to raw probabilities                     |
| `decisions.review.details.verdict.confidence`     | Optional Choice provider confidence                                             |
| `decisions.review.outputs.severity`               | Score: finite number in `[0, criteria.length - 1]`, including fractions         |
| `decisions.review.details.severity.probabilities` | Optional Score array in authored criterion order; use `[0]` for the first level |
| `decisions.review.details.severity.confidence`    | Optional Score provider confidence                                              |
| `decisions.review.outputs.applies`                | Noul: finite yes-probability in `[0, 1]`, never a Boolean                       |
| `decisions.review.details.applies`                | Noul: closed empty record `{}`; no probabilities or separate confidence         |

Unavailable optional details MUST be absent, not null or invented values.
Confidence MUST NOT replace a label probability or Score distribution. The
answer set MUST match the declared names and types exactly; missing, extra,
unknown, duplicate, or wrong-type answers are invalid. Choice labels MUST match
the declaration. Available Choice maps MUST have exactly the declared labels.
Available Score arrays MUST have exactly one value per criterion. Every
probability and confidence MUST be finite and in `[0, 1]`. Distribution sums
MUST satisfy `abs(sum - 1.0) <= 0.02` under `f64` arithmetic. Score sums MUST
follow criterion index order, `0` through `n-1`, not lexicographic wire-key order.
Choice sums follow label-map order. Preserve raw valid
values; do not normalize distributions, replace a valid Choice label, or
recompute a Score. At the provider boundary, Score legend and probability keys
MUST be exactly `0` through `n-1`, and each legend value MUST equal the authored
criterion at that index. The language exposes the validated distribution as an
array, not the wire index map or legend.

An `uncertain` Choice label is ordinary authored output. It is not an
execution state and has no implicit action behavior. A probability threshold
is an author policy, not an accuracy claim.

### 7.1 Attempts, recovery, and publication

Each attempt identity MUST contain candidate, decision name, and attempt number,
independently of provider identity. Each decision permits at most two attempts;
only a known-safe transient failure permits the second. Assessment totals use
checked `u64` counts. A durable dispatch intent MUST precede admission and I/O.
Persist each validated block answer before the next block can send, and recheck
current claim, generation, source, and runtime gates before each intent.

Safe retry and restart MUST reuse successful sibling answers and the exact saved
request for the unfinished block. They MUST NOT resend completed blocks, refit
saved requests, or create a third attempt. An unresolved intent becomes unknown
on recovery. Unknown delivery in any block prevents every further automatic send
for that candidate and all action publication. Late usage remains attached to the
original attempt even when its answer loses publication rights.

After all answers are durable, publication recovery MUST validate the complete
typed set and saved preparation without model discovery, client construction, or
transport admission. Current master, selected configuration, authentication,
source, claim, generation, connection-transition, and shutdown gates still apply.
A connection switch MUST cancel unfinished work; switching A to B and back to A
MUST NOT revive it. Atomic acceptance commits assessment history, ordered action
outcomes, optional status, and watermark together.

Historical accepted status, time, input, provenance, and usage MUST remain
readable under a bounded read-only legacy decoder. Such evidence MUST NOT
authorize execution. Incompatible old pending work MUST stop explicitly without
inference, a YAML shim, a fresh budget, or replay. A malformed saved candidate
MUST NOT stop unrelated candidates from recovering; storage errors still propagate.

Finite replay MUST count durable-total attempts and invocation-new intents
separately as `u64`. Its hard allowance applies before every new intent,
including retries. Before the first new intent, it MUST cover at least one send
for every unfinished block. Exhaustion settles `LimitReached` with no actions or
watermark; it is terminal and cannot resume automatically after the invocation.
Saved-answer recovery consumes zero new intents. Local replay can run with zero
provider allowance under its event and evaluation bounds.

### 7.2 Local execution

A local check MUST evaluate status actions against its frozen candidate without
model discovery, warm-up, model input preparation, provider client/transport
construction, transport admission, or inference. It MUST create zero provider
attempts and zero usage records, including unknown-usage placeholders. Local
execution has no decision input, fitting report, provider request, or model
connection provenance. A `decisions` reference is a compiler error when that root
is undeclared, including behind a false condition. Message actions remain
unsupported, also behind false conditions.

The master setting, configured selected connection, credential/authentication
rules, repository pause, activation generation, source validity, and shutdown
gates still apply. Missing model output support and failed cached model discovery
alone MUST NOT block local work. These rules apply with Jev, Ollama, Cloudflare,
Custom direct, and Custom envelope selected; they do not remove each route's
configuration or credential requirements. Testing a connection remains separate
and can send a chargeable provider request.

Local execution MUST use the existing claim and branch single-flight rules.
Local preparation, the zero-result/zero-attempt answer, and the first local
cooldown MUST save atomically before action acceptance. A repeated start MUST
NOT change the saved start, answer, budget, or cooldown. Restart MUST restore
started work rather than create a provider attempt or start local work again.
Missing, invalid, or incompatible local preparation MUST stop that candidate;
it MUST NOT create a fresh budget or fall back to model preparation.

The runtime MUST check selected evidence dependencies and shared action budgets
before accepting the ordered plan. The existing atomic acceptance commits action
outcomes, an optional status write, and the accepted watermark together. A valid
no-write assessment preserves the earlier status-writing assessment, explanation,
evidence, and write time. Missing evidence and evaluation failure MUST NOT
synthesize success. A blocked saved local answer remains recoverable while the
existing runtime and durable acceptance gates require deferral.

Local evidence MUST identify the exact status-writing candidate as **Frozen
local context**. It describes values available to the actions, not proof that
every value was read or sent to a model. The report MUST NOT label it assessed
model input, add model connection provenance, or show a missing-model-input
warning. Source invalidation and retention keep the same status-writing evidence
binding. See [local status checks](authoring.md#local-status-checks).

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

Decision input values and action message text support substitution. Status
`input.result` accepts whole expressions, not embedded text templates. Names,
descriptions, filters, questions, and criteria are fixed text.

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
literal bracket keys. Tool arguments, result content, and `fromJSON`
values are open JSON subtrees; their keys are checked at runtime. The normalized
tool result and outcome records remain closed. Optional known fields and
dynamic source-data keys may be unavailable. Check variants before reading
variant-specific fields.

Dynamic access MUST NOT bypass closed-record validation. A dynamic object key
requires an open JSON object. Dynamic array indexes and projections retain the
declared item type for all following access. A function return value permits
access only when its result type supports it.

Value identity is separate from evidence provenance. Aliases and indexes retain
the selected frozen value's identity. `fromJSON` retains the dependencies of its
input but creates fresh object and array identities. Separate conversions do not
compare equal merely because they use the same source or produce equal content.

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

For the deployed request-history collection, membership operations require the
frozen `session.requests_complete` value to be `true`. Missing or false values
make those operations unavailable. This includes operations after conversion of
the array or its enclosing record. A whole copy of available partial history
remains usable; this does not establish complete historical membership.

The deployed runtime uses a separate bounded budget for top-level `if`, before
window selection. One assessment budget MUST cover recursive decision input
preparation and all action planning, including action conditions, results, and
messages. Nested templates, map aliases, and later actions MUST NOT reset it.
This boundary applies across every decision block, not separately to each block
or provider attempt. Action replanning restores the saved post-input snapshot;
it does not accumulate abandoned plans or change immutable preparation.

The deployed limits are recursive depth 32, starting at one; 10,000 evaluation
steps; 1,000 visited items; 8 MiB (8,388,608 bytes) of cumulative generated data;
and a separate 8 MiB cumulative materialization allowance. Fixed container entries
MUST count as visited items. Source and alias copies MUST reserve materialized
bytes before cloning. Fixed text and container construction MUST count before
allocation. These limits are
runtime bounds, not schema syntax or YAML fields. They are separate from fitting,
source parser, and provider request limits.

Expression evaluation MUST report budget exhaustion as `EvaluationLimitExceeded`;
the input preparation boundary records `PreparationResourceLimit`. Exhaustion
MUST remain in effect. It prevents the whole assessment and all status writes, including writes planned
before the crossing operation. Saved preparation MUST retain the consumed
budget. Recovery MUST validate and restore it for action planning, not replace
it with a fresh budget. An absent or incompatible snapshot does not authorize a
reset for already prepared work. See the
[deployed evaluation limits](authoring.md#shared-evaluation-limits).

### 8.5 Input fitting

An optional decision-level `fit` mapping authorizes bounded loss from the
evaluated decision input when the complete provider request exceeds its limits.
Without `fit`, or with `fit: {}`, the runtime MUST preserve every input value.
Missing rules alone MUST NOT produce a warning. An input-size explanation MUST
follow an actual preparation or provider failure, not a prediction based on the
absence of `fit`.
Fitting MUST NOT change source evidence, the frozen original input, questions,
criteria, or values outside an authorized target.
It MUST NOT add automatic loss, summary calls, or other model preparation calls.

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
recovery MUST bind one preparation profile to the complete compiled decision
set and provider capabilities, and validate every saved request against its
input, omission report, and fixed construction. It MUST NOT silently refit an
already dispatched request under changed limits or policy. An incompatible
profile settles with zero additional attempts; an earlier safe attempt remains
in the durable attempt count.
Unknown delivery still does not authorize a resend.

### 8.6 Preparation limits and local outcomes

The current release applies the [deployed preparation limits](authoring.md#preparation-limits)
to original input size, nodes and depth, rule/path size, target expansion,
traversal, attempted turns, and cumulative serialized-request validation work.
These are release resource limits, not YAML fields or a shared evidence
retention cap. Admission can stop before checking whether the request fits.

One 64 MiB (67,108,864-byte) fitting-work allowance covers all blocks, turns,
input measurements, and complete-request construction/validation, including each
accepted request. Save its consumed count as an immutable fact. Validation of the
saved set uses a distinct shared 64 MiB pass across all blocks. That pass MUST NOT
fit, change, or replace a saved request, and is not another fitting allowance.

The full ready preparation record, including its tag, profile, names, originals,
fitted inputs, requests, omission reports, and budget, MUST fit within 8 MiB,
65,536 JSON values, and depth 64 with the complete root at depth one. Admission
MUST check before retaining another block. Typed results have a separate 512 KiB,
65,536-value, depth-64 bound. Before the first call, reserve worst-case result
capacity from exact declarations and all optional detail shapes, including the
longest Choice label and bounded numeric encodings. Check actual results again
before saving. These bounds limit runtime work and retained state, not the
language's number of independent blocks. Strict saved decoders MUST reject
duplicates before insertion, unknown versions, invalid budgets, and crossing
values before allocation.

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
actions, except that shared evaluation-budget exhaustion prevents the whole
assessment. Persist every action outcome atomically. Model checks run no actions
if their decision set is invalid or incomplete. Local checks without a model
decision use the same action and acceptance rules with zero provider attempts.

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
when status changes. Describe a retained success as a saved check result, not
"Resolved" or the result of work still running. When showing progress beside a
saved result, keep both meanings clear in one status panel.

The maintained Over-exploring example fails only for `likely_excess` with label
probability at or above 0.95. Its success action uses the exact complement, so
uncertain and below-threshold answers explicitly write success. Ignored
instructions instead requires `no_conflict` at probability 0.95 or above to
write success; an uncertain answer writes no result. These are authored policies,
not engine fallbacks.

For definitions with no matching action, acceptance MUST retain the prior
status-writing assessment, explanation, evidence, and write time while advancing
the accepted watermark. Without a prior write, engine status remains unassessed.
The runtime MUST NOT synthesize a success action when no condition matches.

Model evidence MUST show each decision's saved original evaluated input, exact
fitted input, and nonempty omission report from the status-writing assessment,
not the latest assessment or reread source. Connection provenance MUST identify
that assessment's provider/model and available revisions, not current Settings.
Local evidence remains **Frozen local context** without model provenance.
Historical accepted preparation MUST preserve available input and provenance;
it MUST NOT invent an original input or named block that the record did not save.
Diagnostics MUST use typed codes and context with friendly text, not raw provider
errors or response bodies.

The desktop keeps a YAML check in Not assessed until its first accepted status
write. A saved success puts it in Passed; a saved failure puts it in Failed.
Queued or running work appends Checking beside that result without moving the
card to a separate group. A known event countdown can appear in detail when
source delivery is current. Checking does not create a status write or increase
clean-session counts. A saved failure remains visible during later checking.

Source alerts describe current failures, not missing history alone. A healthy
capture with partial earlier history does not show a capture alert. The host
retains the gap and its evidence limits without claiming that an event was lost.
An agent that uses file updates does not require a live service connection.

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
complete turns. Fast mode value requires recorded requester fast-mode evidence
and affirmative task evidence, not an idle event or a measured activity period.
It suggests a standard-mode trial for similar non-urgent work without promising
route availability, served tier, lower cost, or savings.
Example text MUST NOT imply that current instruction files prove past
applicability, that a skill was never used, or that timing occupancy proves
counterfactual savings.

| Term                        | Definition                                                                                          |
| --------------------------- | --------------------------------------------------------------------------------------------------- |
| Activation generation       | Durable identity for one effective definition and enabled state                                     |
| Assessment                  | One accepted or rejected evaluation of a frozen candidate                                           |
| Candidate                   | Trigger and selected evidence frozen for a possible assessment                                      |
| Completeness                | Whether the source proves selected membership and required content                                  |
| Current status              | Latest accepted status write for one check/branch/generation                                        |
| Native order                | Source-proven position within a producer's data, distinct from timestamps and delivery order        |
| Not assessed (saved status) | No accepted status write yet; a valid no-write assessment can leave this status unchanged           |
| Not-assessed candidate      | Evidence, capability, limits, or execution prevented assessment; this is separate from saved status |
| Response cycle              | Host interaction boundary that can include automatic continuation and tools                         |
| Uncertain answer            | A valid authored model result whose meaning is uncertain; not an execution failure                  |
| Window                      | Rolling array of selected matching messages ending at a trigger                                     |

Runtime capabilities belong in [authoring](authoring.md#supported-subset).
Source and result limits belong in [session coverage](../session-coverage.md)
and [check coverage](../check-coverage.md). Draft syntax does not imply that
every source, release, or provider supplies every field or result capability.
