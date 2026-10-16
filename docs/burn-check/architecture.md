# Burn Checks architecture

Burn Checks reuse native source readers, a shared engine, the TypeSafe/Jev
provider, SQLite persistence, and the existing Checks UI. The
[authoring guide](authoring.md#supported-subset) states supported capabilities;
[session coverage](../session-coverage.md#burn-check-source-facts) owns source limits.

## Ownership and dataflow

| Layer             | Owns                                                                                                                                               |
| ----------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- |
| `antiburn-local`  | Discovery, parsing, native identity/order, tool semantics, and source evidence                                                                     |
| `antiburn-checks` | Compilation, context preparation, selection, decisions, actions, and engine transitions                                                            |
| Desktop           | Watcher lifetime, committed evidence publication, SQLite Store adapter, credentials/provider admission and accounting, Tauri API, and presentation |

```text
native discovery and bounded live reader
  → ToolResultObservation and UserMessage
  → committed publication with frozen ToolResultContext
  → StorePublishedEvidenceReader → FileEventSource
  → frozen candidate → DecisionEvaluator
  → atomic assessment/actions/optional status/watermark
  → report event → existing Checks collection and detail pane
```

`CheckEngine` owns transitions. Expression evaluation and action planning remain
pure. Watch notifications request reconciliation; they are not result events.
Use the native adapters through bounded live readers. Claude, Codex, and Pi use
`read_native_file_page`; OpenCode uses `read_opencode_live_tool_result_page`.
A reserved native-delivery task rotates file and journal turns, independently of
legacy metric indexing. Projection passes do not also publish native result
snapshots for sources owned by live delivery. Source publication starts after
enabled/configured definitions reconcile and the existing first-run Checks gate
opens. Both routes require an enabled live activation for the exact native
environment and stored session CWD. Cold bindings require a supported main
session active within 180 seconds. Compatible admitted work, including interrupted
cold claims, continues after activity expires. Remote, WSL, known forks/subagents,
unknown/empty CWD, and historical-only sources do not start new live work.

Productive continuation is due immediately on both routes. Durable monotonic
claim order rotates productive sources without a clock tick and across restart.
Active sessions and changed cursors get a one-second poll. Idle attached dormant
sources use a fair 30-second resume sweep without a discovery generation change.
Disabled and ineligible cold sources do not create claims or idle wake loops.
Retained journal fences record rotation order; lease presence marks in-flight
work. Recovery requeues leased work and preserves the order.

Live reads restore bounded framing, call joins, request context, and native
frontiers. They read suffixes and bounded validation windows, not the historical
prefix on every turn. File reads check available identity, truncation, head/tail anchors,
and the consumed page. OpenCode checks owner, session, parser/state revision,
frontier, and bounded origin/current-boundary samples. These checks do not prove
all old payloads unchanged. The strict OpenCode replay reader retains full-prefix
hash validation and rejects live cursors.

## Identity, order, and frozen evidence

A definition has a filename-stem ID, installation scope, and content revision.
An activation binds its effective definition, environment/session scope, run,
and durable generation. Session branches retain exact source ownership; a Git
branch name is metadata. A collection row groups by logical ID after resolving
project overrides; it does not merge branch status or evidence.

`ActivationScope` contains `environment`, `session_cwd`, and `run`.
`session_cwd` is the exact stored session working directory, not the resolved
repository root. `StoreCheckStore::activation_scope_key` serializes this scope
as JSON. Runtime records, SQL columns, and evidence target locators use
`activation_scope_key`. `EffectiveActivation.definition_scope` identifies the
global or project definition selected by discovery; `id.scope` identifies the
runtime isolation boundary. `EffectiveActivation.project_root` supplies the
resolved repository root for `repo.root`.

Session branch IDs encode the JSON tuple `[environment_key, agent, session_id]`.
The publication reader, persistence, and deletion share this encoding, so slashes
and escaped characters cannot merge distinct sessions. Session deletion removes
the branch's selector state and runtime rows before the same session key can
start again.

Internal `InstalledCheck*` names and the `installed:` display/snooze
namespace distinguish these rows from native detector IDs. The product term is
**Burn Checks**. These internal discriminators do not extend `DetectorId` or
authorize native remediation.

Native source order selects records. Publication cursors track delivery; engine
positions track committed observations. They are not interchangeable. The first
terminal result can trigger once. A correction keeps its logical result identity
and first-terminal position, but changes its evidence version. Duplicate delivery
is not a new attempt. Timestamps cannot replace proven source order.

Pending/progress mutations do not reset selected terminal order. Corrections that
remain filtered do not create a source gap or a new attempt. A correction that
makes branch membership uncertain invalidates unfinished work and accepted
evidence for that branch, including earlier-page candidates that do not select
the corrected identity. Publication, selector state, and invalidation commit
atomically and survive restart. Other branches remain valid. Stale or duplicate
events acknowledge delivery without replacing newer observations, advancing
engine observation order, or creating candidates.

`EffectiveActivation.bootstrap_observation_position` is the observation boundary
for a new generation. `bootstrap_publication` is its separate publication cursor.
The local evidence sink uses `tool_result_revision_key` to retain revisions; that
key is not a logical result identity.

The local sink freezes user-message prefixes and main-session admission in
`ToolResultContext`. Candidates freeze selected records, linked calls, scope,
positions, completeness, and revisions. Delayed provider work does not reread
mutable source rows. Missing required evidence prevents assessment. Selected
corrections/deletions can invalidate current status without writing success;
unrelated appends do not invalidate a candidate. Project reads filter exact
environment/session CWD before the page limit. Old payloads without context stay
unavailable.

OpenCode request construction excludes synthetic text in snapshot and journal
paths. A mixed message retains its real text. A generated-only message or
replacement cannot fill a missing user request and leaves context partial.
The compiler admits only category filters emitted by the production adapter:
`shell`, `read`, `search`, `edit`, and `other`. The broader draft schema is not an
adapter capability claim. Missing category evidence stays absent; native `Other`
is a separate normalizer result.

## Input preparation and fitting

Core evaluates input templates once against the frozen candidate. The compiler
retains decision-level `fit` rules in YAML order, validates the path grammar and
declared input roots, and rejects overlap. Fitting expands concrete targets once
in rule and original array order. Missing optional properties and empty arrays
are skipped; incompatible present traversal or target types reject preparation.
The original candidate and source evidence remain unchanged. Original input
admission runs before expansion. The [deployed preparation bounds](authoring.md#preparation-limits)
cap input bytes/nodes/depth, rules, path bytes/segments, matched leaves, traversal
visits, attempted turns, and cumulative complete-request measurement. The Jev
adapter's 64 MiB work budget is separate from source retention and provider
request/token/resident limits.

`recent` removes one oldest array item per turn, retaining at least one.
`truncate` removes up to 1,024 bytes of original middle text per turn, retaining
at least 256 original UTF-8 bytes in a Unicode-safe head and tail with one marker.
Unicode alignment rounds retention upward. The first turn is skipped unless the
omitted substring is larger than the marker after JSON escaping. Every concrete
non-exhausted target gets an equal round-robin turn. The complete provider
request validator runs before reduction and after every changed turn, including framing,
the authored question and criteria, omission metadata, serialized size, and
resident-request limits. Fitting stops immediately when validation succeeds.
Other fields are never a fallback. See the
[precise fitting contract](specification.md#85-input-fitting).

The Jev adapter builds the full provider request and supplies validation at the
provider boundary. `state.evidence` holds the prepared input. When fitting changes
it, `state.input_omissions` adds explicit framing and a `targets` list with each
changed path, `strategy`, `original_count`, and `retained_count`. Counts mean array items for
`recent` and original UTF-8 bytes for `truncate`, excluding marker bytes. This
metadata describes preparation loss separately from source-quality limits.

SQLite stores the preparation result in
`burn_check_candidate.preparation_json` before durable dispatch intent. A ready
record holds the preparation profile, fitted input, exact provider request, and
omission report. A rejected record holds a typed failure code, detail, optional
typed decision/field/path/reason/budget context, and any report of attempted
reductions; a rejected report does not mean evidence
was sent. Old rows can have no preparation record. The candidate still retains
the frozen original context. The store also retains typed execution diagnostics
in `diagnostic_json`, independently from friendly display text.

`NoFit` carries a typed reason: no rules, no matching targets, retention/marker
limits, or exhausted reductions. `PreparationResourceLimit` is a distinct result:
local preparation stops without proving that further authorized reductions could
not fit. Its typed budget gives the kind, observed count, and maximum. Legacy
no-fit records without a reason remain inconclusive. The bounded writer reports
the crossing write count when measurement stops, not necessarily a full request
size. These values are local measurements/estimates, not billing usage.

Initial preparation failures settle as not assessed before dispatch with zero
attempts, no new model cooldown, no actions, and no status write. An incompatible
recovery profile settles with zero additional attempts and preserves any prior
durable attempt count. Neither outcome immediately requeues unchanged work or
prevents later candidates from progressing. Known-safe retries reuse the saved
prepared request and its identity.

The Jev preparation profile uses `jev-choice-input-v2` and a digest of the
complete compiled Choice decision and capabilities. It includes the model,
decision name, input templates/AST, ordered fit rules, question, and criteria.
This is an internal recovery identifier, separate from the `v1-prealpha` draft.
Recovery also checks exact constructed JSON, including framing, unknown fields,
input, and omission metadata. It does not refit dispatched work under changed
limits or policy. Unknown delivery still cannot authorize a resend. Existing
retention prunes completed candidate/prepared payloads together while retaining
typed reports and failure metadata; current-status evidence keeps its existing
retention exception.

## Execution and persistence

Discovery resolves global/project overrides and attaches active native main
sessions. A fresh attachment can seed bounded recent results and create only the
latest ready candidate. Retained earlier requests can supply partial context.
Restart resumes unchanged generations and durable progress. Definition or
admission changes cancel old-generation work and fence stale acceptance.

Live checks do not wait for the project backlog. Recent startup is one bounded
attachment operation. Initial publications advance delivery without creating
candidates until that branch attaches; coverage changes still apply. After the
initial receipt and session outbox head are consumed, the host reads at most 40
current terminal observations within 4 MiB in native order. It applies authored
filters and conditions and freezes only the latest ready candidate, including
for checks without `on.window`. Other attached branches can run independently.

One transaction commits observations, dependencies, selector state, the optional
candidate, and the attachment marker. Empty receipts get a marker. Existing
observed work is not reseeded, and unchanged restart cannot repeat startup.
Startup coverage remains partial. A startup-only activation copy uses an explicit
zero seed boundary while preserving immutable first-terminal metadata. Normal
live application keeps its original activation boundary. New first-terminal
results trigger normal live selection. Immutable
`first_terminal_publication` distinguishes them from historical corrections whose
new publication follows activation. Pending mutation order remains separate from
first-terminal logical order. Corrections to seeded historical evidence still
invalidate stale evidence without another attempt. Later non-window live
candidates remain separate. Definition transitions, session deletion, and Clear
Local Data remove attachment state at their existing lifecycle boundaries.
Historical assessment requires explicit isolated replay.

Each journal page commits current rows, immutable outbox events, cursor,
baseline, diagnostics, and continuation together under the current lease/claim,
source incarnation, exact project, parser revision, and fork-admission fences.
Missing page membership is not removal. Real history gaps remain partial across
pages and restart; bounded cold request history is also explicitly partial.
Live boundary checks permit appends without cancelling admitted work for a newer
discovery fingerprint. A detected identity/boundary change or invalid cursor
blocks for source recovery without automatic historical replay. Only strict
replay audits the complete consumed journal prefix; its cost scales with history.
See [source delivery](../session-coverage.md#burn-check-source-facts).

Committed source publication wakes definition-based checks. Native journal
readiness and YAML report evidence do not depend on legacy metric freshness.
Built-in smart-check scheduling and historical enrollment are suspended. Their
saved results, preferences, and implementations remain.

The master **Model-based Checks** setting controls YAML definition discovery and
activation. **Decision model** exposes Jev only. The Jev connection supplies model
credentials and usage accounting.
Partial fields, truncated text, and source gaps are frozen as explicit quality
facts; available evidence remains eligible. Missing expression values can make
one candidate not assessable, without blocking later session results. Core owns numbered
attempts and its known-safe transient retry. Unknown delivery does not authorize
a resend. SQLite persists selector progress, frozen candidates, claims, attempts,
answers, and status. Atomic acceptance can defer for ingestion catch-up and
recover a saved answer without another provider call.

`AssessmentAcceptance` carries a claimed candidate, validated decisions, and
planned actions into `CheckStore::accept`. The returned `AcceptanceOutcome`
states whether the transaction accepted, deferred, or rejected the assessment.

Independent actions use the same frozen input. The last applied status write
wins in authored order. No-write retains the original status-write assessment,
time, explanation, and evidence. A valid no-write assessment still advances the
accepted watermark atomically with its history and skipped action outcomes.
Neither uncertainty nor a below-threshold answer creates fallback success. A
branch success cannot clear another branch's failure.
The report uses eligible current evidence in the native 30-day cohort and counts
sessions before capping displayed targets. Snooze changes presentation, not
execution. Durable history supports recovery and provenance; the UI shows current
detail rather than an assessment-history browser.

Finding evidence reads the saved preparation attached to the status-writing
assessment, not the latest assessment. `get_installed_check_evidence` returns an
`assessed-input` item labeled **Assessed input**, whose JSON excerpt is exactly
`prepared.input`, plus an optional `input-omissions` item labeled **Input
omissions** containing the `FitReport`. Generic JSON values keep their structure;
the reader does not infer a reverse mapping from expressions to original tools.
When saved preparation exists, the response excludes original full requests and
tool results. A rejected preparation does not authorize fallback to source text.

Legacy assessments without saved preparation can return **Original source:
requested task** / **Original source: action result** items with an explicit
limitation that exact assessed input was not saved. These items do not establish
what the provider received. Evidence retrieval retains the current status,
definition generation/revision, exact native scope/ownership, report-cohort, and
source-validity gates. A prior finding can coexist with a later diagnostic only
while that finding remains eligible.

Current branch results require the session's current native environment and CWD
to match the activation scope. Moved or unknown-CWD sessions do not contribute
old status, runtime diagnostics, checking counts, or evidence through that scope.
Remaining trigger counts use only the report's current ready cohort. Journal
waiting/readiness uses the current delivery identity and its blocked, pending,
scheduled-attempt, and generation state, independently from legacy projection
failures. Blocked or exhausted delivery reports `source-unavailable`; it does not
become an indefinite waiting state.

Finite replay uses the same engine through an isolated `RunScope::Replay` and
Store. It settles each consumed trigger sequentially, preserves as-of-event
context, and bypasses live authored cooldown without bypassing evidence or
provider limits. It cannot mutate live rows. See the
[eval guide](../../apps/desktop/src-tauri/eval/README.md#session-replay) for diagnostics.

## Diagnostics and file opening

Installed-check reports expose a `diagnostics` array. Each item contains a stable
`code`, `severity` (`error` or `info`), friendly `title` and `message`, nullable
`source`, label/value `details`, and `affectedSessionCount`. A source contains a
backend-issued `reference`, display `path`, and nullable `line` and `column`.
Definition diagnostics retain YAML/compiler locations. Runtime diagnostics come
from typed preparation/execution records, not parsing internal error strings.
An informational shortening diagnostic comes from a ready omission report.
Every diagnostic includes a safe diagnostic-code detail. Runtime diagnostics add
the available decision and typed preparation context. `NoFit` reasons map to
`input-too-large`; resource stops use `preparation-resource-limit`, and old
unclassified no-fit records use `preparation-incomplete`. Typed no-fit explains
why the current permissions and effective retention limits cannot reduce further;
the other outcomes leave that question open. UI copy and details do not parse or display raw
failure strings or provider bodies.
Deterministic request-shape failures use `InvalidRequest` and actionable definition
copy. Criterion-count details identify the output field, observed count, and
provider maximum; they do not direct the user to reconnect Jev.

The report groups repeated errors by distinct affected session while preserving
separate files and decisions. It reads the latest eligible candidate/assessment
diagnostic per branch and clears stale errors after repair or a later successful
assessment. Diagnostic selection does not erase a prior valid status finding.
The frontend renders errors before empty/passing content, uses short card
statuses, and keeps technical details in a disclosure. Definition and fitting
errors navigate to the YAML, without a session link. The
[design contract](../../apps/desktop/design.md) defines the shared panel treatment.

`open_installed_check_definition({ sourceReference })` resolves the opaque
reference against discovered definition files. The renderer does not submit an
arbitrary filesystem path. The backend revalidates the local definition source
and uses the default application opener. Line/column are diagnostic metadata,
not an editor launch or line-jump contract. Open failures remain inline beside
the action; they do not replace the original diagnostic. Existing directory
access rules apply.
