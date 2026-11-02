# Burn Checks architecture

Burn Checks turn frozen source evidence into a saved status and explanation in
the existing Checks UI. Native readers supply evidence, the shared engine runs
the check, and the desktop owns connection gates and SQLite persistence. Model
checks use the selected System One provider. Local checks send no model request
but still require the selected configuration and base runtime gates.

This document covers ownership, execution, and recovery. Use the
[authoring guide](authoring.md#supported-subset) for supported language features,
[session coverage](../session-coverage.md#burn-check-source-facts) for source
limits, and [provider contracts](provider-contracts.md) for accepted wire shapes.
Passing one boundary does not prove the next.

The draft label is `v1-alpha`: experimental, not yet standardized, and open to
breaking changes without a backward compatibility promise. Beta and stable scope
are not set. Missing features are unassigned future work, not available behavior
or a promise from the broader draft.

## Ownership and dataflow

| Layer             | Owns                                                                                                                                               |
| ----------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- |
| `antiburn-local`  | Discovery, parsing, native identity/order, tool semantics, and source evidence                                                                     |
| `antiburn-checks` | Compilation, context preparation, selection, decisions, actions, and engine transitions                                                            |
| Desktop           | Watcher lifetime, committed evidence publication, SQLite Store adapter, credentials/provider admission and accounting, Tauri API, and presentation |

```text
native discovery and bounded live reader
  → ordered NativeEvidenceBatch: messages, results, cycles, activity, selection, removals, unsafe admission
  → committed publication with frozen native context; persisted host start/idle observations
  → StorePublishedEvidenceReader → PublishedEvidenceSource
  → frozen candidate → complete model decision set or durable local start → action planning
  → atomic assessment/actions/optional status/watermark
  → report event → existing Checks collection and detail pane
```

`CheckEngine` owns transitions. Expression evaluation and action planning remain
pure. Watch notifications request reconciliation; they are not result events.
Use the native adapters through bounded live readers. Claude, Codex, and Pi use
`read_native_file_page`; OpenCode V1 uses `read_opencode_live_tool_result_page`.
A reserved native-delivery task rotates file and journal turns, independently of
legacy metric indexing. Live delivery consumes each ordered evidence batch once;
its result-only view is not a second publication path. Projection passes do not
also publish native result snapshots for sources owned by live delivery. Source publication starts after
enabled/configured definitions reconcile and the existing first-run Checks gate
opens. Both routes require an enabled live activation for the exact native
environment and stored session CWD. Cold bindings require a supported main
session active within 180 seconds. Compatible admitted work, including interrupted
cold claims, continues after activity expires. Remote, WSL, known forks/subagents,
unknown/empty CWD, and historical-only sources do not start new live work.

When a source page reports more work, the worker schedules the next page
immediately on both routes. It saves an increasing claim order to rotate ready
sources without a clock tick, including after restart.
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

OpenCode V2 uses a separate capture worker for the pinned stock `v2.0.32`
contract. It reads the existing local service registration, proves that the
registered process has the selected SQLite file open, and subscribes to
`/api/event`. It does not start OpenCode or require a plugin. The worker consumes
the feed before taking a read-only SQLite transaction with per-session
`event_sequence` boundaries. Events at or before a boundary can seed joins but
cannot create new attempts. Only later, safely admitted public events enter the
shared publication outbox. Snapshot timestamps do not prove completion order.
Cycles also require an observed execution start after the verified boundary.

The V2 feed has no replay cursor, and stock OpenCode does not persist event
bodies. A disconnect or lost buffer therefore ends continuity. A new capture
epoch records a gap, resets affected selection windows, and admits safe future
results without replaying missed work. Request history before attachment stays
partial. V2 capture leases and sequence receipts do not reuse V1 journal state.
Stored-session parsing remains available when the live service is unavailable
or its database binding cannot be proven. Remote and WSL sources do not enter
this native live route.

Cursor Desktop follows a separate ordered snapshot result path. Discovery reads
composer headers and bubbles in one SQLite transaction. The native parser emits
characterized terminal tool results and their user-slot prefix. Winning complete
snapshots reconcile through the native-result outbox; startup binds the exact
source generation and incarnation. Desktop admission requires a native inline
main source labeled `cursor-desktop:*`, no WSL source, and current ready cached
evidence. This route does not use file/journal cursors or supply public user,
assistant, call, cycle, or host-session events. Requester model/mode and operation
paths remain unavailable. See the [Cursor source contract](../session-coverage.md#cursor-desktop-yaml-tool-evidence).

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

An empty project page can advance delivery past unrelated publications. The reader
captures the global high-water mark before its project query, so a later
publication cannot be skipped. A publication that waits for a session attachment
still holds the cursor. The store verifies that the skipped range has no matching
project publications before it saves progress. This does not change observation
order or create candidates.

New public messages, calls, and cycles keep separate namespaced identities and
first-eligible publication boundaries. Corrections retain their original logical
order and event context. Calls and results cannot deduplicate each other. A mixed
message window applies each event alternative, its filters, and top-level
conditions before counting one shared rolling collection. Cycle and session
candidates have their actual roots and no fake message or result.

Pending/progress mutations do not reset selected terminal order. Corrections that
remain filtered do not create a source gap or a new attempt. A correction that
makes branch membership uncertain invalidates unfinished work and accepted
evidence for that branch, including earlier-page candidates that do not select
the corrected identity. Publication, selector state, and invalidation commit
atomically and survive restart. Other branches remain valid. Stale or duplicate
events acknowledge delivery without replacing newer observations, advancing
engine observation order, or creating candidates.
Accepted idle history retains its recorded boundary; it does not prove that the
current source is healthy.

Claude request-origin upgrades use a separate execution gate, not accepted-evidence
invalidation. A source-bound segment reset and the sanitized receipt commit before
new evidence becomes usable. Claim, dispatch, and acceptance require the live
activation to consume that boundary and select later publications. Startup readers
exclude earlier context too. This preserves accepted history and accounting while
retiring unfinished work with unproved request origins.

Unsafe identity, order, or ownership is separate from partial history.
`NativeAdmissionFailure` carries that source failure through an explicit
`UnsafeUpdate`, including when no replacement message exists. The host blocks
affected dispatch and acceptance before selector consumption and invalidates
unsafe saved evidence. Cursors and snapshots retain this state across restart.
It applies to users, assistant text, calls, results, and cycles. A clipped request
prefix or cache limit alone does not authorize this invalidation. Usage and
unknown-delivery records remain intact.

`NativeFilePage.admission_failure` reports a known defect even when output limits
defer its ordered fact. The desktop applies that state before accepting the
receipt. Snapshot failure state remains separate from retained event eviction.

Native member bindings and normalized observations have different version
tokens. Transient `source_evidence_version` metadata carries the immutable
publication's native token without changing authored input, observation hashes,
or checkpoint encoding. A fresh native-token-only change can invalidate a cycle
even when its normalized message is unchanged. An exact duplicate cannot.

`EffectiveActivation.bootstrap_observation_position` is the observation boundary
for a new generation. `bootstrap_publication` is its separate publication cursor.
The local evidence sink uses `tool_result_revision_key` to retain revisions; that
key is not a logical result identity.

The local sink freezes user-message prefixes and main-session admission in
`ToolResultContext` for results and `NativeEventContext` for other native events.
Candidates freeze selected records, linked calls, scope,
positions, completeness, and revisions. Delayed provider work does not reread
mutable source rows. Missing required evidence prevents assessment. Selected
corrections/deletions can invalidate current status without writing success;
unrelated appends do not invalidate a candidate. Project reads filter exact
environment/session CWD before the page limit. Old payloads without context stay
unavailable.

Native publication cancels stale dependent work and marks its saved evidence
invalid in the same transaction, before the engine reads the change. The same
gate protects startup tool results with untyped observation bindings. Database
migration 92 indexes correction updates, removals, and dependency identities
without rewriting saved rows. Routine writes recheck changed identities. A loss
of continuity still checks all native dependencies for that session.

Definitions that reference current global or repository inventories request a
bounded host snapshot during selection. The host binds the read to the native
publication or host observation, agent, branch, exact stored CWD, trusted repository
root, and recorded directory grants. This path supports every observation kind.
A top-level inventory condition captures before evaluation; candidate-only reads
capture after filters, conditions, and any window minimum pass.
Definitions that do not reference these inventories do not trigger collection.
The candidate freezes the returned current scopes, limits, and revisions before
provider preparation. Delayed work and retries use that snapshot rather than
rereading files. A current-file edit does not rewrite the selected tool result
or establish which instructions applied when it ran.

Current session selection is separate from requester attribution. Only explicit
native selection records update it. The host freezes the last proven selection
through the event for `session.model` and `session.fast_mode`; unavailable values
stay absent. Selection does not count as activity or relabel earlier requests.
Common model/mode filters read only their event family scope. Turn attribution
uses the complete, nonempty native generating-request set, including requests
without public text, and derives model and mode independently.

Cycle messages use the same closed normalizers as standalone messages. A complete
cycle binds every unique public member to its exact current kind, revision,
publication, and hash. Native current dependencies remain distinct from the
selected observation's own hash. Member correction or removal cancels stale
dependent work, including candidates created earlier in the same page. A new
public/host publication and source context share the core 4 MiB, 262,144-value,
depth-64 admission bound. Cycles have at most 4,096 messages and exact member
bindings; smaller native bounds still apply. Unavailable cycles do not erase
independent usable messages. See [public-event admission](specification.md#52-public-event-admission-bounds).

The instruction and skill definitions select whole `global` and `repo` scope
objects. This keeps an unavailable array distinct from a known empty array and
lets known entries remain useful beside missing scopes. The current expression
syntax cannot build a selected-field object while omitting unavailable fields.
These inputs can therefore also contain the other inventory in that scope.
Each question limits which inventory it uses. The canonical
[Ignored instructions](../../.antiburn/checks/ignored-instructions.yml) definition
can truncate only unused skill descriptions inside those scopes. It preserves
instructions, metadata, names, paths, requests, and arguments.
[Skill opportunities](../../.antiburn/checks/skill-opportunities.yml) preserves its
inventory unchanged. Large protected input can still reject preparation.

OpenCode request construction excludes synthetic text in snapshot and journal
paths. A mixed message retains its real text. A generated-only message or
replacement cannot fill a missing user request and leaves context partial.
The compiler admits all nine check-facing categories: `shell`, `read`, `search`,
`edit`, `write`, `delegate`, `skill`, `mcp`, and `other`. Each source needs a reviewed
native tool identity to emit a category. These categories are independent of the
metric categories used by deterministic checks. Unsupported source facts stay
absent; a collapsed metric `other` does not prove a write, delegation, skill, or
MCP operation.

## Session observation boundaries

The desktop owns persisted attachment and activity state. One attachment emits
`session_start` with reason `attached`, including for an empty source. Existing
admitted attachments initialize once after successful upgrade validation without
repeating the result seed. `new` and `resumed` remain compiler-valid but are not
desktop-produced reasons.

The attachment stores an observation epoch, activity revision, local observation
start, next idle deadline, emitted revision, and last proven selection. Genuine
admitted native activity moves the deadline to 120 seconds after local
observation. Historical baseline facts, exact replay, selection changes, UI
events, and notices do not. Optional native activity timestamps remain separate
from the timer clock. Lost source continuity cancels unfinished idle work and
requires a fresh interval after recovery. This observation does not measure
model-active or tool-only time and does not change the native Fast mode overuse
check.

An indexed next-deadline wake requests finite source catch-up, not one polling
loop per attachment. Idle publication requires a usable frontier and rechecks the
source receipt, label, CWD, incarnation, attachment epoch, continuity, and activity
revision in the source transaction. The emitted revision and publication save
together. Restart can finish an overdue interval without repeating a published
one. New activity cancels unfinished idle candidates before selection consumes
them; claim, local start, dispatch intent, and acceptance recheck typed dependencies.
Accepted idle history remains valid at its recorded boundary. Start candidates
bind the exact attachment without requiring unchanged later activity.

Native publications freeze explicit session selection as separate immutable
metadata. They do not change native payload bytes or exact binding hashes.
Corrections copy the original selection, including unknown selection. Readers
never fill old events from the latest attachment or source state. V2 source
selection binds its capture epoch and claim fence. A reconnect or scoped
recovery cannot reuse selection from the lost claim.

## Input preparation and fitting

Core evaluates recursive input templates once against the frozen candidate.
Whole expressions preserve JSON types; embedded expressions produce text. Pure
functions, dynamic brackets, projections, and nested map aliases share the same
closed-record and phase validation. Dynamic access cannot turn a normalized
record into arbitrary JSON. Source text is never evaluated as another template.

Evaluation tracks value identity separately from selected evidence dependencies.
Map aliases and indexes retain the original identity and source path. `fromJSON`
retains dependencies but creates fresh object/array identities. A map selects
collection membership, evaluated conditions, and emitted values; excluded unused
fields do not become dependencies. Missing values remain unavailable through
functions, conversion, and nested maps. Preparation and action planning check
membership traces against frozen completeness facts. Operations on
`session.requests` require `session.requests_complete == true`, including after
JSON conversion. Whole copies of available partial history remain usable. A
missing collection does not invalidate unrelated available fields.

Top-level `if` has a separate bounded budget before window selection. One shared
assessment budget covers recursive input preparation across all blocks and status action planning,
including action conditions, dynamic results, and messages. The deployed bounds
are depth 32 from one, 10,000 steps, 1,000 visited items, 8 MiB of cumulative
generated data, and a separate 8 MiB cumulative materialization allowance. Fixed
container entries count as visited items. Source and alias copies reserve their
materialized bytes before cloning. Fixed text/container construction counts
before allocation. Exhaustion
stays in effect and prevents all assessment writes. See
[shared evaluation limits](authoring.md#shared-evaluation-limits).

`EvaluationBudget` is serializable. Saved preparation retains consumed costs;
recovery validates the snapshot and restores it for action planning. A nested
template, later action, retry, or restart does not grant a fresh budget for
prepared work. Invalid snapshots do not authorize a budget reset. Incompatible
old pending preparations stop locally without inference or reconstruction. A
bounded read-only legacy decoder preserves accepted historical input and
provenance; it cannot authorize execution. Its prior-format bounds are 64 MiB,
131,072 JSON values, and depth 72. These bounds cover the previously reachable
input, report, and request wrappers, rather than the smaller current
preparation-set limit. One malformed candidate does not stop unrelated recovery
rows. Database errors still propagate.

For local checks, the assessment budget covers action planning without a decision
input or provider preparation step. Local recovery requires its saved budget.

The compiler retains decision-level `fit` rules in YAML order, validates the path grammar and
declared input roots, and rejects overlap. Fitting expands concrete targets once
in rule and original array order. Missing optional properties and empty arrays
are skipped; incompatible present traversal or target types reject preparation.
The original candidate and source evidence remain unchanged. Original input
admission runs before expansion. The [deployed preparation bounds](authoring.md#preparation-limits)
cap input bytes/nodes/depth, rules, path bytes/segments, matched leaves, traversal
visits, attempted turns, and cumulative complete-request measurement. The desktop
evaluator's 64 MiB work budget covers input measurement and complete-request
construction/validation across all blocks and turns, including accepted requests.
Its consumed count remains immutable in saved preparation. A separate shared
64 MiB saved-validation pass cannot fit or change requests. Both are separate
from source retention and provider request/token/resident limits.

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

The selected System One adapter builds the full provider request and supplies
validation at the provider boundary. `state.evidence` holds the prepared input. When fitting changes
it, `state.input_omissions` adds explicit framing and a `targets` list with each
changed path, `strategy`, `original_count`, and `retained_count`. Counts mean array items for
`recent` and original UTF-8 bytes for `truncate`, excluding marker bytes. This
metadata describes preparation loss separately from source-quality limits.

SQLite stores the preparation result in
`burn_check_candidate.preparation_json` before durable dispatch intent. A ready
record holds one `PreparedAssessment` with its profile, saved evaluation budget,
consumed fitting work, and authored-order blocks. Each block holds its name,
original evaluated input, fitted input, exact provider request, and omission report.
A rejected record holds a typed failure code, detail, optional
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

The preparation profile's compatibility digest binds the complete compiled
decision set, effective per-type capabilities, construction version, and
sanitized connection descriptor. The descriptor
contains the provider, endpoint digest, model/revision, connection revision, and
response mode. It contains no credential or raw endpoint. The decision policy
includes every block name, input template/AST, ordered fit rules, named question,
type, and ordered criteria.
This is an internal recovery identifier, separate from the `v1-alpha` draft.
Recovery also checks exact constructed JSON, including framing, unknown fields,
input, and omission metadata. It does not refit dispatched work under changed
connections, limits, or policy. Unknown delivery still cannot authorize a resend.
See the [selected-connection evaluator](../../apps/desktop/src-tauri/src/jev/decision_evaluator.rs).
Existing retention prunes completed candidate/prepared payloads together while
retaining typed reports and failure metadata; current-status evidence keeps its
existing retention exception.

Core bounds the full ready record, including its status tag, at 8 MiB, 65,536
JSON values, and depth 64 with the root at one. Original inputs, fitted inputs,
all requests, reports, names, and the profile share this bound. Append admission
measures borrowed data before retaining another block. Typed results have a
separate 512 KiB, 65,536-value, depth-64 bound. Worst-case result-capacity
admission runs before dispatch using exact declarations and all optional detail
shapes; actual results are measured before save. Strict bounded decoders reject
duplicates before insertion, crossing values before allocation, unknown versions,
and invalid budgets. There is no syntax-only independent-block count limit.

## Local execution and recovery

`CompiledCheck.decisions` is an authored-order `Vec<DecisionBlock>`. An omitted
YAML declaration selects local execution; a present empty, null, or invalid
declaration does not. `question_requirements()` returns no requirements for
local work, rather than an empty model contract. Each model block supports one
to eight mixed Choice, Score, and Noul questions. Local actions
cannot read undeclared `decisions`, and message actions remain unsupported even
behind false conditions.

Core claims and schedules the same frozen candidate under branch single flight.
It checks the provider-free runtime gate and top-level condition before local
start. Local execution does not call the model evaluator's `prepare`,
`validate_prepared`, or `evaluate_prepared` methods, or its
provider-preparation notification hook. A valid local answer
has no decision results and zero attempts; crossed local/model result shapes fail.

`PreparationRecord::Local` contains `LocalPreparation`: internal version 1,
`started_at_unix_ms`, and the saved `EvaluationBudget`. This is not a YAML version.
It stores no model input, request, endpoint, credentials, or provider profile.
`CheckStore::record_local_start` atomically saves that preparation, the canonical
zero-decision, zero-attempt typed answer, and the first authored cooldown, then sets
the existing `answered` state. A zero cooldown creates no rounded future delay.
Repeating the same current start changes nothing; a different record or stale
claim cannot refresh the start or cooldown. Local work creates no dispatch
intent, attempt row, usage callback, or usage record.

Action planning uses the saved budget and frozen evidence dependencies. The
normal acceptance transaction commits ordered outcomes, an optional status write,
and the watermark together. Budget failure discards every tentative write.
No-write acceptance keeps the prior status-writing assessment and evidence.
Recovery restores an `answered` local start with zero attempts and does not start
it again. Missing, wrong-kind, unknown-version, or invalid-budget local
preparation stops that candidate without model preparation or a budget reset.

The host separates base admission from model discovery/capability failure. A
ready local check skips capability discovery/cache errors, model warm-up, client
construction, transport admission, and inference for every selected route. It
still requires the master setting, valid selected configuration, existing
credential/authentication rules, repository/source readiness, activation
generation, and shutdown gates. An ordinary unavailable model capability does
not waive a failed base gate.

The provider-free gate applies to selected execution and factory answer recovery,
including before metadata publication. A blocked saved answer remains recoverable
without extending its cooldown. The Store adapter samples its publication gate
before the SQLite transaction; durable master, authentication, connection-change,
claim, generation, source, and watermark checks remain in the transaction.
A commit already admitted at the runtime sample can finish during shutdown;
this is not a serializable shutdown transaction. Connection changes retain the
existing cancellation and A-to-B-to-A protection.

## Decision routing and connection transitions

Settings selects Jev, compatible Ollama, Cloudflare, or Custom through the
existing `SystemOneConnection`. Custom retains direct System One and
Cloudflare-envelope response modes. The YAML compiler rejects a decision-level
`model` as an unknown field. Observed coding-agent model fields remain evidence.
For model checks, the host checks each selected question type, its criterion
interval, and referenced optional details separately from language validity.
Choice probabilities are label maps; Score probabilities are arrays in authored
criterion order. Choice/Score confidence is a separate capability and cannot
replace probabilities. Noul is a finite yes-probability with closed empty
details, not a Boolean or a separate confidence value. Whole-object and indirect
reads must collect the same detail requirements as direct access. Unknown output support and failed
discovery do not become available because numeric overrides exist. See
[host admission](../../apps/desktop/src-tauri/src/installed_checks.rs) and the
[connection contracts](authoring.md#decision-connections).

The [worker snapshot](../../apps/desktop/src-tauri/src/jev/worker.rs) captures the
selected connection, resolved credential, and generation under one lock. The host
resolves capabilities once for a model assessment before fitting or dispatch intent.
The evaluator owns that snapshot through preparation and attempts. Costly
preparation uses the bounded blocking-preparation path rather than the async
coordinator. Credentials stay in memory or secure storage and never enter saved
preparation, diagnostics, or analytics.

[Settings transitions](../../apps/desktop/src-tauri/src/jev/settings.rs) that
change or remove the selected connection, or explicitly select another connection,
suspend new admission before the Store [durably cancels unfinished candidates](../../apps/desktop/src-tauri/src/store/burn_checks/connection_changes.rs).
Inactive connection edits and removals leave active work unchanged.
A cancellation failure keeps admission suspended. Definition identity,
activation, source checkpoints, observations, attachment, windows, and accepted
status remain intact. Returning from connection A to B and then A does not revive
cancelled candidates, including after restart. Claim and generation checks fence
late status acceptance and stale authentication failures. Switching connections
does not authorize retry or automatic failover. Compatible retries reuse the
exact saved request and successful sibling answers; incompatible preparation
stops without another send. Provider identity is provenance, not part of attempt
identity, so a switch cannot bypass unknown delivery.

The [shared dispatcher](../../apps/desktop/src-tauri/src/jev/dispatch.rs) preserves
request-slot, byte, pacing, response-size, and local inference-slot bounds. It
uses separate 60-second admission and transport deadlines; the transport deadline
includes the Ollama inference-slot wait. Timeout or cancellation after sending
has unknown delivery and cannot authorize automatic resend.

Each transport records available reported usage before typed answer validation,
including both Custom response modes. The host binds accounting to the original
snapshot's provider/model even if Settings changes or publication is cancelled.
The [idempotent usage sink](../../apps/desktop/src-tauri/src/store/burn_checks/usage.rs)
counts each numbered attempt once and permits known usage to replace an unknown
outcome without double counting. The [pricing ledger](../../apps/desktop/src-tauri/src/store/usage_accounting.rs)
uses route identity: Custom and Cloudflare do not inherit Jev prices from a model
name. Missing prices remain explicit. Persisted preparation retains the actual
decision provider/model, separate from the coding-agent model in source evidence.

## Sequential model execution and recovery

The engine evaluates every block input from one frozen candidate with one shared
expression budget. The selected evaluator prepares and fits all complete
multi-question requests before the first dispatch. A failure in any block's
preparation causes zero inference sends. Fitting uses authored rules only, with
no summary calls, implicit loss, or fixed-question changes.

Blocks execute sequentially in authored order within the existing candidate
slot and branch single-flight reservation. `DecisionAttempt` carries a declared
decision name and number one or two. Its identity helper encodes candidate,
decision, and attempt without provider data. Store uniqueness and usage
deduplication use that same identity. Only a known-safe transient failure
permits attempt two. Assessment totals and invocation-new intents use `u64`.

Durable progress owns attempt counts. `record_decision_answer` settles the exact
intent and saves a declaration-bound typed block answer atomically. A current
claim can continue to the next block; stale settlement cannot authorize a send.
The final answer moves the candidate to answered. Before action planning, core
validates the complete exact result set. Missing, extra, duplicate, unknown, or
wrong-type outputs prevent all actions and preserve incurred usage.

Restart reuses saved successful siblings and exact unfinished requests. A safe
settled partial candidate can release and requeue only without an unresolved
intent. It retains preparation, responses, attempts, and cooldown, and cannot be
coalesced as unsent rolling work. Busy claims remain retryable. An unresolved
intent becomes unknown and stops every further automatic send for that candidate.
Current source, generation, connection, and runtime gates apply before each intent.

Cooldown starts only at the candidate's first durable intent. Later decisions,
safe retries, and resume cannot extend it. Resume bypasses only its own cooldown.
When all answers are saved, publication validation uses saved sanitized profile
facts and pure request-construction checks. It requires no discovery, client, or
transport admission, but keeps master, selected-configuration, authentication,
source, claim, generation, connection-transition, and shutdown fences. Existing
atomic acceptance still owns history, ordered outcomes, status, and watermark.

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

This recent seed remains result-only, including for mixed message checks. Other
public kinds use a separate future-only baseline and first-eligible publication.
The named file-cursor-4 and journal-4 converters preserve committed progress,
pending result joins, source bindings, and the original result baseline when
upgrading to revision 5. They set a new-event frontier without replaying old
result identities. Parser 67 accepts parser-63, parser-64, parser-65, and parser-66
cursor/state-5 receipts, and the named parser-62/cursor/state-4 predecessors
through their exact converters. Separate converters accept parser-64
file cursor 1 and V1 live state 2 only from producer commit
`143d4b05681938cb362e828ecc27d2e082638176`. Actual old-encoder synthetic
fixtures define the accepted shape, not a published release range. File conversion
validates the original identity and hash anchors; V1 conversion requires matching
stored and encoded parser 64 and validates the session, native owner, sequence,
origin, and boundary. Both retain result progress and pending joins, then freeze
the validated current source frontier for new public event kinds. They do not
infer missing attribution or erase history limits. Partial publication retains
the consumed position and frozen public frontier across restart. Decoder
eligibility does not grant readiness: a validated layout-5 page must commit under
the existing claim and source fences first. Conversion leaves accepted inputs,
history, paid attempts, cooldowns, and usage intact. Unsupported layouts keep
their receipt and prior assessments but remain blocked without automatic rewind
or reseeding. Unknown revisions fail closed. The historical
parser-65 Claude request-origin boundary remains unchanged. Core checkpoint
version 1 and existing result encodings remain unchanged. Missing new
first-eligible fields do not authorize historical new-kind events.

Parser-66 journal cold retries retain their original parser-number hash binding
until their pinned page commits. Advancing to parser 67 does not change the
retained source window or create a new provider attempt.

Each journal page commits current rows, immutable outbox events, cursor,
baseline, diagnostics, and continuation together under the current lease/claim,
source incarnation, exact project, parser revision, and fork-admission fences.
Missing page membership is not removal. Real history gaps remain partial across
pages and restart; bounded cold request history is also explicitly partial.
The transaction also stores generic evidence, exact native dependencies, activity,
selection, and its source receipt. Shared native publications and host events for
the exact activation, branch, incarnation, and epoch are filtered before page
limits. Whole-message removals expand through durable owner bindings across all
message kinds; tombstones prevent late restoration. Bounded file/V1 removal
continuations keep their source cursor until complete. An oversized V2 removal
batch rolls back with its capture sequence. Unbound removals record a gap rather
than guessing current membership.
Live boundary checks permit appends without cancelling admitted work for a newer
discovery fingerprint. A detected identity/boundary change or invalid cursor
blocks for source recovery without automatic historical replay. Only strict
replay audits the complete consumed journal prefix; its cost scales with history.
See [source delivery](../session-coverage.md#burn-check-source-facts).

Committed source publication wakes definition-based checks. Native journal
readiness and YAML report evidence do not depend on legacy metric freshness.

The master **Run checks** setting under Model-based checks controls YAML check activation. It
preserves existing saved preferences. A fresh setup without a connection stays
off, and malformed or unreadable storage disables execution. Discovered global
or repository definitions supply check cards and navigation. Without a discovered
definition, these surfaces and placeholder errors stay absent; the Settings
master toggle and connection setup remain available. A discovered check without
a configured System One provider gets an actionable setup diagnostic in its detail.
Definition discovery remains active with **Run checks** off, including at cold
startup. It reads and parses bounded local YAML files, refreshes directory
watches, and publishes names, diagnostics, and paused cards. It keeps the existing
256 KiB/file and 128-files/folder limits and directory-access gates. This path does
not restore provider configuration, discover model capabilities, catch up session
sources, or execute local/model assessments. Provider readiness still gates live
delivery and execution, not definition visibility.

The connection list under **Model-based checks** separates saved configuration
from selection. Native save takes an explicit create or update intent. Updates
require an existing profile and matching revision, so a stale editor cannot
recreate a removed profile. Creating the first profile selects it without
enabling checks. Other saves preserve selection, including a confirmed null
selection. **Use** explicitly selects an unchanged saved profile. Removing the
selected profile pauses checks and clears selection without choosing a
replacement. Empty profiles and null selection persist across restart. Inactive
edits and removals preserve active work and shared credential references.
The settings snapshot exposes a safe pending-operation descriptor and native-owned
display numbers. The private journal retains before/after settings, revisions,
and credential-slot references, never secret values. Explicit Retry binds to a
unique native operation token. A separate renderer request tag binds Save
completion to its originating draft; matching model fields alone are not proof.
Reads expose recovery state without completing it. Retry returns a typed completed
or rolled-back outcome, restores the correct selected runtime from current
durable enablement and authentication state, and preserves unknown-delivery
protection. Native display-number allocation keeps concurrent Add labels distinct
without changing model connection data or frozen requests.
Partial fields, truncated text, and source gaps are frozen as explicit quality
facts; available evidence remains eligible. Missing expression values can make
one candidate not assessable, without blocking later session results. For model
checks, core owns named per-decision attempts and their known-safe transient retry. Local
checks retain zero attempts. Unknown delivery does not authorize
a resend. SQLite persists selector progress, frozen candidates, claims, attempts,
answers, and status. Atomic acceptance can defer for ingestion catch-up and
recover a saved answer without another provider call.

`AssessmentAcceptance` carries a claimed candidate, validated decisions, and
planned actions into `CheckStore::accept`. The returned `AcceptanceOutcome`
states whether the transaction accepted, deferred, or rejected the assessment.

Independent actions use the same frozen context and any validated decision results.
Status results can be whole expressions returning exactly `success` or `failure`;
messages can use whole or embedded expressions and must produce nonblank text.
Invalid or unavailable action values skip that action without blocking later
actions. Shared-budget exhaustion instead stops the whole assessment, including
earlier planned writes. The last applied status write wins in authored order.
No-write retains the original status-write assessment,
time, explanation, and evidence. A valid no-write assessment still advances the
accepted watermark atomically with its history and skipped action outcomes.
The engine does not create fallback success for uncertainty or below-threshold
answers. Four maintained examples explicitly write success when their failure
condition does not match. Ignored instructions instead requires `no_conflict`
with label probability at least 0.95. Its uncertain answers leave the saved result
unchanged. A branch success cannot clear another branch's failure.
The desktop shows Not assessed until an accepted status action writes success or
failure. Queued or running work adds Checking beside the last saved result,
without a separate group. Waiting progress belongs in detail. These progress
states do not create a status write or add a clean session.
The report uses eligible current evidence in the native 30-day cohort and counts
sessions before capping displayed targets. Snooze changes presentation, not
execution. Durable history supports recovery and provenance; the UI shows current
detail rather than an assessment-history browser. An unfinished local start can
show Checking from its bound saved local preparation; a claim alone does not
prove that local evaluation started.

Finding evidence reads the saved preparation attached to the status-writing
assessment, not the latest assessment. For model preparation,
`get_installed_check_evidence` returns a saved original-input item per decision,
an **Assessed input** item with the exact saved
fitted input, and an optional **Input omissions** item containing that block's
`FitReport`. Each item identifies its decision. Generic JSON values keep their structure;
the reader does not infer a reverse mapping from expressions to original tools.
The reader does not substitute reread full requests or tool results for saved
fitted input. A rejected preparation does not authorize fallback to source text.
Historical accepted single-question preparations retain saved fitted input,
omissions, and available provenance without inventing an unsaved original input
or deriving a decision name from the current definition.

For `PreparationRecord::Local`, evidence reads that status-writing candidate and
returns `local-context`, labeled **Frozen local context**. It contains the frozen
values available to the status actions, not a claim that every value was read.
The structured view uses **Raw local context** for its raw section. No model
request was sent, so this path returns no omission report, provider provenance,
or missing-model-input warning. No-write acceptance and retention preserve the
earlier exact status-writing context.

The [local evidence report](../../apps/desktop/src-tauri/src/installed_checks_report.rs)
reads the evaluator's narrow `prepared_profile_provenance` helper. A **Decision
connection** item identifies the status-writing assessment's provider, model,
and available model/connection revisions. It does not substitute current
Settings or expose an endpoint, endpoint digest, or credential. Diagnostics use
the same helper for available connection facts rather than parsing the private
preparation descriptor independently.

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
provider limits. It cannot mutate live rows. Durable-total attempts and
invocation-new intents are separate `u64` counters; only new intents charge the
invocation allowance. Before the first new intent, allowance must cover one
possible send per unfinished block, then each intent and retry checks it again.
Exhaustion settles `LimitReached` terminally with no actions, watermark, or
automatic later resume. Saved-answer publication consumes zero new intents;
local replay still works with zero provider allowance. See the
[eval guide](../../apps/desktop/src-tauri/eval/README.md#session-replay) for diagnostics.

## Diagnostics and file opening

Missing `fit` rules alone do not produce a diagnostic. Input-size explanations
require an actual preparation or provider failure. Fitting retains authored loss
permissions and never adds automatic shortening or summary calls.

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
selected-route maximum; they direct model changes to Settings rather than a YAML
decision `model` field or a generic reconnect instruction.

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
