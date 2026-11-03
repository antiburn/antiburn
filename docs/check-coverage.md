# Burn Check Source Coverage

This document covers local passive session evidence and the desktop's read-only
current resource inventory. Session evidence supports historical claims. Current
inventory supports claims about what is enabled now. It cannot prove what a past
session exposed. Existing persisted output from the reviewed Pi example extension
is a passive input. The OpenCode WSL discovery limits are recorded in
`session-coverage.md`.

Copilot, Cline, and Kiro file bundles use discovery fingerprints for evidence
claim matching. Outdated claims stop before parsing or row writes. Separate
read-time checks still apply; matching discovery fingerprints alone do not prove
an unchanged database snapshot or expand finding and clean-result eligibility.

See [`session-coverage.md`](session-coverage.md) for discovery, framing, parsing,
companion-source, and provider-route coverage for the same source formats.

## Burn Check definitions

Accepted native readers emit ordered `NativeEvidenceBatch` facts under their
characterized contracts: user messages, completed public assistant text, explicit
calls, results, supported response cycles, activity, recorded selection state,
and removals. Local parsing freezes available user prefixes and main-session
admission in result or event contexts. The desktop publication store retains
these facts, and `StorePublishedEvidenceReader` converts them to core source
events. SQLite/core persistence retains scoped activation, selector
checkpoints, exact claims, attempt recovery, and atomic acceptance. See the
[supported subset](burn-check/authoring.md#supported-subset) for current runtime
admission and the [source facts](session-coverage.md#burn-check-source-facts)
for exact producer/evidence limits. These fixture-backed shapes do not establish
all installed versions or arbitrary retained histories. Remote/WSL and child
sources are outside this definition-based path.

The native file page API freezes evidence with bounded user-slot context and
exact file-byte positions. An incomplete final record stays pending across
restart. Page limits retain undelivered evidence; per-record or oversized-fact
limits report omissions and keep history partial. Completed calls release full join
payloads, while non-evicting identity summaries prevent rejected or retired call
IDs from later acquiring a new argument binding. A possible summary match can
withhold evidence; it is not proof that an event did not occur. See
[native file delivery](session-coverage.md#native-file-delivery) for the exact
read, call, request, and result bounds. These limits do not expand producer support
or prove whole-session absence.
Cursor Desktop uses the separate
[ordered snapshot result route](session-coverage.md#cursor-desktop-yaml-tool-evidence),
not the file-page reader or the five public-event live routes.
An identity-summary match alone does not prove a source conflict. Proven exact
Claude replays do not create new public work or revoke earlier evidence.
Retained Pi requests remain exact conflict proof after the general ID cache
fills. Proven Codex ordinal regressions revoke admission across earlier pages.
Terminal replay proved by a retained original or latest digest creates no
activity or new idle boundary. Intermediate correction history is not exhaustive.
Complete malformed or oversized file records deny later admission for that
cursor; pending partial writes do not.

Proven unsafe identity, order, or ownership travels as an explicit source
failure, not a partial-history flag. It invalidates affected public messages,
calls, results, and cycles across pages and restart. Pending work cannot dispatch
or accept, and later windows cannot reuse unsafe facts. Usage and unknown-delivery
protection remain intact. Ordinary clipped history can still support usable
evidence; it does not by itself authorize an unsafe reset.

Live checks attach to supported active native main sessions independently of the
historical backlog. New scopes and cold bindings require activity within 180
seconds and an enabled exact native live activation. Unknown or empty CWD,
remote, WSL, known fork/subagent, and historical-only sources are excluded from
new live work. Compatible unfinished admitted work drains after activity expires;
consumption does not reclassify delayed publications by their current timestamp.
V2 execution success, failure, or interruption ends unfinished local joins. This
cleanup creates no result and keeps accepted evidence.

After the initial receipt and session publications are consumed, fresh attachment
seeds at most 40 current terminal tool results within 4 MiB in native order. It
applies authored filters and conditions and freezes only the latest ready
candidate, even without `on.window`. Startup coverage stays partial. The marker,
selector state, observations, dependencies, and optional candidate commit
atomically. Empty seeds get a marker. Unchanged restart does not repeat startup;
existing observed work is not reseeded. Other attached branches can run while a
branch initializes. Normal live selection admits new first-terminal results after
attachment. Immutable
first-terminal publication metadata prevents a later historical correction from
becoming a new attempt after activation. Retained pending/running results can
enter when they first become terminal. Corrections to seeded evidence invalidate
stale results without another attempt. Later non-window live candidates remain
separate. Historical work requires explicit isolated replay.

Other message kinds and cycles are future-only. Their native first-eligible
position and first-eligible publication remain separate from the latest mutation
and the older result seed boundary. A historical correction, parser upgrade, or
restart cannot make them new live work. Missing first eligibility cannot authorize
a new-kind trigger. Host attachment events use their exact activation, branch,
incarnation, and attachment epoch instead of a fabricated native result.
Partial first-page file publication retains the original startup position and
live baseline across retry and restart. Later appends remain eligible; delivery
limits do not move them into historical work or discard pending events.
Partial first-page journal publication also keeps its original recent window
and result/public baselines. Retry validates and finishes that window before
reading later appends. The incomplete cold receipt cannot enable startup seeding
or report readiness; warm continuation remains ready under the normal gates.

### Reachable event families

A definition selects one family: message events (`user_message`,
`assistant_message`, `tool_call`, `tool_result`), response cycles (`turn_end`), or
session events (`session_start`, `session_idle`). Message alternatives filter
before one shared mixed-message window counts them. Cycles and session events
have no rolling window. Each candidate uses its actual `message`, `turn`, or
`session` root; cycle and session candidates do not contain fake tool results.
Message actions remain unsupported, including behind false conditions.

The table describes native live routes only. Each entry requires its pinned
accepted shape, safe identity/order/ownership, source baseline, and exact live
activation. The [public source contracts](session-coverage.md#public-messages-and-live-event-eligibility)
give producer commits, finality, correction, and bounded-history limits.

| Native route                      | `user_message`                 | `assistant_message`                                | `tool_call`                                       | `tool_result`                                    | `turn_end`                                              | `session_start`      | `session_idle`               |
| --------------------------------- | ------------------------------ | -------------------------------------------------- | ------------------------------------------------- | ------------------------------------------------ | ------------------------------------------------------- | -------------------- | ---------------------------- |
| Claude Code CLI 2.1.278 / 2.1.309 | Accepted user slot             | Unavailable                                        | Complete explicit `tool_use`                      | Terminal result with exact call binding          | Unavailable                                             | Host `attached` only | Observed 120-second boundary |
| Codex 0.160.1                     | Retained user proof            | Completed `AgentMessage` plus exact raw projection | Characterized explicit calls                      | Terminal result with exact call binding          | Exact complete task interval                            | Host `attached` only | Observed 120-second boundary |
| OpenCode V1 1.18.35 journal       | Non-synthetic user text        | Final text-end plus completed assistant header     | Full running call in observed assistant lifecycle | Terminal result with exact call binding          | Unavailable                                             | Host `attached` only | Observed 120-second boundary |
| Pi 0.84.4 core V3                 | Accepted user slot and lineage | Characterized persisted `stop` / `toolUse`         | Complete explicit `toolCall`                      | Terminal result with exact call binding          | Unavailable                                             | Host `attached` only | Observed 120-second boundary |
| OpenCode V2 v2.0.32 live capture  | Inbox delivery, not enqueue    | Final text slots plus successful `step.ended`      | Full call with exact step/name join               | Captured terminal result with exact native joins | Verified baseline and later complete execution interval | Host `attached` only | Observed 120-second boundary |

Existing terminal-result support remains under its result contracts. Stored
snapshots, exports, remote/WSL copies, and other source formats do not inherit
these live events. User-slot admission does not prove human origin or approval.
OpenCode capture alerts cover current activity and unfinished checks. A saved
error or expired capture on an inactive session does not add an alert or count as
new unassessed work. This does not restore missing evidence or accepted results.
An unapplied detach publication alone does not establish current alert relevance;
the runtime still consumes it and applies its evidence-validity rules.
Stored OpenCode V2 metrics cannot revoke live-check evidence. Verified capture
updates still invalidate results when their selected evidence changes.
Private reasoning and unfinished streams do not become public assistant
messages. Source-specific synthetic, summary, and UI projections still need the
stated finality proof. A public assistant completion does not itself prove a
complete response cycle.

Cursor Desktop is not another row in this live-event matrix. Its characterized
`CursorIdeComposer` route supplies ordered snapshot `tool_result` evidence with
accepted user-slot prefix context. It does not supply separate user, assistant,
call, cycle, start, or idle triggers. It requires an exact native inline
`cursor-desktop:*` main source, current ready cached evidence, first-record
metadata, recorded header order, and exact bubble/call identities. Snapshot
reconciliation and startup generation/incarnation fences still apply. Cursor CLI,
generic stores, forks, children, remote, and WSL sources do not inherit this route.
See the [source contract](session-coverage.md#cursor-desktop-yaml-tool-evidence)
for synthetic tests, terminal-result fields, and missing-evidence limits. Native
O findings and their remediation remain separate from YAML status checks.

`turn_end` supports `completed`, `cancelled`, and `error` only with all required
public members and exact current member bindings. Corrections and removals can
invalidate a cycle even when its members never matched an authored message
filter. Missing input, finality, joins, or continuity leaves it unavailable while
independent admitted messages can remain usable. Model and mode each require
uniform native evidence across every generating request, including private-only
requests. A user-only cancellation has no generating model. Source timestamps
do not replace native order. See [cycle limits](session-coverage.md#response-cycle-evidence-limits).

Codex exec item results keep their native IDs. An exact same-turn wrapper output
closes the outer call without publishing a second result. Missing or malformed
wrapper output still withholds the cycle. Recognized inert top-level metadata
can invalidate a cycle without making later request history partial; unhandled
evidence fields still leave a history gap.

V2 cycle context retains the known session request prefix separately from
`turn.messages`. A later cycle can keep earlier retained requests without making
them cycle members. Lost or bounded partial prefixes remain partial; a later
valid request does not restore completeness. The accepted source and producer
bounds still apply.

V2 public steps, their calls, and retained assistant messages share frozen
request prefixes instead of charging the same allocation more than once against
the retained-state limit. Emitted contexts remain owned and bounded. Actual state
or output overflow still leaves a capture gap and does not authorize later live
evidence.

Capture continuity is separate from each tool result's request-history
completeness. A complete cycle prefix can coexist with a partial tool member
context. Frozen member contexts retain their exact revision/hash bindings. A
complete local cycle or duplicate terminal does not by itself clear a capture gap.

`session_start` emits only `attached`, once per admitted attachment, including
empty V2 sources. `new` and `resumed` are not reachable source claims.
`session_idle` requires at least 120 seconds of continuous host observation and
a validated current source frontier. Genuine native activity resets the local
deadline regardless of authored filters. Exact replay, historical baseline
facts, usage-only refreshes, selection changes, and UI state do not. Gaps and
disconnects require a fresh observation interval. Host session contexts have no
historical request prefix. New activity cancels unfinished idle work; claim,
local start, provider dispatch, and acceptance recheck attachment and activity
dependencies. Accepted idle evidence remains valid at its saved boundary. Idle
does not prove cycle completion or permanent session termination. See
[host session boundaries](session-coverage.md#host-session-boundaries).

A correction that makes branch membership uncertain invalidates that branch's
unfinished work and accepted evidence atomically, including earlier-page work
without a direct dependency on the corrected identity. Other branches stay
valid. Stale or duplicate updates acknowledge delivery without replacing newer
observations or creating an assessment. None of these transitions writes success.

Quality gaps preserve accepted assessments when their saved input is unchanged.
Continuity resets cancel pending work and clear the affected selection window.
V2 capture gaps invalidate unverified current public-message and cycle bindings
without erasing history. Reconnection or replay cannot restore those bindings;
new identities remain eligible under the normal gates. Tool-result reconciliation
and accepted host idle boundaries remain separate. An unsafe source update revokes that
branch's assessments and pending work and resets its window.

Publisher gaps with unknown membership, main-session scope, or ordering revoke
the affected branch's evidence. Only known content and history limits use
quality-only gaps.

Native file and journal checks follow committed source publication. Their report
readiness uses the committed delivery receipt and current scope, not
legacy metric freshness. **Run checks** under Model-based checks controls YAML check
activation; the provider controls under **Model-based checks** select the active Jev, compatible
Ollama, Cloudflare, or Custom System One connection. Custom supports direct and
Cloudflare-envelope response modes. Settings owns routing, not a YAML decision
`model` field. Observed coding-agent model fields remain evidence.

Fresh setup without a connection stays off. Existing saved master preferences
remain in place; malformed or unreadable storage disables execution. Saving an
inactive connection does not select it. Removing the selected connection turns
checks off and clears selection without deleting accepted results, evidence, or
usage. The empty selection persists across restart. YAML check cards and navigation require a discovered global or
repository definition. Settings keeps its master toggle and decision-model setup
available without definitions. A discovered check with no configured System One
provider shows an actionable setup error in its detail pane; without definitions,
there is no placeholder check or error.

With **Run checks** off, bounded local YAML definition reads, parsing, and
directory watches still supply names, diagnostics, and paused cards, including
at cold startup. This visibility work does not restore provider configuration,
discover model capabilities, catch up session sources, or execute assessments.
It adds no source or result eligibility.

Definition validity and selected-route admission are separate. Named independent
decision blocks support one to eight mixed Choice, Score, and Noul questions each.
Choice and Score need at least two criteria and their own verified type-specific
route intervals. Actions can read optional Choice label maps, Score arrays in
criterion order, and Choice/Score confidence when the route supplies them.
Confidence cannot replace probabilities. Noul is a finite yes-probability, not a
Boolean, with closed empty details and no separate confidence. Manual numeric bounds cannot supply
unknown output support or bypass failed discovery. A compatible connection can
restore scheduling without changing the definition. This adds no session source,
producer version, or native detector support. See
[decision connections](burn-check/authoring.md#decision-connections).

Maintained probability-gated examples, including Fast mode value, cannot run
on a label-only Choice route. A route's selected label does not establish a
probability distribution. Missing probabilities stay unavailable rather than
receiving invented values or confidence substitutions.

The expression runtime supports pure functions, dynamic brackets, nested
projections, embedded text templates, nested map aliases, and dynamic status
results/messages. These operations select from the same frozen evidence; they
add no source fields or completeness proof. Closed-record validation still
applies after dynamic access. Map conditions can exclude unused fields, but
missing selected dependencies remain unavailable through conversion. Composite
identity is separate from source provenance; `fromJSON` creates fresh objects.
Input preparation and action planning share one bounded assessment budget, which
recovery must retain. Budget exhaustion prevents all status writes. See
[evaluation limits](burn-check/authoring.md#shared-evaluation-limits).
All blocks prepare before the first inference call, run sequentially on one
captured connection, and must validate before actions. Shared runtime work,
retained-preparation, and typed-result bounds limit total work without a
syntax-only block limit. Safe retries and restart reuse completed siblings and
exact saved requests; unknown delivery in any block prevents all further
automatic sends and actions for that candidate. Current instruction and skill
inputs use bounded native current-file capture during selection. The host captures
only when an authored condition or candidate needs these inputs, before condition/window
evaluation or candidate fingerprinting as appropriate. It saves the snapshot
with the candidate; native publications, observations, and evidence-version
hashes do not gain mutable file contents. Delayed decisions, safe retries, and
restart reuse saved input without rereading files. A required unavailable value
can prevent assessment, but useful partial scopes remain available. This does
not change native detector finding, prompt, Auto Fix, verification, or estimate support.

Call/result filters support all nine check-facing categories and supported
repository-relative paths. Result and cycle alternatives can filter native
status; start alternatives can filter reason. Model and mode filters use message
generator/requester, cycle-wide uniform attribution, or separately observed
session selection, according to the configured event. User messages have no
generator model or mode. Filters run before conditions and message-window counting
and never fall back to current Settings or another event scope. Missing mode
matches neither true nor false.
Reviewed OpenCode V2 OpenAI Fast/Ultrafast catalog IDs establish selected fast-mode
evidence without requiring endpoint or served-tier proof. They do not prove
actual served speed, route availability, or savings.
Claude's accepted response speed, Codex's matching owned requested-tier preference,
and Pi's recorded assistant model-ID suffix also supply mode evidence under their
source limits. The optional `message.quality.fast_mode_evidence` qualifier keeps
reported speed, selected mode, requested preference, and low-confidence name hints
distinct in saved input. A high judgment probability does not strengthen the
underlying mode evidence.

`session.model` and `session.fast_mode` are separate current-selection fields
where supplied, not past-request attribution. Native publications freeze these
values when first stored; corrections keep that selection, including unknown
values. Old publications do not gain selection from later state. V2 selection
cannot cross a capture epoch or claim fence. Host session contexts use explicit
Codex settings, Pi model changes, or V2 model selections. Claude and V1 selection
remain unavailable in this subset. Current selection cannot repair a missing
message or cycle requester; model and mode availability remain independent.

V2 native `edit` and `write` calls, results, and cycle members support the matching
category filters with explicit local-execution proof. Terminal provider-execution
upgrades correct earlier call facts and exact cycle bindings. A provider-executed
same-name tool does not retain a local edit or write category.

Known included paths can match a partial set; ignore filters require complete
paths to rule out a veto. Any known ignored path wins. Unavailable, invalid, and
over-budget path evidence cannot match. The latest unavailable/incomplete filter
diagnostic persists with branch progress and appears in check detail without
creating a provider attempt or status write. An ordinary nonmatch is not an error.
See [captured native facts](session-coverage.md#captured-requester-and-operation-facts)
for exact source/version limits. New facts do not establish new native detector
finding, prompt, Auto Fix, verification, estimate, or clean-result support.

Omitting `decisions` selects local status actions against the same frozen
candidate; present empty or null decisions remain invalid. Local actions can use
supported expressions but cannot read undeclared decision results, even behind a
false condition. Local
work runs no model discovery, warm-up, provider preparation, transport, or
inference, and creates zero provider attempts and usage records. A cached failed
model discovery or missing output capability alone does not block it. Master
enablement, configured selected connection, credential/authentication, source,
pause, generation, and runtime gates still apply.

Local cooldown defaults to zero rather than the model default of 60 seconds. Its
durable start retains the shared action budget and cooldown through restart,
including action failure. Atomic acceptance, selected-dependency completeness,
no-write evidence/time retention, and current-source validity checks remain the
same. Local evidence is **Frozen local context** from the exact status-writing
candidate: available values, not proof that every field was read or model input.
It has no model connection or missing-model-input warning. These local rules add
no source formats, producer versions, or parser evidence. See
[local status checks](burn-check/authoring.md#local-status-checks).

Native V1-only, V2-only, and mixed discovery retains recent session-header sources before
an optional bounded message/part activity scan. Complete scans also find older
message-only updates. A limited scan keeps the header sources and their activity
times. It can omit old message-only sessions while that limit persists; it does
not guarantee eventual discovery. In mixed databases, a malformed header row in
one family does not hide usable sources from the other. Migrated V1 copies remain
excluded if V2 header decoding fails. A row error without usable sibling sources,
or a schema, query, cancellation, or budget failure, still fails discovery. These
rules do not add parsed evidence or check eligibility. Discovery shares a total
budget of five million SQLite VM instructions and one second. Optional activity
scans leave one million instructions and 200 ms for required work within that budget.

Native OpenCode provider-DB metadata can enter discovery before full strict
hashing finishes. A session-scoped bounded admission revision binds the source
identity to selected metadata and recent record frontiers, not all payloads.
A changed revision clears the prior strict fingerprint, advances the generation,
and invalidates old metric analysis. The pending strict fingerprint persists
across restart and blocks legacy evidence claims until it completes. Native
journal delivery and saved YAML results use their own receipts and evidence
bindings; pending metric hashing alone does not block them.

Strict hashing reads one consistent SQLite snapshot, checks the scoped revision
before and after hashing and before commit, and retries stale or failed work.
Restart recovers interrupted work. A changed admission revision clears prior
retry backoff and becomes eligible immediately. Successful hashes schedule
reconciliation after five minutes for sessions active within the last hour, one
hour for sessions active within the last day, or one day for older sessions,
subject to scheduled scanning and worker availability. Missing databases and
sessions are quarantined until discovery restores their source path; other read
or snapshot failures remain retryable. Reconciliation detects current payload
changes outside bounded admission. Neither admission nor periodic reconciliation
proves original historical retention or extends live journal validation to the
full consumed prefix. See
[metadata admission](session-coverage.md#native-opencode-metadata-admission)
for the bounds. Message/part activity uses the latest valid normalized created
or updated timestamp; the candidate database query retains millisecond units.

Reserved native delivery alternates bounded file and journal turns independently
of legacy metrics. Productive continuation is due immediately on both routes.
Durable monotonic claim order provides fair rotation without a clock tick and
across restart. Active sessions and changed cursors get a one-second poll; idle
attached dormant sources use a fair 30-second resume sweep without discovery
changes. Disabled and ineligible cold sources do not cause idle wake loops.
Live reads retain bounded state and read new suffixes with bounded validation
windows. They do not audit all prior payloads. Normal appends do not cancel
admitted work merely because a discovery fingerprint changes.

Reports distinguish source initialization and pending native delivery from
checks that are ready and waiting for their configured event. Detail shows
catch-up or event progress without replacing the last saved result. A healthy
capture does not show an alert about earlier incomplete history. A current
capture failure still produces one source alert; hiding old gaps does not restore
missing evidence, clear source safety limits, or replay missed work.

Progress follows receipt reset and scheduling eligibility for native OpenCode
journals and native Claude, Codex, and Pi files. Missing receipts and bindings
that require reset wait for data only when current activity admits delivery.
Compatible pending or scheduled delivery can outlive the activity window.
Inactive sources without eligible work do not keep the report catching up.
Current enabled candidates and unconsumed publications can still keep a scope
working. A numeric remaining tool-call count needs a ready selector with known
trigger progress; receipt, attachment, or delivery catch-up does not supply that
count. Before a saved result, **Waiting for activity**, **Catching up**, and
**Checking** describe pipeline progress, not an assessment outcome.

Known `message.removed.1` markers during cold attachment and later live delivery
keep request history partial without making unrelated new result order unsafe.
Future contexts omit deleted request text; earlier frozen contexts stay unchanged.
Deleted message IDs cannot restore request text or establish tool ownership.
Deletion tombstones share the live request cache's 128-record and 64 KiB limits;
they survive ordinary context eviction and restart. If deletion proof alone
exceeds these limits, later ownership stays unavailable.
The desktop publishes exact removals for the deleted message's stored user,
assistant, call, and result bindings, including bindings outside the parser
cache. These removals invalidate affected selection, dependent cycles, and
accepted evidence. Later updates cannot restore a deleted binding. Updates,
removals, activity, selection state, and source receipts commit in native event
order. File and V1 removal expansion commits bounded progress at the unchanged
source cursor until complete. V2 over-budget expansion rolls back the batch and
capture sequence. Unbound removals report coverage gaps; page absence never
removes evidence. Strict historical replay still does not support whole-message
deletion.

The source parser is revision 67, analyzer is 25, evidence schema is 22, file
cursor is 5, resume snapshot is 25, and V1 live journal is 5. Compatible
parser-63, parser-64, parser-65, and parser-66 cursor/state-5 receipts retain committed
progress. Named parser-62/file-V4 and V1 journal-V4 conversions retain
old result progress, result identities, joins, and result seed semantics. They
establish a separate future-only public baseline without replaying old results.
Pre-65 Claude request contexts are an exception: old pending text has no retained
origin proof and becomes partial rather than entering a new assessment. This
conversion does not rewrite accepted inputs, attempts, cooldowns, usage, or status.
Native Claude execution also requires a committed request-origin boundary and
its consumed selector reset. Pre-boundary candidates cannot dispatch or write a
new status. A late answer can retain its response and usage without acceptance;
unknown delivery still cannot trigger a resend. Startup selection cannot seed
old request context merely because its selector cursor is newer.
Transient read errors do not erase proved request origin or revoke otherwise
current frozen work. Existing source-evidence and idle-continuity gates still apply.
Batch resume snapshots require parser 67. Parser numbers alone do not establish
layout compatibility: parser 64 from the producer commit below used file cursor 1
and V1 live state 2, not the accepted cursor/state-5 layouts. Named converters
accept only the exact parser/layout pairs from producer commit
`143d4b05681938cb362e828ecc27d2e082638176`, pinned by synthetic old-encoder
fixtures, not a published release range. They validate source identity and
boundaries before continuing the retained result position and pending joins.
New public event kinds start after the validated current file or journal
frontier; missing historical attribution remains unavailable. Old receipts are
conversion candidates, not ready sources. Readiness requires a committed
layout-5 page and the existing origin, health, and ownership gates. Conversion
does not rewind, reseed, reset paid attempts, or rewrite accepted history and
usage. Malformed and unsupported pairs remain blocked.
An exact proven branch parser-65 or parser-66 cursor-5 receipt keeps its request-origin boundary
and validated context without another reset. Unknown revisions fail closed
without rewind or automatic resend. See
[saved-state conversion](session-coverage.md#saved-state-revisions-and-conversion).

Parser-66 journal cold retries keep their original parser-number hash binding
until the pinned page commits. Upgrade does not replay the window or reset paid
attempts.

For these definitions, `user_message` means accepted content in the user slot,
including extension/API input. It does not require literal-human origin.
Stored V2 user text retains this origin through selection. It can bind an exact
remediation marker without supplying human-text or scope-approval proof.
OpenCode synthetic parts are excluded from snapshot and journal request history.
Real text in mixed messages remains usable. A generated-only message or replacement
without real request text leaves history partial. Pi user-slot messages are
supported under the same prefix/order rules.
Claude meta, synthetic, plan, and explicit non-user-origin text is also excluded.
The pinned CLI envelope admits optional human, API, extension, and SDK user-slot
markers; unknown or malformed markers remain unavailable. Safe text in mixed
records remains partial. API, extension, and SDK markers do not prove human approval.
The pinned Claude `Read` result can be completed without `is_error` only with
one exact matched call/result and matching structured text plus numbered output.
Missing or conflicting proof leaves completion unknown.
Tool-result carriers and assistant/tool text are separate facts. Available
fields remain assessable when optional context is missing or source history is
partial. `message.quality`, `session.requests_complete`, and
`session.source_coverage` tell the model which limits apply. Unknown identity,
scope, or ordering is still not promoted as reliable evidence.

For the pinned V2 snapshot reader, missing shell output and migration-cleared
results keep a result-level content limit through selected publication. Native
completion and program exit facts do not make missing output complete. A file
read with a different returned filename does not prove a range in the requested
file. These are shared source facts, not additional check capabilities.

`ToolResultAdmission` gates source identity, native order, and main-session
ownership separately from content completeness. Missing frozen context and unsafe
bindings produce source gaps without provider calls for those records. The local
legacy sink's 4,096-observation/4 MiB recent result window can omit older work;
native live delivery continues from bounded file/journal cursors. Neither
retained state nor one page proves whole-session absence. Unavailable tool categories
remain absent and cannot match an `other` filter. A recorded tool name can normalize
to native `Other` when no more specific class applies. All nine check-facing
categories compile, but each source needs its own reviewed native identity.
The metric category `Other` cannot supply a more specific check-facing class.

For native OpenCode, current rows, immutable outbox events, the cursor, baseline,
diagnostics, and continuation state commit atomically. Page membership does not
imply full source membership: absent identities never create removals. Real
history gaps persist across pages and restart; page limits alone do not create
gaps. Cold journal request history is explicitly partial. New gap
events precede usable observations. Live validation accepts later appends, but
detected identity/boundary changes and invalid cursors require explicit source
recovery without automatic rewind. It does not detect every interior old-payload
rewrite. Strict journal replay retains full-prefix validation. Lease, claim,
incarnation, exact project, parser, and fork-admission checks still apply at publication.
An exact journal removal without a retained call binding adds a coverage gap,
not a blanket ban on later independent results. A message-role change invalidates
retained ownership even if Store can restore older frozen request context.
Selected cold-window identities retain their first-terminal binding under cache
pressure within the existing identity budget. Missing frozen context stays
unavailable; corrections cannot replace it with later request text. A terminal-only
correction for an identity evicted during warm delivery remains unsafe.
An interrupted cold journal page stores a bounded retry cursor and output
digest. Retry and restart validate the original window before publishing its
remaining results. Frozen ownership lookups do not change when projection rows
change. Later messages keep their future-live eligibility. A changed
window requires source recovery rather than a new cold baseline.

Missing legacy model/provider/usage/timing metadata does not by itself block
journal delivery or YAML results. Missing fields, non-text user parts,
compaction, or bounded request-history loss can remain usable partial input;
they cannot repair unsafe identity or order. A sequence gap or unknown event
envelope makes affected order unsafe. No-progress gaps produce explicit errors.
See [journal delivery and metadata limits](session-coverage.md#opencode-journal-delivery)
for the bounded payloads, sticky coverage limits, and recovery contract. These
fixture-backed behaviors are not live certification or a measured latency guarantee.

Accepted user messages and explicit calls can trigger their own message
alternatives as well as supply frozen result/cycle context. The desktop
shows a YAML check as Not assessed until an accepted status action writes success
or failure. Cards stay in their last saved result group while queued or running
work adds Checking. A known event countdown can appear in detail after catch-up.
These progress states do not write engine status or add a clean session.
The maintained examples select tool-result windows, not assistant-message
context from the session. Provider framing and explicit omission metadata are separate
from the selected session evidence.

Most maintained YAML examples explicitly pass every valid answer that does not meet
their failure condition. Ignored instructions instead requires `no_conflict`
with label probability at least 0.95 to write success. Uncertain and
below-threshold answers write no new result, so an earlier valid finding can
remain visible. Fast mode value requires `try_standard_mode` with a label
probability of at least 0.95 to fail; uncertainty and a lower label probability
write success. Such success
does not prove repair or whole-session correctness. Missing evidence, failed
execution, and invalid answers do not receive automatic success. A status write
without `input.message` clears the previous message for that branch.

Missing records, truncated fields, or unfinished work can limit selected evidence.
Terminal state does not prove complete source history or content. A required
field read that is unavailable can make that candidate not assessable; later
results in the same session remain eligible. No definition can prove that an
unrecorded external change did not occur.

Over-exploring selects completed `read` and `search` results, not every shell
command. The pinned Codex single-file numbered shell slice supplies category
`read` only when its command, path, successful status, and complete stdout extent
match the reviewed contract. Other shell-based investigation does not enter that
window. Codex MCP syntax can supply category `mcp` for authored tool filters, but
does not prove a trusted server or expand native remediation support.

Fast mode value selects the latest five matching fast-mode results, with a
minimum of three. It suggests a standard-mode trial only when the recorded task
affirmatively permits slower work, such as an explicit non-urgent request.
Missing urgency requirements alone do not support advice. Its `try_standard_mode`
label needs probability at least 0.95 to fail; every other valid answer writes
success. That success means only that the selected evidence does not meet this
advice rule. The check makes no current-setting, served-tier, duration, or savings
claim. Recorded OpenCode catalog choices can supply selected-mode evidence
without proving the actual tier. See the source-specific limits above.

Ignored instructions separates a conflict with a recorded correction that
preceded the assessed work from a mismatch with currently observed files.
Current-file findings do not claim that a newly observed rule applied in the
past. Conditional scope and exceptions must remain supported by the selected
evidence. A request prefix through the trigger does not prove that every request
preceded every earlier window result. Unavailable instruction scopes do not
prevent an independent recorded-correction comparison.

It retains a 20-result window and all available request history. Success covers
only the supported comparison in the retained selected work, not omitted
evidence, past rule activation, or whole-session correctness.

Skill opportunities suggests reviewing the supplied configured descriptions for
useful next-step guidance. Its result stays at inventory level and points to the
saved assessment input. It does not name an unbound specific skill, claim that a
session loaded the skill, or claim historical non-use. Duplicate names retain
their distinct paths; missing descriptions remain empty. Skill inventory inputs
do not contain full bodies, though selected tool results can contain file text.

The two inventory definitions select whole current `global` and `repo` scopes
to retain useful entries when an array is absent. The current expression language
cannot build a selective object that safely omits that unavailable array. As a
result, the other inventory in each scope can also enter the selected input; the
questions ignore it. Ignored instructions permits only unused skill descriptions
to shrink, through `current_global_scope.skills[*].description` and
`current_repo_scope.skills[*].description`. Instruction text and limits, skill
names and paths, task text, approvals, request history, and selected arguments
remain intact. Skill opportunities preserves its inventory fields. Both permit
selected result-content truncation. Large protected input can still reject
preparation rather than silently shrink required evidence. See
[current-file source limits](session-coverage.md#yaml-current-file-inputs).

Current-file inputs require admitted paths. On Windows, ordinary path components
that end in a space or period remain unavailable. Drive and UNC paths can retain
literal verbatim names but do not bypass root or symlink checks. Other Windows
namespaces remain unavailable. Safe entries can remain available beside rejected
configured sources.

### Decision-input fitting limits

Decision-level `fit` authorizes loss from selected evaluated input paths only.
Missing rules alone do not produce a warning. Input-size explanations follow
actual preparation or provider failures, with no automatic loss or summary calls.
`recent` removes one oldest array item per turn and retains at least one;
`truncate` removes up to 1 KiB of original middle string text per turn and retains
at least 256 original UTF-8 bytes in a Unicode-safe head and tail with one marker.
Unicode alignment can retain more than 256 bytes. A first truncation is skipped
unless the omitted substring is larger than the marker after JSON escaping.
Rules expand once in YAML and original array order. Concrete targets take strict
round-robin turns, with complete provider-request validation after each turn and
an immediate stop on fit. Unselected values, questions, criteria, frozen original
input, and source evidence remain unchanged. Missing optional fields and empty
arrays are skipped; incompatible present values, unknown input roots, unsupported
paths, and overlap are errors. See the
[authoring limits](burn-check/authoring.md#fit-large-inputs) and the
[deployed preparation bounds](burn-check/authoring.md#preparation-limits).

The maintained Fast mode value definition permits only
`activity[*].tool.result.content: truncate`. Result records, linked arguments,
status, and request/authorization text remain intact. Source-reader bounds can
already omit evidence before fitting; fitting cannot recover that evidence or
make incomplete history complete. Large request text or argument values outside
the fit targets can still prevent assessment.

The provider receives an explicit omission report with concrete paths, strategies,
and original/retained item or text-byte counts. Successful shortened assessments
show an informational detail note. A finding or explicit success covers only the
retained evidence; omitted text or records cannot prove absence, recovery,
authorization, or whole-session cleanliness. Typed no-fit distinguishes no rules,
no matching targets, retention/marker limits, and exhausted reductions under
local request validation. `PreparationResourceLimit` separately covers original
input admission, expansion/traversal, attempted-turn, and cumulative validation
work limits. It does not prove that further authorized fitting could not succeed.
Old no-fit records without a typed reason remain inconclusive. The 8 MiB
per-block original-input bound and shared 64 MiB fitting-work bound are preparation
limits, not source parser limits. A separate shared 64 MiB saved-validation pass
cannot refit requests. The full ready record has an 8 MiB, 65,536-value,
depth-64 bound; typed results have a separate 512 KiB bound, checked for
worst-case capacity before calls. See the authoring guide for complete
aggregate limits; these do not establish additional source evidence.

Initial model-input preparation failures settle as not assessed before dispatch: zero provider
calls, no new model cooldown, no actions, and no status write. Incompatible
recovery profiles cause zero additional attempts and preserve prior attempt
counts. These outcomes do not immediately retry unchanged work. Later
candidates remain eligible, and permitted provider retries reuse identical
prepared input and omission metadata.

Model finding evidence uses each decision's original evaluated input, exact
fitted input, and omission report from the status-writing assessment. A later assessment does not substitute its input, and
original full tool output is not displayed as shortened assessed input. Legacy
records without preparation are explicitly labeled original source with the
limit that exact assessed input was not saved. Current definition, activation,
scope/ownership, source-validity, and report-cohort gates still apply.
Saved **Decision connection** evidence identifies that assessment's provider and
requested model, not the current Settings connection or the coding-agent model. It omits
endpoints and credentials. Missing legacy provenance stays unavailable.
Choice and Score require at least two criteria. Selected-route admission checks
each type's verified interval and required details, including indirect and
whole-object reads, separately from language validation. Jev's local Choice guard
permits at most 26 criteria; Score has its own limit. Neither is a language-wide
or universal upstream maximum. See the [pinned provider contracts](burn-check/provider-contracts.md).
Deterministic request-shape failures
remain separate from fitting and connection failures.

Native Jev requests use `jev-latest` and the native long-context header. Both
local context ceilings are 262,000 estimated tokens before the 4,096-token
reserve; serialized requests also have a 384 KiB body guard and an 832 KiB
resident request-charge guard. Automatic route limits and shared preparation
limits still apply; new work does not use saved manual overrides. This does not
establish tokenizer-level capacity or make every
candidate assessable. Custom, Cloudflare, and Ollama do not inherit these bounds.
Observed served models belong to individual attempts, not saved credentials or
the immutable request alias. Unknown served models retain confirmed usage but
remain unpriced. Incompatible old preparation stops without another send, and
unknown delivery remains terminal. No source or clean-result support follows
from a larger provider context.

Definition and execution diagnostics appear in the normal check detail with the
available file location and a default-application **Open check file** action.
Repeated errors are grouped without hiding distinct files/decisions or prior
valid findings. A later successful assessment or definition repair clears stale
errors. Available details contain safe typed codes, decision/path context, and
local measurements or estimates; they do not expose raw provider errors.
Diagnostics and fitting add no source formats, producer versions, native
remediation, verification, or savings capability.

### Current status and native detector boundaries

The native 30-day report cohort groups eligible current status by logical
filename-stem ID after resolving project overrides. No-write preserves the
original status explanation, time, and evidence. When no action of a definition
matches, the accepted assessment advances its watermark without a status write.
A branch with no prior write remains unassessed; there is no engine fallback
success. Fast mode value's complement action writes success for uncertainty
and below-0.95 answers. Ignored instructions writes success only for
`no_conflict` at probability 0.95 or above; its uncertain answers preserve the
previous status. One branch's success cannot clear another's
valid failure. Explicit success covers assessed work only, not whole-session
cleanliness, repair, or savings. YAML definitions provide status/explanations,
without native fix prompts, Auto Fix, verification watches, burn estimates, or
savings. Snooze changes presentation only. Isolated developer replay does not
contribute live status or extend source coverage.

Current native environment and CWD must match the activation scope. A moved or
unknown-CWD session cannot contribute old results, diagnostics, evidence, or
checking counts through its earlier project scope. Trigger progress uses the
current ready report cohort. Native journal readiness and waiting follow the
current delivery state, not legacy projection failure. Blocked or exhausted
delivery yields a source-unavailable diagnostic rather than indefinite waiting.

The following matrices describe the nine deterministic native detectors. Their
finding, prompt, Auto Fix, verification, and estimate requirements remain
separate from definition semantics. User-slot admission does not prove human
approval. Shared parser and private storage capabilities do not add YAML
definitions to the native detector inventory or expand the YAML runtime subset.

## Status Rules

Native skill facts are fixture-characterized for `ClaudeJsonl`,
`OpenCodeSqliteV1`, `CodexRolloutJsonl`, and `PiV3Jsonl` under the
[accepted shapes and limits](../crates/antiburn-local/tests/fixtures/skill_use_characterization/README.md).
Exact references preserve selected event identity, lifecycle, source roles,
timestamps/order when present, and session/publication fences. Adapters persist
generic validated skill facts before check preparation; supplemental records
cannot repair invalid selected proof. Claude 2.1.278 and the characterized
2.1.309 CLI variant retain failed Skill result state and exact ranges, not
successful delivery. Older conditional launch decoding remains
separate. OpenCode load
results retain bounded parsed proof only with explicit output selection. Its typed load
result evidence is characterized only for the pinned SQLite v2 producer
`anomalyco/opencode@772392050500e0ddcd2ad2193411a22a3824372f`, and only for a
selected, complete, untruncated, uninterrupted, uncompacted native result bound
to its session, call, message, and part. This does not establish session-wide
absence, historical visibility, or a model-based finding path.
OpenCode JSONL is unavailable. Other accepted agents use separate native
selection/request/result contracts. A semantic evaluation pass does not establish
product support. Codex full documents are
selection evidence, not task success. Pi explicit requests do not imply
successful extension execution. Aliases and current-file identity stay inferred.
Aggregates cannot prove absence. Even complete selected records cannot prove
session-wide or equivalent-use absence.

Optional `UserAnswer` and `PlanReference` fields have shared engine types and
private storage/query contracts. They require explicit selection. Both are
conditional for `OpenCodeSqliteV1`, `ClaudeJsonl`, `CodexRolloutJsonl`, and
`PiV3Jsonl` under the pinned contracts in `session-coverage.md`; other sources,
including Cursor and Antigravity, remain unavailable for these typed forms. OpenCode plan content
remains unresolved. Claude retains exact recorded plan text/content digests but
does not infer approved versions. Claude structured string answers retain
submitted status only for intact unambiguous results, with unknown human origin.
SDK `updatedInput`, permission changes, auto-approved tools, and `ExitPlanMode`
completion do not establish scope approval. Rejection can mean interruption;
later ordinary user free text stays separate from question results. Missing,
partial, synthetic, conflicting, or mutable evidence cannot supply authority.
Codex answer origin remains unknown; retained acceptance order is not human
authorization. Its plan records remain proposals without approved-version joins.
Pi question origin and producer identity remain unknown. Its exact plan-mode
execution choice has synthetic origin. Typed headers, context, comments, and
skipped batch status preserve meaning without widening approval eligibility.
Ordinary tool names/results cannot supply
authoritative answers. Missing origin and approved-version linkage remain
unavailable. These private fields are not additional YAML context inputs.

| Status      | Meaning                                                                                                                                           |
| ----------- | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| Assessable  | The current reader can produce the evidence needed for a finding and a clean result. A damaged or incomplete session can still be partial.        |
| Partial     | The source has useful evidence, but its supported claims are limited. It cannot support a clean result.                                           |
| Unsupported | The reviewed passive sources do not prove a required fact or its policy semantics. This is source-scoped, not a claim about future formats.       |
| Unknown     | The source or its relevant field semantics are not characterized. Do not infer support from a path, field name, mode name, or generic JSON shape. |

For metric checks, `Partial` cannot support clean. `Assessable` describes an
accepted source contract, not every session or past release. Clean results need
complete required facts and eligible activity. No status promises exhaustive
history or a finding without its source note. An unimplemented or
uncharacterized path remains `Unknown` or `Unsupported`.

Native checks use their individual enabled preferences. A disabled check does
not contribute active findings, clean results, counts, remediation actions,
verification watches, or savings. Existing remediation history and incurred
usage remain stored. With no checks enabled, the desktop reports that state
rather than claiming Clean or Passed.

## Optional file-read evidence

`ReadFileRequest` and `ReadFileResult` require explicit Jev field selection.
`ReadFileOutput` separately selects recorded result text. These fields are
shared private evidence APIs, not native model-based checks or new YAML fields.

Requested line units are characterized for exact Claude `Read`, Pi `read`, and
OpenCode V1 SQLite `read` calls under the schema and producer pins in
[`session-coverage.md`](session-coverage.md#private-file-read-evidence). Other
accepted read aliases retain native range arguments with unknown units.
Returned extents cover pinned OpenCode numbered output, Claude 2.1.278 matched
payload/numbered text, Pi 0.84.4 text/clipping details, and the Codex 0.160.1
numbered single-file shell slice. Session coverage defines exact joins, status,
and renderer limits. Cursor synthesized stores cannot supply typed tool-read
evidence through their generic selected-content projection. The separate Cursor
Desktop YAML result route does not add typed file-read extent. Other native tool
sources have no characterized returned extent.
Excluded source formats do not gain selected-content support from these types.

Missing ranges, unmatched calls, failed operations, clipped lines, directory
listings, image/PDF notices, previews, outlines, and searches do not prove that
the requested file interval was read. Shell-mediated reads other than the pinned
Codex slice are unavailable, including simple `cat`. Output bytes/digests identify recorded text,
not a file size or version. Current adapters do not supply recorded file versions.
Read counts or repeated/overlapping slices alone are not evidence of waste.

Parser revision 58 rebuilds private native facts from source; evidence schema
revision 22 is unchanged. Unselected read ranges and results do not change a
selected-content digest when selected input and the publication fence stay
unchanged. Parser-bound input revisions and publication
fences still invalidate stale work through the normal revision contract.

## Checks

| Code | Check                 |
| ---- | --------------------- |
| D    | Session overdepth     |
| T    | Model overthinking    |
| S    | Overpowered subagents |
| M    | Unused MCP servers    |
| B    | Unused built-in tools |
| K    | Unused skills         |
| O    | Old model usage       |
| F    | Fast mode overuse     |
| C    | Cache churn           |

## Source Inventory

The tables list all 35 `SourceFormat` keys. Known source shape and release
version are separate facts. A version range is not always available; an accepted
schema, header, or pinned producer commit with synthetic fixtures can establish
a bounded contract. No row promises parity across all historical versions.

The [reachable event table](#reachable-event-families) states the additional
native live finality and first-eligibility rules. A row's stored parser support
does not grant these events to exports, remote/WSL copies, or other formats.

| `SourceFormat`                 | Passive source format                                                                                                          | Version statement                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            | Current reader                                                                                                                                                                                                                                                                                  |
| ------------------------------ | ------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ClaudeJsonl`                  | Claude Code session JSONL and child sidecars, including Claude Desktop Cowork transcripts from nested `.claude/projects` roots | Private producer; fixture-bounded core/sidecars and optional decoder pins, without a public schema pin. CLI 2.1.278 and 2.1.309 have characterized retained roots and native results, including their exact linear bridge, informational prelude, and explicit human markers. This does not admit SDK/GC history or other releases. YAML context separately accepts user-slot text outside tool envelopes with exact direct-parent call joins, UUID/block/physical positions, error state, and completeness; human proofs remain stricter. Read returned extents stay pinned to 2.1.278. Cowork main JSONL has a separate synthetic metric shape observed with Desktop 2.2553.1 and embedded Code 2.1.275; `audit*.jsonl` is excluded. See session coverage. | Dedicated                                                                                                                                                                                                                                                                                       |
| `CodexRolloutJsonl`            | Codex rollout JSONL with discovered children                                                                                   | Legacy recorder `e7637306bc9246a3e42e407cb94f96b7ed345e3e`; retained native roots/completed items and synthetic YAML result/context shapes use 0.160.1 at `d27764b82f7118f674371e6d6e76271d9d606edb`. YAML observations require recognized linked response-item outputs or accepted completed command/file-change items, native line order, available status, and complete retained root `user.text`; UI echoes are not extra requests. Qualified environment context is single-local and non-authorizing. No historical release range.                                                                                                                                                                                                                      | Dedicated                                                                                                                                                                                                                                                                                       |
| `OpenCodeJsonl`                | OpenCode legacy exported session data                                                                                          | Accepted export wrappers and native message/part shapes; pinned research below                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               | Dedicated                                                                                                                                                                                                                                                                                       |
| `OpenCodeSqliteV1`             | OpenCode `session`/`message`/`part` snapshots; native YAML uses the 1.18.35 `event`/`event_sequence` journal                   | Fixture-backed exact-ID snapshot. Native message/skill producer `772392050500e0ddcd2ad2193411a22a3824372f`; Read renderer `652c090dc119b5f3dc1e5e0bf1c4b40d9721f0ef`. YAML pins 1.18.35 at `53d1eabb61e21162157817bf677da0a4ad3332e3`. Live delivery uses bounded recent input, indexed suffixes, and atomic cursor/outbox state. Corrections retain first-terminal order and frozen requests. Synthetic text is excluded; cold requests are partial. Page absence is not removal. Optional content can remain usable; unsafe identity/order/ownership cannot. Live validation samples boundaries; strict replay audits the prefix. See [journal limits](session-coverage.md#opencode-journal-delivery). No live certification or historical-release claim.  | Dedicated                                                                                                                                                                                                                                                                                       |
| `PiV3Jsonl`                    | Pi session JSONL                                                                                                               | Header V1/V2/V3 metric migrations; retained native core V3 roots/results and synthetic YAML result/context shapes use 0.84.4 at `b79e4cc834970cca69daebffab7df1da7d1e52c4`. YAML observations retain physical order, exact call/tool/parent joins, status, and completeness; main-session user-slot content needs no sender-origin proof. Branch/partial-record gaps block complete context. V1/V2 metric migration does not establish YAML support. Optional extension question/plan pins are separate. Headerless/unsupported sources reject; no historical release range.                                                                                                                                                                                 | Dedicated                                                                                                                                                                                                                                                                                       |
| `OmpV3Jsonl`                   | Oh My Pi session JSONL behind the title slot                                                                                   | Fixed-width 256-byte `type: "title"` slot, then an exact version 3 header; synthetic fixtures pin the accepted shape, an allowlist admits only the OMP core rows, characterized `custom` rows, and signal-free `title_change`/`credential_pin`/`ttsr_injection`/`session_init` housekeeping rows, and every other record type or header version fails closed                                                                                                                                                                                                                                                                                                                                                                                                 | Shared Pi-family reader                                                                                                                                                                                                                                                                         |
| `CursorJsonl`                  | Cursor compatibility JSONL without a surface marker                                                                            | Unversioned and uncharacterized                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              | Dedicated shared Cursor reader                                                                                                                                                                                                                                                                  |
| `CursorCliAgentJsonl`          | Cursor agent transcript JSONL                                                                                                  | Separate partial export contract with content blocks and explicit subagent-path parent observations; no model fallback                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       | Dedicated shared Cursor reader                                                                                                                                                                                                                                                                  |
| `CursorCliStoreDb`             | Legacy Cursor CLI `chats/**/store.db` data                                                                                     | Private `blobs`/`meta` subset pinned by public reverse engineering; partial                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  | Dedicated shared Cursor reader                                                                                                                                                                                                                                                                  |
| `CursorChatStoreDb`            | Cursor chat `~/.cursor/chats/<workspace>/<session>/store.db` data                                                              | Chat path and private `blobs`/`meta` subset pinned by public reverse engineering; explicit `subagentInfo.parentAgentId` is partial                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           | Dedicated shared Cursor reader                                                                                                                                                                                                                                                                  |
| `CursorIdeComposer`            | Cursor IDE composer data from `state.vscdb`                                                                                    | Private and unversioned. Synthetic discovery, parser, and desktop publication tests characterize exact recorded header order, first-record metadata, bubble/call identities, native main ownership, and `run_terminal_command_v2` results. The separate [YAML snapshot route](session-coverage.md#cursor-desktop-yaml-tool-evidence) adds no Cursor CLI/store, public-event family, requester/path, typed file-read, or release-range claim.                                                                                                                                                                                                                                                                                                                 | Dedicated shared Cursor reader; partial native O findings remain separate from bounded YAML tool-result assessment                                                                                                                                                                              |
| `CursorLegacyChatJson`         | Cursor IDE `chatSessions/*.json`                                                                                               | Unversioned and uncharacterized                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              | Dedicated fail-closed profile                                                                                                                                                                                                                                                                   |
| `AntigravityJson`              | Internal Antigravity compatibility profile                                                                                     | Not emitted by current source classification                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 | Dedicated shared profile                                                                                                                                                                                                                                                                        |
| `AntigravityBrainJsonl`        | Antigravity brain transcript JSONL                                                                                             | Unversioned; model-setting changes, thinking, and truncated-field markers are characterized partially                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        | Dedicated                                                                                                                                                                                                                                                                                       |
| `AntigravityCascadeJson`       | Antigravity API cascade or mirror JSON                                                                                         | Unversioned; thinking and nested tool arguments are characterized partially                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  | Dedicated                                                                                                                                                                                                                                                                                       |
| `AntigravityWorkspaceChatJson` | Antigravity workspace `chatSessions/*.json`                                                                                    | Unversioned and uncharacterized                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              | Dedicated fail-closed profile                                                                                                                                                                                                                                                                   |
| `AntigravitySqlite`            | Native `conversations/<uuid>.db` plus an optional brain transcript                                                             | agy 1.0.16 reverse-engineered subset; requires `user_version = 1` and reviewed `gen_metadata(idx,data)` or `steps(idx,metadata)` columns; exact bounded response identities; conflicting model joins are partial; not full schema support                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    | Dedicated                                                                                                                                                                                                                                                                                       |
| `CopilotCliJsonl`              | `session-state/<uuid>/events.jsonl` plus sibling `session-store.db`                                                            | Public Copilot SDK v1 envelope plus schema-v7 read-only request store; target-session request rows reconcile the persisted shutdown totals; unknown lanes remain partial                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     | Dedicated v1 bundle reader                                                                                                                                                                                                                                                                      |
| `CopilotIdeChatJson`           | VS Code-family `chatSessions/*.json`                                                                                           | Unversioned; IDE and CLI contracts are separate                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              | Dedicated fail-closed                                                                                                                                                                                                                                                                           |
| `ClineSessionJson`             | Cline metadata and message companion                                                                                           | Cline 2.0+ naming is known; message schemas are not pinned                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   | Dedicated fail-closed                                                                                                                                                                                                                                                                           |
| `ClineMessagesContractV1`      | Cline `.cline/data/db/sessions.db`, root manifest, and messages artifacts                                                      | Cline messages-contract v1; required `sessions` columns include `agent_id`; root and child artifacts use exact paths, parent IDs, origins, and model matches                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 | Dedicated v1 bundle reader                                                                                                                                                                                                                                                                      |
| `KiroSessionJson`              | Kiro workspace-session JSON                                                                                                    | Unversioned and uncharacterized                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              | Dedicated fail-closed                                                                                                                                                                                                                                                                           |
| `KiroChat`                     | Kiro `.chat` fallback                                                                                                          | Unversioned and uncharacterized                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              | Dedicated fail-closed                                                                                                                                                                                                                                                                           |
| `KiroCliV2Bundle`              | Kiro CLI V2 `.json` metadata and matching `.jsonl` journal                                                                     | Fixture-backed observed V1 metadata and V1 envelope contract; UUID siblings only; D/S unavailable and C unsupported                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          | Dedicated V2 bundle reader                                                                                                                                                                                                                                                                      |
| `KiroCliV3Bundle`              | Kiro CLI V3 `session.json` and `messages.jsonl` directory                                                                      | Separate path is known, but no pinned `session.json` producer shape; fail-closed                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             | Dedicated fail-closed                                                                                                                                                                                                                                                                           |
| `KiroChatSaveExport`           | Manual Kiro CLI `/chat save` JSON                                                                                              | Public command is known; export schema is not published                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      | Not scanned; unsupported                                                                                                                                                                                                                                                                        |
| `AmpThreadJson`                | Amp `threads/*.json` whole-thread record                                                                                       | Explicit full-export envelope version 39; ordered assistant usage with `totalInputTokens`, `maxInputTokens`, model, timestamp, tools, and activated skills; findings only                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    | Dedicated v39 export reader                                                                                                                                                                                                                                                                     |
| `AmpFileChanges`               | Amp `file-changes/**/*.{json,jsonl}`                                                                                           | File-change fallback, not a thread                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           | Dedicated fail-closed                                                                                                                                                                                                                                                                           |
| `WindsurfWorkspaceJson`        | Windsurf workspace chat JSON                                                                                                   | Unversioned and uncharacterized                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              | Dedicated fail-closed                                                                                                                                                                                                                                                                           |
| `WindsurfMirrorJson`           | Configured Windsurf mirror JSON                                                                                                | Unversioned and uncharacterized                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              | Dedicated fail-closed                                                                                                                                                                                                                                                                           |
| `WindsurfCascadeProtobuf`      | Windsurf Cascade `.pb` data                                                                                                    | Private and uncharacterized                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  | Dedicated fail-closed when discovered                                                                                                                                                                                                                                                           |
| `DevinLocalSqlite`             | Devin Local migration-17 `sessions.db`                                                                                         | Migration 17 and required table columns are fixture-pinned; WAL-visible read-only snapshots; ACP schema 6 is optional child companion only                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   | Dedicated S-only reader                                                                                                                                                                                                                                                                         |
| `Uncharacterized`              | Unknown-agent generic fallback                                                                                                 | No source contract                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           | Generic fail-closed                                                                                                                                                                                                                                                                             |
| `OpenCodeSqliteV2`             | OpenCode Core V2 `session_v2` / `session_message` SQLite                                                                       | OpenCode v2.0.32 producer commit `843a6c514e606b436a7fe713422ace0a93ad3d61`; bounded read-only parser for the pinned shape. Caps at 100,000 rows, 128 MiB aggregate row data, and 8 MiB per row; partial on cap. Live direct-result delivery uses the registered local event feed, one verified database, and the exact native main-session Live CWD scope. It admits at most 256 active sessions and fails closed above that cap. It has no replay; request history before attach stays partial, and reconnect publishes a gap at a new sequence boundary.                                                                                                                                                                                                  | D/S/O/C finding support is partial and has bounded estimates. M/B/K resource findings, T/F, and clean results are unsupported. Model and worker edits need an exact current-consumer target. O verification stays unavailable because the saved evidence does not prove the provider API route. |
| `OpenCodeV2SessionExport`      | V2 `SessionTransfer.Data` JSON from WSL                                                                                        | Pinned OpenCode v2.0.32 transfer DTO from an already-running registered service; existing Linux curl, bounded streaming reads without redirects or proxies, exact session IDs, and stdin-only authentication. Registration honors an absolute WSL command-environment `XDG_STATE_HOME`, otherwise the default state root. Selects at most 100 exports from 500 metadata rows. At most four exports share a 32 MiB session-list/export streaming budget. Each response keeps its 16 MiB limit, independent of other pending responses. Completed exports survive later export failures when final service identity checks pass.                                                                                                                               | Dedicated partial stored parser. No detector-grade facts, native live eligibility, remediation, or clean-result support. State-root lookup does not read the service's process environment. Offline reads and real WSL runtime validation remain outside this characterized support.            |

OpenCode 2 releases enter the V2 reader by default, without an exact-release
allowlist. The producer pins above define characterized shapes, not version
gates. Required schema, protocol fields, size limits, and identity checks still
apply to native stored reads, live capture, current config reads, and WSL exports.
This policy does not add clean results or remediation capabilities.

V2 live YAML execution is separate from the native check matrix below. The
registered OpenCode 2 service must be bound to one native database before
capture starts. Fresh attachments do not seed historical terminal results or
new message kinds. Captured evidence retains native event order and partial
request history. Cycles require the verified baseline and a later observed
execution start; host attachment and idle use the separate observer contract.
Retained-history limits do not prove a transport outage or full membership.
Public-only batches cannot restore omitted historical coverage or clear an
existing result window.
Continuity loss resets the affected branch window without clearing other
sessions or retrying uncertain provider attempts. Quality gaps and continuity
resets preserve accepted status and its saved input when that evidence is
unchanged. A reset cancels pending work from the affected selection segment;
corrections and removals still invalidate affected assessments.
Empty attachment receipts let already captured work drain after disconnect.
Stored snapshots do not prove complete terminal history, historical tool exposure,
or current config targets.
V2 static D/S/O/C findings have partial support and bounded estimates. Their
prompts are reachable, but they cannot produce clean results. Model and worker
edits need an exact current-consumer target. O verification stays unavailable
because saved evidence does not prove the provider API route. M/B/K resource
findings remain unsupported.

## Coverage Matrix

This matrix records implemented eligibility and source limits for the nine
deterministic native checks. YAML definitions use the separate runtime and
source contracts above.

| `SourceFormat`                 | D           | T           | S           | M           | B           | K           | O           | F           | C           |
| ------------------------------ | ----------- | ----------- | ----------- | ----------- | ----------- | ----------- | ----------- | ----------- | ----------- |
| `ClaudeJsonl`                  | Assessable  | Assessable  | Assessable  | Partial     | Partial     | Partial     | Assessable  | Assessable  | Assessable  |
| `CodexRolloutJsonl`            | Assessable  | Assessable  | Assessable  | Partial     | Partial     | Partial     | Assessable  | Assessable  | Assessable  |
| `OpenCodeJsonl`                | Assessable  | Unsupported | Assessable  | Unsupported | Unsupported | Partial     | Assessable  | Unsupported | Assessable  |
| `OpenCodeSqliteV1`             | Assessable  | Unsupported | Assessable  | Unsupported | Unsupported | Partial     | Assessable  | Unsupported | Assessable  |
| `PiV3Jsonl`                    | Assessable  | Assessable  | Partial     | Unsupported | Unsupported | Unsupported | Assessable  | Unsupported | Assessable  |
| `OmpV3Jsonl`                   | Partial     | Partial     | Unsupported | Unsupported | Unsupported | Unsupported | Partial     | Unsupported | Unsupported |
| `CursorJsonl`                  | Unsupported | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Partial     | Unknown     | Unknown     |
| `CursorCliAgentJsonl`          | Unsupported | Unknown     | Unsupported | Unsupported | Unsupported | Unknown     | Partial     | Unknown     | Unsupported |
| `CursorCliStoreDb`             | Unsupported | Unknown     | Unsupported | Unsupported | Unsupported | Unknown     | Partial     | Unknown     | Unsupported |
| `CursorChatStoreDb`            | Unsupported | Unknown     | Unsupported | Unsupported | Unsupported | Unknown     | Partial     | Unknown     | Unsupported |
| `CursorIdeComposer`            | Unsupported | Unknown     | Unsupported | Unsupported | Unsupported | Unknown     | Partial     | Unknown     | Unsupported |
| `CursorLegacyChatJson`         | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     |
| `AntigravityJson`              | Partial     | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Partial     | Unsupported | Unsupported |
| `AntigravityBrainJsonl`        | Partial     | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Partial     | Unsupported | Unsupported |
| `AntigravityCascadeJson`       | Partial     | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Partial     | Unsupported | Unsupported |
| `AntigravityWorkspaceChatJson` | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     |
| `AntigravitySqlite`            | Partial     | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Partial     | Unsupported | Unsupported |
| `CopilotCliJsonl`              | Unsupported | Unsupported | Assessable  | Unsupported | Unsupported | Unsupported | Assessable  | Unsupported | Unsupported |
| `CopilotIdeChatJson`           | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Partial     | Unknown     | Unknown     |
| `ClineSessionJson`             | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     |
| `ClineMessagesContractV1`      | Unsupported | Unsupported | Partial     | Unsupported | Unsupported | Unsupported | Partial     | Unsupported | Unsupported |
| `KiroSessionJson`              | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     |
| `KiroChat`                     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     |
| `KiroCliV2Bundle`              | Unsupported | Unknown     | Unsupported | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unsupported |
| `KiroCliV3Bundle`              | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     |
| `KiroChatSaveExport`           | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported |
| `AmpThreadJson`                | Partial     | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Partial     | Unsupported | Unsupported |
| `AmpFileChanges`               | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported |
| `WindsurfWorkspaceJson`        | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     |
| `WindsurfMirrorJson`           | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     |
| `WindsurfCascadeProtobuf`      | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     |
| `DevinLocalSqlite`             | Unsupported | Unsupported | Partial     | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported |
| `Uncharacterized`              | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     |
| `OpenCodeSqliteV2`             | Partial     | Unsupported | Partial     | Unsupported | Unsupported | Unsupported | Partial     | Unsupported | Partial     |
| `OpenCodeV2SessionExport`      | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported |

## First-Tier Product Matrix

Support means reachable product behavior, not a parser flag or an unused engine
function. This documentation-only grouping does not gate runtime behavior.
`Finding` is `Y`, `FO` (a finding without a clean-result claim), or `N`.
`Prompt`, `Verification`, and `Estimate` are `Y` or `N`. `Auto Fix` is `Y`, `C`
(only with an exact production binding), `P` (prompt only), or `N`. Prompt `Y`
requires both an engine recommendation and a finding that the desktop can
reach. Estimate `Y` means the finding can reach its typed estimate or bounded
finding-rate fallback; it does not promise a numeric measured value.

The engine contract checks source findings, recommendation support, and estimate
methods. The desktop contract also requires a reachable watch before it accepts
verification, a reachable inventory target before it accepts an M/B/K prompt,
and production editor policy plus a typed target operation before it accepts
Auto Fix. The limit stays beside the row it qualifies.

| Agent       | Check | Finding | Prompt | Auto Fix | Verification | Estimate | Reachability limit                                                                                                                                                                        |
| ----------- | ----- | ------- | ------ | -------- | ------------ | -------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Claude Code | D     | Y       | Y      | Y        | N            | Y        | Auto Fix needs one supported current compaction control.                                                                                                                                  |
| Claude Code | T     | Y       | Y      | Y        | Y            | Y        | Verification needs a later complete lower control on the same route and model.                                                                                                            |
| Claude Code | S     | Y       | Y      | Y        | N            | Y        | Auto Fix needs one exact named agent definition.                                                                                                                                          |
| Claude Code | M     | Y       | Y      | C        | N            | Y        | Current inventory can find targets; Auto Fix needs indexed provenance and one exact MCP binding. Historical subsets cannot verify absence.                                                |
| Claude Code | B     | Y       | Y      | Y        | N            | Y        | Only the allowlisted optional tools are reachable. Historical subsets cannot verify absence.                                                                                              |
| Claude Code | K     | Y       | Y      | C        | N            | Y        | Current inventory can find targets; Auto Fix needs indexed provenance and one exact skill binding. Historical subsets cannot verify absence.                                              |
| Claude Code | O     | Y       | Y      | Y        | Y            | Y        | Auto Fix and verification need publication-time physical attribution.                                                                                                                     |
| Claude Code | F     | Y       | Y      | Y        | Y            | Y        | The source must record the fast tier and one current winning control must still be fast.                                                                                                  |
| Claude Code | C     | Y       | Y      | N        | N            | Y        | No durable cache-policy target exists.                                                                                                                                                    |
| Codex       | D     | Y       | Y      | Y        | N            | Y        | Auto Fix needs one supported current compaction control.                                                                                                                                  |
| Codex       | T     | Y       | Y      | Y        | Y            | Y        | Verification needs a later complete lower control on the same route and model.                                                                                                            |
| Codex       | S     | Y       | Y      | Y        | N            | Y        | Auto Fix needs one exact named agent definition.                                                                                                                                          |
| Codex       | M     | Y       | Y      | C        | N            | Y        | Auto Fix needs indexed provenance and one exact trusted MCP table. Historical subsets cannot verify absence.                                                                              |
| Codex       | B     | Y       | Y      | N        | N            | Y        | The finding is catalog-backed; no reviewed built-in control binds the target.                                                                                                             |
| Codex       | K     | Y       | Y      | C        | N            | Y        | Auto Fix needs indexed provenance and one exact trusted skill binding. Historical subsets cannot verify absence.                                                                          |
| Codex       | O     | Y       | Y      | Y        | Y            | Y        | Auto Fix and verification need publication-time physical attribution.                                                                                                                     |
| Codex       | F     | Y       | Y      | Y        | Y            | Y        | The source must record the fast tier and one current winning control must still be fast.                                                                                                  |
| Codex       | C     | Y       | Y      | N        | N            | Y        | No durable cache-policy target exists.                                                                                                                                                    |
| OpenCode    | D     | Y       | Y      | Y        | N            | Y        | Auto Fix needs one supported current compaction control.                                                                                                                                  |
| OpenCode    | T     | N       | N      | N        | N            | N        | Accepted sources do not retain a historical effort map.                                                                                                                                   |
| OpenCode    | S     | Y       | Y      | Y        | N            | Y        | Auto Fix needs one exact named agent definition.                                                                                                                                          |
| OpenCode    | M     | Y       | Y      | C        | N            | Y        | Current inventory reaches the finding and prompt. Auto Fix additionally needs indexed provenance and one exact effective V2 MCP binding.                                                  |
| OpenCode    | B     | Y       | Y      | P        | N            | Y        | Current inventory reaches only allowlisted optional tools; no production built-in editor policy is available.                                                                             |
| OpenCode    | K     | Y       | Y      | C        | N            | Y        | Auto Fix needs indexed provenance and one exact standard skill winner. Historical subsets cannot verify absence.                                                                          |
| OpenCode    | O     | Y       | Y      | Y        | Y            | Y        | Auto Fix and verification need publication-time physical attribution.                                                                                                                     |
| OpenCode    | F     | N       | N      | N        | N            | N        | Accepted sources do not retain an effective speed tier.                                                                                                                                   |
| OpenCode    | C     | Y       | Y      | N        | N            | Y        | Cache episodes require consecutive validated order and distinct message IDs; Anthropic routes use the five-minute default unless the preceding cache write records one-hour TTL evidence. |
| Pi          | D     | Y       | Y      | Y        | N            | Y        | Auto Fix needs one supported current compaction control.                                                                                                                                  |
| Pi          | T     | Y       | Y      | Y        | Y            | Y        | Effort evidence requires positive usage on that record; verification covers the saved agent-selected policy, not provider-translated effort.                                              |
| Pi          | S     | FO      | Y      | N        | N            | Y        | Only reviewed example-extension output can produce the finding.                                                                                                                           |
| Pi          | M     | Y       | Y      | P        | N            | Y        | Current inventory reaches the finding and prompt, but no exact production editor binding is supported.                                                                                    |
| Pi          | B     | N       | N      | N        | N            | N        | Pi's built-ins are core tools and never become eligible B targets.                                                                                                                        |
| Pi          | K     | Y       | Y      | P        | N            | Y        | Current inventory reaches the finding and prompt, but no exact production editor binding is supported.                                                                                    |
| Pi          | O     | Y       | Y      | Y        | Y            | Y        | Auto Fix and verification need publication-time physical attribution.                                                                                                                     |
| Pi          | F     | N       | N      | N        | N            | N        | Accepted sources do not retain an effective speed tier.                                                                                                                                   |
| Pi          | C     | Y       | Y      | N        | N            | Y        | Reviewed native API routes support repeated-input accounting and recovered miss episodes; unsupported route or episode evidence cannot produce a clean or actionable result.              |
| Cursor      | D     | N       | N      | N        | N            | N        | Characterized Cursor surfaces do not emit request-depth evidence.                                                                                                                         |
| Cursor      | T     | N       | N      | N        | N            | N        | No complete effective effort contract exists.                                                                                                                                             |
| Cursor      | S     | N       | N      | N        | N            | N        | Current relation hints do not establish a detector-grade worker finding.                                                                                                                  |
| Cursor      | M     | FO      | N      | N        | N            | Y        | Current inventory can produce a target; no Cursor remediation recommendation exists.                                                                                                      |
| Cursor      | B     | N       | N      | N        | N            | N        | No optional Cursor built-in is eligible for a B target.                                                                                                                                   |
| Cursor      | K     | FO      | N      | N        | N            | Y        | Current inventory can produce a target; no Cursor remediation recommendation exists.                                                                                                      |
| Cursor      | O     | FO      | Y      | N        | N            | Y        | Characterized direct timed-model observations reach findings and prompts, but never clean or physical attribution.                                                                        |
| Cursor      | F     | N       | N      | N        | N            | N        | No complete effective speed contract exists.                                                                                                                                              |
| Cursor      | C     | N       | N      | N        | N            | N        | No compatible request-accounting contract exists.                                                                                                                                         |
| Antigravity | D     | FO      | Y      | N        | N            | Y        | Direct depth evidence is positive-only.                                                                                                                                                   |
| Antigravity | T     | N       | N      | N        | N            | N        | No complete effective effort contract exists.                                                                                                                                             |
| Antigravity | S     | N       | N      | N        | N            | N        | No persisted detector-grade delegation contract exists.                                                                                                                                   |
| Antigravity | M     | FO      | N      | N        | N            | Y        | Current inventory can produce a target; no Antigravity remediation recommendation exists.                                                                                                 |
| Antigravity | B     | N       | N      | N        | N            | N        | No optional Antigravity built-in is eligible for a B target.                                                                                                                              |
| Antigravity | K     | FO      | N      | N        | N            | Y        | Current inventory can produce a target; no Antigravity remediation recommendation exists.                                                                                                 |
| Antigravity | O     | FO      | Y      | P        | N            | Y        | Direct model evidence reaches a prompt, but no physical model control is attributed.                                                                                                      |
| Antigravity | F     | N       | N      | N        | N            | N        | No complete effective speed contract exists.                                                                                                                                              |
| Antigravity | C     | N       | N      | N        | N            | N        | No compatible request-accounting contract exists.                                                                                                                                         |

Cursor and Antigravity M/K are current-inventory targets, not historical
exposure claims. Pi S remains limited to reviewed example-extension evidence.
M/B/K estimates can use measured replicated tokens when available and otherwise
the bounded finding-rate fallback. The fallback is not measured token or price
evidence.

## Second-Tier Product Coverage

GitHub Copilot, Cline, Kiro, Amp, Devin, and Oh My Pi remain second-tier in
product documentation. This defers no implemented parser, finding path, prompt,
Auto Fix, verification, or burn-estimate behavior. The accepted Copilot CLI v1
event and schema-v7 request-store bundle supports S/O results. It does not
support D because the production reader has no request-depth evidence. Oh My Pi
supports D/T/O results from the shared Pi core. Overdepth reads the largest
single request, so an in-file abandoned branch cannot change the context of
another request. It does not support S, because OMP subagents live in sibling
files this reader does not open. It has no inventory, no remediation prompt,
and no clean result. The other current source and check limits remain the
source inventory and coverage matrix above.

## Evidence Boundaries

Remote session copies retain the accepted Claude Code or Codex transcript
contract; they do not acquire broader coverage by arriving over SSH. A
per-session finding requires sufficient accepted copied evidence. Missing
companions and historical configuration remain unavailable or partial, never
clean. This computer's configuration and provider account do not enrich remote
evidence. Bounded discovery does not establish a complete remote inventory.
Rejected or incomplete exports keep the previous cached generation; a partial
host scan does not establish fresh evidence for the rejected sessions. Skipping
expired listing candidates does not prove a complete companion roster; Codex
exports still require bounded origin discovery for older linked children.
Remote sessions do not contribute to the local global-check report,
Overview, quota attribution, or live HUD. Local path actions, Auto Fix, and
remediation/watch enrollment reject remote origins at the backend boundary.
The product and remediation matrices below describe native supported contexts;
they do not grant remote editing or verification. See
[remote sessions](remote-sessions.md) for the supported host and agent limits.

Lifecycle provider sweeps use same-turn published provider/model evidence and
keep the harness and inferred model vendor separate from the recorded route.
Missing or custom routes do not prove direct provider activity. These display
signals do not authorize a finding, a clean result, remediation, or historical
spend attribution; all check-specific route and API requirements below remain
unchanged. See [`session-lifecycle-events.md`](session-lifecycle-events.md#scoped-sweep-evidence).

Burn checks use only sessions admitted by the repository scan gate. A session
needs a resolvable Git repository CWD, either recorded or inferred from
transcript paths below a parent-folder CWD. Disabled roots and their linked
worktrees are excluded before evidence processing; missing or unresolved CWDs
are unavailable, never clean. When "Include folders without git" is on, a
session outside a repository enters the index with no project root. Its findings
can still show, but its agent's resource inventory counts as limited, so that
agent gets no clean result, and remediation has no repository target to edit.

No current session reader proves a full historical resource inventory. The
legacy per-session M/B/K rules deny `Clean`, even when a nested observed-resource
map is complete. The desktop target assessment can report clean only when every
applicable current inventory scan and positive-use input is complete and within
its bounds. A scoped finding requires complete coverage of that observed subset,
calls, and eligible activity. An unrelated partial resource group does not block it.
Detector-level absence never verifies an M/B/K remediation. Unavailable,
partial, wrong-source, wrong-agent, wrong-scope, truncated, and non-applicable
evidence does not verify the target.

The native desktop advisory inventory is separate from these per-session
detector rules. It enumerates bounded standard current resources for Claude
Code, Codex, Cursor, Copilot, Cline, OpenCode, Kiro, Amp, Antigravity,
Devin/Windsurf, and Pi and can merge current indexed resource observations. Its result
contains logical names, state, scope, provenance, and
limits only. Skill candidates can include a proportional token estimate for the
frontmatter `title` or `name` plus `description`. The estimate excludes the
skill body. The inventory contains no physical path or selector.

The desktop 30-day report reduction creates a separate target-based M/B/K
assessment. One key contains the exact agent, resource kind, normalized name,
and global or canonical repository scope. Any exact positive use in that scope
suppresses the key. Remaining candidates become one target each, not one target
per session. The assessment records target totals and at most three supporting
sessions per target. The shipped Checks DTO and action command use this
assessment for M/B/K category counts, agents, status, burn estimates, and named
target rows. They do not use the older session-based M/B/K rows.

Positive use can come from tool counts, invoked loaded sources,
catalog-backed tool definitions, or persisted initial-context source rows.
Observed positive facts remain valid inside partial evidence. Partial or
unsupported facts, malformed or dynamic applicable inventory, unknown scope,
ambiguous aliases, failed scans, and repository, context, use, directory, or
target caps block clean. They do not remove a known finding from an unrelated
kind, scope, or agent. Disabled current resources do not become candidates.

Global candidates share use only across the same agent. Project candidates
share use only inside one canonical accessible repository. A raw call with no
origin uses a same-name project candidate in its repository before a global
candidate. Unknown-origin indexed resources do not become scoped targets.
Same-name resources in separate scopes and repositories stay separate.

Resource burn estimates use the report's existing total-token denominator and
rounding. Skill listing tokens use the existing proportional `chars / 4`
estimate and replicate across applicable assistant turns. MCP estimates use
only measured indexed definition tokens. Claude Code and Codex built-in tools
reuse measured catalog definitions. Only optional specialized tools can become B
targets; required shell, read, write, edit, search, and subagent tools remain
measured but never become findings. OpenCode 1.2.15 and Pi 0.52.12 use pinned
default catalog captures. A matching positive use removes the target and its
estimate. A missing definition, missing denominator, cap, truncation, or
arithmetic failure uses the detector's bounded finding-rate fallback instead
of presenting no percentage. Measured token attribution always replaces that
fallback. The fallback is an estimated workload share, not measured tokens or
price data. It stays within the 0% to 100% display range.

B tool eligibility is a product safety policy, not a claim that vendors make
other tools impossible to disable. The only eligible names are Claude Code
`WebSearch`, `WebFetch`, `Workflow`, `ReportFindings`, and `ScheduleWakeup`, Codex `web_search`, and OpenCode `websearch` and
`webfetch`. Pi, Cursor, Copilot, Cline, Kiro, Amp Code, Antigravity, and
Windsurf have no B target. Shell, read, write, edit, search, task, agent, and
subagent tools remain measured but never become B targets for any agent.
Every displayed resource target has at least one supporting failed session. A
resource without that session evidence makes the category unavailable instead
of creating a target without a session to open.

The reviewed primary sources are Claude Code [permissions](https://code.claude.com/docs/en/permissions)
and [tools](https://code.claude.com/docs/en/tools), Codex [configuration
reference](https://developers.openai.com/codex/config-file/config-reference),
OpenCode [permissions](https://opencode.ai/docs/permissions/) and
[tools](https://opencode.ai/docs/tools/), Pi [settings](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/settings.md#tools),
and Cursor [SDK tool restrictions](https://cursor.com/docs/sdk/python#restricting-the-toolset).
These sources show that several core tools are configurable. They do not
identify them as optional, so Antiburn does not suggest disabling them.
Claude Code documents `Workflow` as a separate dynamic-workflow tool and
documents independent feature controls in its [workflows](https://code.claude.com/docs/en/workflows#turn-workflows-off)
and [settings reference](https://code.claude.com/docs/en/settings-reference#disableworkflows).

Provider matching accepts exact case-insensitive names and the reviewed call
forms: Claude Code and Codex `mcp__<server>__<tool>`, OpenCode
`<server>_<tool>`, and `pi-mcp-extension` 1.5.0 default
`mcp_<sanitized-server>_<tool>`. Claude Code and Codex accept a unique bare
suffix for namespaced skills and the final segment of a catalog-backed built-in
alias. Ambiguous skill aliases suppress every possible matching target and
block clean. OpenCode and Pi MCP names can be ambiguous because the server and tool share one underscore
delimiter. Such a call suppresses every possible matching server finding and
blocks clean, but it does not count any server as used. OpenCode and Pi use exact
skill and built-in names.

The accepted current shapes are Claude standard user/project `mcpServers`,
standard `.claude/skills`, `skillOverrides`, and exact permission controls;
Codex trusted layered `mcp_servers` with omitted `enabled` treated as enabled,
plus standard `.agents/skills` and compatibility `.codex/skills`; OpenCode
JSON/JSONC direct and `mcp.servers` maps, standard skill roots, Boolean `tools`,
and object or array permission controls; and Pi `defaultTools`, standard skill
roots, explicit non-pattern skill directories, and pinned
`pi-mcp-extension` 1.5.0. The Pi package manifest must name version 1.5.0 and
`./src/index.ts`; the reviewed producer commit is
`8a01fc53f3289d2e8eb492d67ba45cd84d64e7f2`. Runtime, managed, remote, plugin,
pattern, lazy Pi MCP, malformed, unsafe, capped, conflicting, and partial
indexed sources remain explicit clean-result limits.

Inventory files are Cursor `.cursor/mcp.json`; Copilot
`~/.copilot/mcp-config.json`, `.mcp.json`, and `.github/mcp.json`; Cline MCP
settings; Kiro `.kiro/settings/mcp.json`; Amp settings; Antigravity
`mcp_config.json`; and Devin/Windsurf MCP settings. Their documented skill roots
are scanned in global and project scope. Where supported, global
`~/.agents/skills` is scanned with the vendor-specific root, and duplicate
same-agent, same-scope identities are merged. These inputs are current state,
never historical exposure, and malformed, dynamic, plugin, unsupported, or
capped inputs block clean.

OpenCode current inventory requires a verified registered V2 consumer receipt,
regardless of the transcript's source format. Each report captures authority
once per CWD outside its store snapshot. A failed capture stays
unavailable for that report; inventory does not retry it or infer the current
consumer from an older transcript. Static legacy config parsing does not replace
this authority check.

Report-time token estimates (`insights/report.rs::token_cost` and
`TokenBurnTurnEvidence`), old-model remediation savings, and provider-limit
attribution all read `turn.cache_write_1h_tokens` and price that subset at
two times the input rate. These readers are report-time views of turn rows,
not part of `SessionEvidence`, so the evidence schema revision does not
change when this pricing split changes.

Thread attribution retains at most 16,384 distinct UUIDs, each at most 256 bytes.
An overflow or oversized UUID records `CapExceeded`, makes attribution
incomplete, and blocks every clean result that needs complete affected evidence.
An oversized resume identity set is rejected instead of being trusted.

Codex pairs `token_usage_record` and `event_msg`/`token_count` records once.
Matching per-response and nonempty cumulative usage identifies exact copies
without a time limit. Different cumulative producer bases require matching
per-response usage within five seconds. Available identities distinguish
same-format requests. The reader retains bounded state across resume boundaries.
Unmatched valid usage remains evidence; malformed usage remains partial.

Cache accounting compares usage-bearing requests across ordinary Codex assistant
messages. Broken links, unknown requests, route changes, and compaction boundaries
still break pairs. The paid denominator includes every eligible request,
including each segment's initial payment. Partial cache or repeated-context
evidence permits neither a ratio finding nor a clean result.

Maintainer confirmation (2026-09-12): repair delayed exact-copy deduplication,
request pairing, and full-denominator accounting. Reviewed passive alternatives
include increasing the time limit and adjusting thresholds; neither repairs
all three accounting errors. Per-session thresholds remain unchanged.

Maintainer confirmation (2026-09-10): raise the identity cap to 16,384 as an
interim measure for longer sessions. Reviewed passive alternatives include
indexed local relationship queries and bounded batch processing. Those
alternatives are not implemented; sessions above the cap still lose
clean-result eligibility. Analyzer revision 22 reprocesses prior evidence.

Skills mean full documents injected into model context. Listings, installed
skills, and names in tool calls do not prove unused document overhead. Resource
identity is retained without copying private document bodies into evidence.

| Source                      | Checks              | Implemented contract and remaining limit                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| --------------------------- | ------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Claude and Codex            | D, O                | Direct request depth and timed model use reach checks independently of token-accounting policy. Clean needs all required session facts and reviewed model state. Unknown models are not automatically current.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| Claude and Codex            | T, F                | Request-level model, effort, speed, and route observations are evaluated together. Codex preserves explicit provider/control changes and inherited fork state; child controls retain delegated scope. Missing eligible signals or unreviewed routes deny clean.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| Claude                      | S                   | Exact `Task`/`Agent` call IDs join unique sidecar `toolUseId` claims to actual child models and the model on the parent call. Missing, duplicate, malformed, mismatched, or nested-parent claims remain partial. Requested model aliases and directory ancestry are not proof.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| Codex                       | S                   | Owned `spawn_agent` records and discovered child rollouts provide delegation and actual models. Incomplete child evidence cannot prove clean.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| Claude                      | M, K                | Observed MCP injection plus exact server calls and full skill-document injection plus invocation identity support scoped findings. Skill listings alone do not. The observed subset is not a historical inventory.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| Codex                       | M                   | Accepted completed client `tool_search_output` namespace records expose exact MCP server identities. Complete observed exposure and calls support scoped findings without a full-inventory capability. Ambiguous or incomplete search results do not establish injection.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| Claude and Codex            | B                   | The existing harness-version/model catalog path supports scoped definition findings with complete calls. Deferred, situational, and zero-cost definitions are excluded. Catalog resolution does not prove every historical enabled tool or exposure change; it cannot justify a whole-inventory claim.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| Codex                       | K                   | Selected full skill documents reach observed injection/invocation evidence. Listings remain availability only. Selected documents do not establish unused listing overhead or full inventory coverage.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| OpenCode                    | S                   | Native `task` metadata identifies the child session and model; ancestry and the child's assistant model must agree. The parent model comes from the task request. A bare `subtask`, fork, or `parent_id` relation is insufficient.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| OpenCode                    | K                   | Complete native selected-skill results preserve full identity as injected and invoked. Truncated, compacted, empty, or invalid result wrappers do not prove full injection. This is observed selected-skill support, not an unused-listing finding or complete inventory.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| OpenCode                    | T, M, B, F          | Confirmed unsupported for the reviewed sources: no historical effort map, model-facing resource inventories, or effective speed tier. The reader does not retain a variant as effort. Variant labels, current configuration, and tool registries cannot substitute.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| Pi                          | T                   | `EffortSemantics::AgentSelectedPolicy` evaluates the saved agent-selected thinking level, not translated provider effort. Above-cap findings require positive usage on the same model/effort observation. Missing usage, levels/routes, and unknown models fail closed; provider overrides are not guessed.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| Pi                          | S                   | Existing output from the official subagent example extension supplies nested `toolResult` messages, exact native call/worker identity, and actual models. This is finding-only. Arbitrary extensions, fork ancestry, requested aliases, and a nonpremium observed worker cannot establish clean.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| Pi                          | M, B, K, F          | Confirmed unsupported for the reviewed sources. Tool calls and bounded skill invocation identity do not establish historical resource exposure or speed. No alternative local proof was identified.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| Claude, Codex, OpenCode, Pi | C                   | Durable request provider/API fields and the compatible-request query select reviewed cache-write or uncached-input accounting. A finding additionally needs a same-route hit/miss/recovery episode after the route's reviewed cache lifetime: Claude Code uses its configured one-hour default; Pi and OpenCode Anthropic routes use five minutes unless an earlier write records one-hour TTL evidence, which carries across hits and refreshes at hit request start; reviewed OpenAI routes use 30 minutes. OpenCode continuity uses validated consecutive order and distinct message IDs because `parentID` identifies the answered user, not the predecessor. Main-thread identity, order, token classes, model, route, and compaction boundaries constrain pairs. Codex pairs `token_usage_record` with equivalent `token_count` usage by per-response fields; matching cumulative fields permit delayed exact copies. Unknown or incompatible segments prevent findings and clean results. Google cache policy remains unreviewed. |
| OpenCode                    | C                   | Both accepted export and SQLite shapes use validated ordered history. `parentID` identifies the user being answered, not the predecessor. Missing wrappers/timestamps, duplicate or out-of-order messages, and unresolved forks prevent complete history. CoreV2 `session_message` is not the existing SQLite table contract.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| Cursor                      | D, O                | O retains direct timed-model findings; the source gate denies clean on every surface. D remains unavailable because the current reader does not emit request-usage evidence. Synthetic source-gate tests do not establish native parsing support.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| Cursor                      | T, S, M, B, K, F, C | Broader surface characterization is deferred. Current settings, relations, inventories, and cache evidence remain partial, unknown, or unsupported as listed; no new parity claim is made.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| Antigravity                 | D, O                | Brain/cascade steps and native SQLite preserve direct usage/model findings where present. Missing model/time is not filled from an earlier step or an invented database timestamp. Private identity, enum, and completeness gaps deny clean.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| Antigravity                 | T, S, M, B, K, F, C | Confirmed unsupported in the reviewed native evidence. Token classes do not establish compatible request linkage or cache cause. Runtime descriptors and unproved relationship sidecars do not establish persisted delegation, controls, or resource exposure. Workspace chat remains uncharacterized.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |

M automatic remediation accepts only one named, enabled target with indexed
provenance and an exact current editor resolution. Codex resolves exactly one
active trusted project or global `mcp_servers.<name>` table. Claude resolves
exactly one standard project or global MCP source and adds only its matching
deny rule to one same-scope existing settings file. OpenCode has an exact
`enabled = false` editor, but M remains prompt-only when indexed provenance or
one exact current control is unavailable.
Antigravity has no public source and precedence proof for one persisted disable
field. Cursor MCP remediation is unavailable and never reads a private toggle
store or invokes a CLI command.

Cache churn selects its policy from `RepeatedContextAccounting`, not from the
agent or the session's dominant model. `CacheWrite` uses the reviewed Claude
family policy. `UncachedInput` uses the reviewed OpenAI family policy. This rule
also applies to mixed-family sessions. A cache-churn cause names a model from the
same accounting family; it does not use an unrelated dominant model. Repeated
input totals remain available even when there is no actionable episode. A
continuous-activity hit/miss/recovery episode is informational and leaves a
high-ratio result not assessed. A finding requires recovered cache use after a
supported route-specific user inactivity interval. Unknown routes, missing
timestamps, broken identity, compaction, model changes, and unrecovered misses
cannot establish that finding or a clean result.

Old-model causes remain separate by provider, API, observed model, and reviewed
replacement. Token-burn percentages are unknown when a required price or the
total-token denominator is absent. The estimator does not use a 10 percent
fallback and does not force a positive minimum.

## Quota and provider incidents

Quota pressure and provider incidents are separate from the nine native checks
(FR-15). Each reports only when transcripts contain evidence of that incident
type. `CodexRolloutJsonl` and `ClaudeJsonl` supply both types.

`CodexRolloutJsonl`: an `event_msg`/`task_complete` record with a non-null
`error` object maps to one of the two groups, against the pinned
`openai/codex` protocol commit
[`e7637306bc9246a3e42e407cb94f96b7ed345e3e`][codex-source] and a synthetic
fixture (`task_complete_errors.jsonl`):

- `quota_incidents`, a `QuotaIncident`, from `rate_limit_exceeded`
  (`RateLimit`) and `usage_limit_exceeded` (`UsageLimit`). Both identify a
  user-allocation limit.
- `provider_incidents`, a `ProviderIncident`, from `server_overloaded`
  (`Capacity`), `internal_server_error` (`ServerError`), and the four
  transport codes `http_connection_failed`,
  `response_stream_connection_failed`, `response_stream_disconnected`, and
  `response_too_many_failed_attempts` by their `http_status_code`: `5xx`
  maps to `ServerError`, an absent or `null` status maps to `Connection`,
  and any other status is ignored because the retry wrapper hides which
  layer produced it.

Every other Codex code (`context_window_exceeded`, `session_budget_exceeded`,
`cyber_policy`, `misalignment_policy_violation`, `unauthorized`,
`bad_request`, `sandbox_error`, `active_turn_not_steerable`,
`thread_rollback_failed`, `other`) is ignored.

`ClaudeJsonl`: a `type: "assistant"` record with `isApiErrorMessage: true`
maps to one of the two groups from its `apiErrorStatus` (an HTTP status,
present only for a response the provider returned) and `error` (Claude
Code's own coarser classification), reviewed against harness version
`2.1.270` and a synthetic fixture (`api_error_records.jsonl`):

- `quota_incidents`, a `QuotaIncident` (`RateLimit`, `HardHit`), from status
  `429` or, when no status is present, `error: "rate_limit"`.
- `provider_incidents`, a `ProviderIncident`, from status `529` (`Capacity`),
  another `5xx` status (`ServerError`), or, when no status is present,
  `error: "server_error"` (`ServerError`).

A Claude quota incident also carries the limit family and reset time stated in
its message text. A `session limit` text gives `RollingWindow`; a `weekly limit`
text gives `Weekly`. If neither phrase matches, the kind stays `RateLimit`,
because `apiErrorStatus` alone still proves the refusal. The stated reset becomes
a `QuotaResetClock` with an hour, a minute, and a named zone. It is not an instant:
the engine holds no zone database, so the application resolves the clock. The
reader keeps the two parsed values and drops the text; no message text is stored.
Evidence written before this field existed deserializes with no clock.

Every other status or `error` value is ignored, including `error: "unknown"`
with no status (Claude's connection-refused case) and every 4xx other than 429.
`ProviderIncidentKind::Connection` is Codex-only. No reviewed Claude field
identifies a connection failure without reading message text.

Neither group supports a clean result or an absence claim for any source. Each
section remains not assessed without at least one observed incident of its own
kind, as required by FR-15.

Maintainer confirmation (2026-09-14) covers `ServerError` and `Connection`, the
pinned Codex transport/server `codex_error_info` mappings, and the characterized
Claude `isApiErrorMessage` source. The following alternatives remain unsupported:

- Classifying Claude errors from `content[].text`: this free text is unpinned.
  The reader parses only the limit family and reset clock from an
  `isApiErrorMessage` text. It keeps neither that text nor any other message text.
- Mapping Claude `error: "unknown"` to `Connection`: the label covers more than
  connection failures.
- Mapping non-5xx `http_status_code` values inside Codex transport variants: the
  retry wrapper hides which layer produced the status.
- Mapping `context_window_exceeded` / `session_budget_exceeded`: these name the
  user's own context or budget, a different failure class.
- Splitting Claude 429s into `UsageLimit` vs `RateLimit` from `quotaLimits`: this
  field has no characterized synthetic fixture.

## Passive Verification

Every T, O, or F finding with supported positive verification from a winning
`Ready` evidence publication can create one passive attempt, up to 100 attempts
per publication. Candidate filtering applies the detector, agent, source-format,
physical-target, and non-resource requirements before bounded selection. The
selection is fair across all nine detectors. V45 does not backfill old published
rows. The immutable boundary is the publication time in milliseconds, not the
session time. Replay reuses an active target. After recurrence, a later
publication can create a new attempt. A later action keeps a separate action
attempt for the same target; it does not replace the passive attempt or move its
boundary. Attempt creation, dirtying, evidence publication, and fenced row
replacement share the winning transaction.

The table below is the exact implemented positive-proof matrix. `Supported`
means the current backend can verify a fixed transition. `Unavailable` means it
does not enroll a passive attempt and cannot prove the initial fix from the
accepted passive evidence. An explicit action can store
`verificationUnavailable`. Source coverage from the main matrix still applies.

| Check | Claude Code | Codex       | OpenCode    | Pi          | Cursor      | Antigravity | Proof or blocker                                                                                                                                                                              |
| ----- | ----------- | ----------- | ----------- | ----------- | ----------- | ----------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| D     | Unavailable | Unavailable | Unavailable | Unavailable | Unavailable | Unavailable | The finding identity is one historical session. A later session is not positive proof that the original session changed.                                                                      |
| T     | Supported   | Supported   | Unavailable | Supported   | Unavailable | Unavailable | A complete later assessment plus an explicit same-route, same-model lower control proves the transition. Pi proves only its agent-selected policy.                                            |
| S     | Unavailable | Unavailable | Unavailable | Unavailable | Unavailable | Unavailable | A later worker or call has a different identity. No accepted source records a durable worker-setting transition.                                                                              |
| M     | Unavailable | Unavailable | Unavailable | Unavailable | Unavailable | Unavailable | Observed resource subsets cannot prove that a server was removed or disabled.                                                                                                                 |
| B     | Unavailable | Unavailable | Unavailable | Unavailable | Unavailable | Unavailable | Observed resource subsets cannot prove that a tool was removed or disabled.                                                                                                                   |
| K     | Unavailable | Unavailable | Unavailable | Unavailable | Unavailable | Unavailable | Observed resource subsets cannot prove that a skill was removed or disabled.                                                                                                                  |
| O     | Supported   | Supported   | Supported   | Supported   | Unavailable | Unavailable | The strict verifier requires actual replacement use on the same publication-attributed physical target, scope, provider, and API. Cursor and Antigravity have no physical target attribution. |
| F     | Supported   | Supported   | Unavailable | Unavailable | Unavailable | Unavailable | A complete later assessment plus an explicit same-route, same-model standard-tier delegated request proves the transition.                                                                    |
| C     | Unavailable | Unavailable | Unavailable | Unavailable | Unavailable | Unavailable | A later request pair is not the same session-route target and does not prove a durable cache-policy transition.                                                                               |

Truncated assessment sets, sessions that start at or before the boundary,
missing controls, changed detector or catalog policy, stale projections, and
unsupported source contracts return verification unavailable or continue
watching. They never become fixed through generic absence. For T and F, one
complete later session must contain the exact positive control for the same
target. For O, one later session must contain actual replacement-model use for
the same attributed target. Report-level absence and historical counts do not
verify a fix. A fixed supported target recurs only on a later exact positive
observation. Prior contributions end at the recurrence boundary and remain
durable.

The engine verifier is fail-closed for M/B/K. Historical session subsets are
always unavailable, whether they contain or omit the target. Only a complete,
bounded later current inventory with matching agent, source, scope, and use
coverage could verify absence or recurrence. No production desktop path supplies
that inventory to a watch today, so reachable M/B/K verification is unavailable.
Their stored resource selector cannot use model proof, establish report clean,
or create verified savings.

A successful M/B/K Auto Fix retains only the crash-safe write record with
`verificationUnavailable`. The result is `applied_verification_unavailable`, not
`applied_awaiting_verification`, and the UI does not move the check into the
awaiting-verification group.

## Automatic Editor Support

`Auto Fix` means the backend can bind a finding to one effective physical
setting, prepare a reviewed edit, and recover an uncertain write. Resource Auto
Fix additionally requires indexed provenance and an exact current resource,
scope, value, and physical key. `Prompt only` means the existing bounded prompt
can describe the finding, but the backend cannot prove one safe physical edit.
Source versions mean the accepted source shapes in the source inventory. No row
claims every historical agent release.

Each Auto Fix edits one winning control. An inherited value selects its global
or user control. A project target requires the exact explicit project setting or
resource; the editor never creates a project config. Scalar edits are not
batched across active layers. Findings from multiple projects that resolve to
the same global control form one target, and prepare and apply revalidate every
grouped project context. Model and reasoning edits remain pinned to the
publication-time scope and physical target.

| Agent                            | Operation                   | Result                        | Reason or exact limit                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| -------------------------------- | --------------------------- | ----------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Claude Code                      | Model replacement           | Auto Fix                      | `ClaudeJsonl` only. The publication must bind the observed main-loop model to an existing effective `model` setting.                                                                                                                                                                                                                                                                                                                                                                                |
| Claude Code                      | Reasoning effort            | Auto Fix                      | `ClaudeJsonl` T findings only. The publication must bind the observed level to an existing effective top-level `effortLevel` or model-specific `modelSettings.<model>.effortLevel`. The reviewed replacement is `medium`.                                                                                                                                                                                                                                                                           |
| Claude Code                      | Fast mode                   | Auto Fix                      | `ClaudeJsonl` F findings only. The finding needs explicit fast-tier evidence and the current winning existing `fastMode` value must be `true`; the editor writes `false` to that one control so an inherited global `true` cannot become effective. No publication-time config attribution is required.                                                                                                                                                                                             |
| Claude Code                      | Named subagent model        | Auto Fix                      | `ClaudeJsonl` S findings only. The current scan must locate exactly one named Markdown agent whose frontmatter `model` equals the observed worker model.                                                                                                                                                                                                                                                                                                                                            |
| Claude Code                      | MCP control                 | Auto Fix or prompt only       | Auto Fix requires indexed provenance, one current standard project or global MCP source, and one exact same-scope existing settings target. Otherwise the named target remains prompt-only.                                                                                                                                                                                                                                                                                                         |
| Claude Code                      | Skill control               | Auto Fix or prompt only       | Auto Fix requires indexed provenance and one current standard `SKILL.md` winner. The editor writes only `skillOverrides.<name> = "off"`. Inventory-only or ambiguous targets remain prompt-only.                                                                                                                                                                                                                                                                                                    |
| Codex                            | Model replacement           | Auto Fix                      | `CodexRolloutJsonl` only. The publication must bind the observed main-thread model to an existing effective top-level `model`. Project edits require an explicit `trust_level = "trusted"` entry and a repository-root cwd.                                                                                                                                                                                                                                                                         |
| Codex                            | Reasoning effort            | Auto Fix                      | `CodexRolloutJsonl` T findings only. The publication must bind the observed level to an existing effective top-level `model_reasoning_effort`. The reviewed replacement is `medium`.                                                                                                                                                                                                                                                                                                                |
| Codex                            | Fast service tier           | Auto Fix                      | `CodexRolloutJsonl` F findings only. The finding needs explicit fast-tier evidence and the current winning existing `service_tier` must be `fast`; the editor changes it to reviewed `standard`. No publication-time config attribution is required.                                                                                                                                                                                                                                                |
| Codex                            | Named subagent model        | Auto Fix                      | `CodexRolloutJsonl` S findings only. The current scan must locate exactly one named agent TOML file whose `model` equals the observed worker model.                                                                                                                                                                                                                                                                                                                                                 |
| Codex                            | MCP enablement              | Auto Fix or prompt only       | Auto Fix requires indexed provenance and one exact active trusted project or global `mcp_servers.<name>` table. Inventory-only, duplicate, or untrusted targets remain prompt-only.                                                                                                                                                                                                                                                                                                                 |
| Codex                            | Skill enablement            | Auto Fix or prompt only       | Auto Fix requires indexed provenance and one current trusted standard `SKILL.md` winner with one matching `skills.config.<name>.enabled` control. Other targets remain prompt-only.                                                                                                                                                                                                                                                                                                                 |
| OpenCode                         | Model default               | Auto Fix                      | `OpenCodeJsonl` and `OpenCodeSqliteV1` O findings only. Direct `openai`, `anthropic`, and `google` provider IDs use their reviewed native API when OpenCode omits it. Publication must bind the observed `provider/model` route to the effective merged `model` value. Dynamic, remote, agent, mode, and managed overrides are rejected.                                                                                                                                                            |
| OpenCode                         | Named subagent model        | Auto Fix                      | S findings only. The current scan must locate exactly one named Markdown agent whose frontmatter `model` equals the observed worker model. Variant-only workers remain unavailable.                                                                                                                                                                                                                                                                                                                 |
| OpenCode                         | Reasoning control           | Unavailable                   | The accepted sources have no historical effort map. A variant label is not an effective reasoning control.                                                                                                                                                                                                                                                                                                                                                                                          |
| OpenCode                         | MCP control                 | Auto Fix or prompt only       | Current inventory can create a named target. Auto Fix requires indexed provenance and one exact effective V2 MCP control; other targets remain prompt-only.                                                                                                                                                                                                                                                                                                                                         |
| OpenCode                         | Skill control               | Auto Fix or prompt only       | Auto Fix requires indexed provenance and one current standard `SKILL.md` winner. The editor appends one V2 `skill` deny with that exact resource. Other targets remain prompt-only.                                                                                                                                                                                                                                                                                                                 |
| Pi                               | Model and provider default  | Auto Fix                      | `PiV3Jsonl` O findings only. Publication must bind the observed `provider/model` route to an existing paired `defaultProvider` and `defaultModel` setting.                                                                                                                                                                                                                                                                                                                                          |
| Pi                               | Thinking level              | Auto Fix                      | `PiV3Jsonl` T findings only. Publication must bind the saved agent-selected level to an existing route-specific `modelThinkingLevels` entry or `defaultThinkingLevel`. The reviewed replacement is `medium`.                                                                                                                                                                                                                                                                                        |
| Pi                               | MCP or skill control        | Prompt only                   | Current inventory can create named M/K targets. Pi core built-ins never become B targets. The production policy has no exact M/K editor binding. No `SKILL.md` file is changed.                                                                                                                                                                                                                                                                                                                     |
| Pi, Cursor, Antigravity          | Named subagent model        | Unavailable                   | Pi extension output and Cursor or Antigravity findings do not bind one effective persisted worker-model selector.                                                                                                                                                                                                                                                                                                                                                                                   |
| Claude Code, Codex, OpenCode, Pi | Session compaction          | Auto Fix for D only           | The current project or global config must contain a supported disabled compaction flag or a numeric limit above the finding depth cap. The editor enables the flag or lowers that limit to the cap. Ordinary session growth, enabled controls, fixed instructions, runtime overrides, and unsupported schemas remain unavailable.                                                                                                                                                                   |
| Antigravity                      | Model or documented setting | Prompt only for O findings    | Accepted sources can retain direct model use, but no accepted IDE or CLI source binds it to one effective documented physical setting.                                                                                                                                                                                                                                                                                                                                                              |
| Antigravity                      | MCP control                 | Unavailable                   | The reviewed native evidence has no MCP exposure or effective-control contract. IDE and CLI configuration cannot be interchanged.                                                                                                                                                                                                                                                                                                                                                                   |
| Claude Code                      | Built-in tool               | Auto Fix for optional B tools | One exact observed tool must have a matching canonical permission name. A project target requires that bare name in its exact `permissions.allow` array. Otherwise the inherited control is global. The editor can add the bare deny to existing global settings or create the missing global settings file. `Bash`, `Edit`, `Read`, and `Write` remain measured but cannot receive Auto Fix or a targeted disable prompt. Wildcards, scoped rules, and general permission changes are unavailable. |
| Codex                            | Built-in tool               | Unavailable                   | The documented `apps.<id>.tools.<tool>.enabled` control applies to an app tool, not one built-in tool identity.                                                                                                                                                                                                                                                                                                                                                                                     |
| OpenCode                         | Built-in tool               | Prompt only                   | Current inventory can create an allowlisted optional target, but the production policy does not expose a built-in automatic edit.                                                                                                                                                                                                                                                                                                                                                                   |

The retained engine recommendation matrix below comes from
`remediation/prompts.rs`. It describes helper capability, not current product
reachability. Use
the first-tier product and source-format remediation matrices for reachable
prompts; a helper `Yes` cannot supply a new finding.
The check-level Copy action can return bounded generic text for one or more
selectable current targets. A complete query with no selectable target returns no
prompt.
An empty truncated query can return inspection-only text without a current
finding claim.
Each explicit click queries current targets and prepares fresh text, including
retries after clipboard failure. Target-level Copy resolves the current target
by stable finding ID. A missing selected identity can return inspection-only text
without old target facts or watch enrollment. This does not establish finding or
source support. Existing remediation state does not block generation.
Fallback text retains all source, agent, check, and protected-tool limits; it
does not establish new prompt support or exact advice without current evidence.

Prompt enrollment leaves existing exact active action attempts unchanged.
Missing eligible attempts enroll as one batch only if all fit the capacity
bound, without pruning history. Full capacity still permits valid prompt text.
Each selected waiting attempt returned by enrollment gets one exact opaque
`ABR-` marker, up to 100. Other states and unenrolled targets need no new
reference. The 64 KiB prompt bound includes every marker. Incomplete selections
or oversized aggregate text can use a bounded inspection fallback. A matched
ineligible target remains unavailable.

| Agent and source                                                                                          | D   | T   | S   | M   | B   | K   | O   | F   | C   |
| --------------------------------------------------------------------------------------------------------- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Claude Code, `ClaudeJsonl`                                                                                | Yes | Yes | Yes | Yes | Yes | Yes | Yes | Yes | Yes |
| Codex, `CodexRolloutJsonl`                                                                                | Yes | Yes | Yes | Yes | Yes | Yes | Yes | Yes | Yes |
| OpenCode, `OpenCodeJsonl` or `OpenCodeSqliteV1`                                                           | Yes | No  | Yes | Yes | Yes | Yes | Yes | No  | Yes |
| Pi, `PiV3Jsonl`                                                                                           | Yes | Yes | Yes | Yes | No  | Yes | Yes | No  | Yes |
| Cursor, `CursorCliAgentJsonl`                                                                             | No  | No  | No  | No  | No  | No  | No  | No  | No  |
| Antigravity, `AntigravityJson`, `AntigravityBrainJsonl`, `AntigravityCascadeJson`, or `AntigravitySqlite` | Yes | No  | No  | No  | No  | No  | Yes | No  | No  |

### Source-Format Remediation Matrix

This exact matrix lists every `SourceFormat` once. Check-code lists are typed
sets: `None` or slash-separated codes from D/T/S/M/B/K/O/F/C with no duplicates.
The rows describe reachable remediation, so a production recommendation without
a reachable finding is not listed. Auto Fix columns describe production policy;
each operation still needs an exact target binding at runtime.

| `SourceFormat`                 | Prompt checks     | Model Auto Fix | Reasoning Auto Fix | Other Auto Fix checks | Verification checks |
| ------------------------------ | ----------------- | -------------- | ------------------ | --------------------- | ------------------- |
| `ClaudeJsonl`                  | D/T/S/M/B/K/O/F/C | O              | T                  | D/S/M/B/K/F           | T/O/F               |
| `CodexRolloutJsonl`            | D/T/S/M/B/K/O/F/C | O              | T                  | D/S/M/K/F             | T/O/F               |
| `OpenCodeJsonl`                | D/S/M/B/K/O/C     | O              | None               | D/S/M/K               | O                   |
| `OpenCodeSqliteV1`             | D/S/M/B/K/O/C     | O              | None               | D/S/M/K               | O                   |
| `PiV3Jsonl`                    | D/T/S/M/K/O/C     | O              | T                  | D                     | T/O                 |
| `OmpV3Jsonl`                   | None              | None           | None               | None                  | None                |
| `CursorJsonl`                  | O                 | None           | None               | None                  | None                |
| `CursorCliAgentJsonl`          | O                 | None           | None               | None                  | None                |
| `CursorCliStoreDb`             | O                 | None           | None               | None                  | None                |
| `CursorChatStoreDb`            | O                 | None           | None               | None                  | None                |
| `CursorIdeComposer`            | O                 | None           | None               | None                  | None                |
| `CursorLegacyChatJson`         | None              | None           | None               | None                  | None                |
| `AntigravityJson`              | D/O               | None           | None               | None                  | None                |
| `AntigravityBrainJsonl`        | D/O               | None           | None               | None                  | None                |
| `AntigravityCascadeJson`       | D/O               | None           | None               | None                  | None                |
| `AntigravityWorkspaceChatJson` | None              | None           | None               | None                  | None                |
| `AntigravitySqlite`            | D/O               | None           | None               | None                  | None                |
| `CopilotCliJsonl`              | None              | None           | None               | None                  | None                |
| `CopilotIdeChatJson`           | None              | None           | None               | None                  | None                |
| `ClineSessionJson`             | None              | None           | None               | None                  | None                |
| `ClineMessagesContractV1`      | None              | None           | None               | None                  | None                |
| `KiroSessionJson`              | None              | None           | None               | None                  | None                |
| `KiroChat`                     | None              | None           | None               | None                  | None                |
| `KiroCliV2Bundle`              | None              | None           | None               | None                  | None                |
| `KiroCliV3Bundle`              | None              | None           | None               | None                  | None                |
| `KiroChatSaveExport`           | None              | None           | None               | None                  | None                |
| `AmpThreadJson`                | None              | None           | None               | None                  | None                |
| `AmpFileChanges`               | None              | None           | None               | None                  | None                |
| `WindsurfWorkspaceJson`        | None              | None           | None               | None                  | None                |
| `WindsurfMirrorJson`           | None              | None           | None               | None                  | None                |
| `WindsurfCascadeProtobuf`      | None              | None           | None               | None                  | None                |
| `DevinLocalSqlite`             | None              | None           | None               | None                  | None                |
| `Uncharacterized`              | None              | None           | None               | None                  | None                |
| `OpenCodeSqliteV2`             | D/S/O/C           | O              | None               | S                     | None                |
| `OpenCodeV2SessionExport`      | None              | None           | None               | None                  | None                |

| Scope and environment              | macOS       | Linux       | Native Windows        | WSL         |
| ---------------------------------- | ----------- | ----------- | --------------------- | ----------- |
| Global model or reasoning setting  | Auto Fix    | Auto Fix    | Read attribution only | Unavailable |
| Project model or reasoning setting | Auto Fix    | Auto Fix    | Read attribution only | Unavailable |
| Session or worker setting          | Unavailable | Unavailable | Unavailable           | Unavailable |

The macOS and Linux implementation rejects unreviewed runtime overrides,
managed or system configuration, Codex profiles, untrusted workspaces,
unsupported precedence, unsupported missing files, duplicate definitions, malformed data,
files above 256 KiB, non-regular files, target or ancestor symlinks, and wrong
Unix owner or group. Apply re-resolves precedence, checks the original file
identity and bytes, writes an exclusive same-directory temporary file,
preserves mode and owner/group, syncs it, atomically replaces the target, syncs
the directory, and performs typed readback. A changed target conflicts without
retargeting. The approved missing-file exception creates only global Claude
settings for an eligible optional built-in tool. It uses exclusive creation,
safe parent directories, directory sync, and exact-byte readback.

Native Windows can resolve and store publication-time attribution. Apply remains
unavailable because the repository has no reviewed implementation and executable
tests for ACL preservation, reparse points, sharing conflicts, Windows file
identity, replacement semantics, and uncertain-write recovery. The backend does
not create a prepared review on Windows. WSL has a separate environment key and
never reads or edits the native host configuration.

Prepared changes are memory-bounded and expire after ten minutes. A crash in
`writing` becomes durable `recoveryNeeded`. Recovery accepts only the same
native agent, accepted source format, scope, physical target, and replacement
value after fresh override and managed-policy checks. A changed target or an
unprovable result stays in recovery and does not start a second write.

## Config Attribution Contracts

Attribution is publication-time metadata, not
historical session evidence. The backend stores a keyed physical target hash,
scope, observed value, physical path, selector, typed expected value, and a
keyed precedence identity only after complete control observations match the
resolved setting. This local data is not sent in analytics or diagnostics.

| Agent       | Reviewed persisted contract                                                                                                                     | Attribution decision                                                                                                                                                                                                           |
| ----------- | ----------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Claude Code | Managed settings override CLI, local, project, and user files. `ANTHROPIC_MODEL` has per-key precedence over `model`.                           | Model and reasoning use only the attributed existing user, project, or local selector. Current resolution must match its saved scope and physical target. Managed, CLI, host, or environment winners are unavailable.          |
| Codex       | CLI overrides trusted project files from repository root through CWD, then an explicitly selected profile, user config, and system config.      | Model and reasoning resolve every trusted nested project file, then require the saved scope and physical target. A selected profile, runtime override, system or managed config, or untrusted workspace is unavailable.        |
| OpenCode    | Remote, global, custom, nested direct project, `.opencode`, inline, managed file, then MDM sources merge in that order.                         | Model attribution reads the reviewed global and nested project JSON/JSONC subset, then pins Auto Fix to the saved scope and physical target. Remote, custom, inline, managed, dynamic, agent, and mode inputs are unavailable. |
| Pi          | Trusted project `.pi/settings.json` deep-merges over global settings. CLI provider/model/thinking and session-directory inputs take precedence. | Model and reasoning use the attributed existing winning project or global selector and require its saved scope and physical target. Agent-directory, CLI, or split provider/model inputs are unavailable.                      |
| Cursor      | CLI JSON, CLI permissions, MCP JSON, IDE settings, and team controls are separate contracts.                                                    | Unavailable. No accepted Cursor session source proves that one persisted CLI or IDE setting caused the observed model behavior.                                                                                                |
| Antigravity | Documented global and workspace MCP files do not define model-setting precedence for every IDE and CLI surface.                                 | Unavailable. Accepted session sources do not bind a model or setting to one physical control.                                                                                                                                  |

The official contracts reviewed are [Claude settings][claude-config-source],
[Codex config basics][codex-config-source], [OpenCode config][opencode-config-source],
[Pi settings][pi-config-source], [Cursor CLI configuration][cursor-config-source],
and [Antigravity MCP][antigravity-config-source]. These sources describe current
configuration behavior. They do not expand accepted session-source versions.

[claude-config-source]: https://docs.anthropic.com/en/docs/claude-code/settings
[codex-config-source]: https://developers.openai.com/codex/config-basic
[opencode-config-source]: https://opencode.ai/docs/config/
[pi-config-source]: https://github.com/badlogic/pi-mono/blob/b2602be77cb7b0de45dd616407fd210daa48aa75/packages/coding-agent/docs/settings.md
[cursor-config-source]: https://cursor.com/docs/cli/reference/configuration
[antigravity-config-source]: https://antigravity.google/docs/mcp/

## Savings Contracts

All nine methods have typed inputs, methods, revisions, units, and unavailable
reasons. Known zero and negative values remain known. Missing evidence,
assumptions, comparisons, rates, revisions, or durable ownership remains
unknown. Arithmetic overflow is unknown, not a saturated saving.

| Check | Method                               | Result unit                                | Current numeric eligibility                                                                                                                                                                                                                                                                                                                                  |
| ----- | ------------------------------------ | ------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| D     | Repeated context above the depth cap | Literal input tokens                       | Numeric only with an observed request total and pinned cap. Confirmed accumulation is unavailable while D verification is unavailable.                                                                                                                                                                                                                       |
| T     | Reviewed output reduction assumption | Assumed output tokens                      | Requires observed output and an explicit basis-point assumption. No default assumption exists.                                                                                                                                                                                                                                                               |
| S     | Worker model price difference        | API-equivalent USD                         | Requires exact worker tokens, reviewed alternative rates, route, pricing revision, and ownership. Missing inputs remain unknown.                                                                                                                                                                                                                             |
| M     | MCP definition exposure              | API-equivalent USD or literal input tokens | Requires attributable definition tokens and compatible-request count. Names or exposure alone are nonnumeric. Reports API-equivalent USD only when every contributing turn's model resolves in the live pricing table and the stamped pricing revision is still current; otherwise the finding still reports with literal input tokens and no dollar figure. |
| B     | Built-in definition replication      | API-equivalent USD or literal input tokens | Numeric for established catalog-backed replication counts. It is not converted into a confirmed win while B verification is unavailable. Same pricing-table and revision requirement as M applies for the API-equivalent USD figure; an unresolvable model still reports the token count.                                                                    |
| K     | Injected skill document              | API-equivalent USD or literal input tokens | Requires full document tokens and compatible-request count. Listings never qualify. Same pricing-table and revision requirement as M applies for the API-equivalent USD figure; an unresolvable model still reports the token count.                                                                                                                         |
| O     | Old-model price difference           | API-equivalent USD                         | Implemented for exact attributed Claude Code, Codex, OpenCode, and Pi replacement activity with both reviewed rates and a pricing revision. Zero and negative differences remain known.                                                                                                                                                                      |
| F     | Fast-tier price premium              | API-equivalent USD                         | Requires same-model, same-route standard and fast rates, eligible tokens, pricing revision, and ownership. Missing comparisons remain unknown.                                                                                                                                                                                                               |
| C     | Paid versus cache-read difference    | API-equivalent USD                         | Requires attributable repeated paid tokens and reviewed paid/cache rates. Raw repeated tokens alone do not establish dollars.                                                                                                                                                                                                                                |

Literal input tokens, assumed output tokens, cache-class tokens,
API-equivalent USD, and improvement counts are separate units. Aggregation adds
only values with one nonempty durable owner, no duplicate owner, and one unit.
Mixed units and unresolved overlap stay separate. Confirmed contribution rows
contain bounded derived facts, replace equal or newer facts for one owner, and
survive normal session retention. Verification transition and contribution
replacement commit together. Aggregate reads return at most 1,000 newest rows.
Durable storage keeps at most 1,000 contribution rows and 1,000 closed attempt
rows. Active attempts remain until they verify or recur.

The stored estimated-savings value is the target's pre-remediation opportunity;
it is not recent usage. The aggregate-savings read returns only exact current
cycles that remain fixed, retain a fixed verification result and verified
boundary, match their stored finding snapshot, are enabled, and are not actively snoozed.
The renderer additionally requires a visibly Passed detector and hides Savings
when no eligible cycle remains. Confirmed savings alone can use eligible
post-verification sessions.

The remediation backend lists current displayable findings even when no action
is safe. Prompt support follows the source and check limits in this document.
T and F prompt watches require fresh, complete post-boundary assessment and the
exact positive control described in the matrix before a fix can verify.
Old-model watches are stricter: only actual old or replacement model use
attributed to the same publication-time effective physical target, scope,
provider, and API can change the result. Other checks store
`verificationUnavailable`; generic absence cannot verify them. Positive-only
sources cannot verify absence, and missing later evidence remains `watching`.
A newly enrolled prompt attempt remains outside verification until its exact
opaque `ABR-` reference appears in a later captured user message. Existing
prompt-group markers remain valid for activation. The marker publication sets
the boundary and cannot prove the same attempt. Repeated Copy does not rearm
attempts or change snapshots, boundaries, or savings. Copy alone proves neither
verification nor savings; resource verification remains unavailable.

Cursor can use its explicit synthesized source-header model, but does not borrow
the previous message's model. This preserves existing basic support without
claiming native per-request completeness.

## Confirmation Ledger

These are current source-scoped decisions and their evidence limits. They do
not authorize runtime collection or claim future impossibility.
Workspace/unknown shapes retain `Unknown` rather than inheriting a native
format's contract.

| Agent                   | Named checks                                                                                        | Decision and alternatives reviewed                                                                                                                                                                                                                                       |
| ----------------------- | --------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Pi                      | M (MCP), B (built-ins), K (skills), F (fast mode)                                                   | Unsupported. Core session persistence, resource/tool configuration, and official example-extension output do not supply alternative historical inventories or speed proof. See [Pi source][pi-source].                                                                   |
| OpenCode                | M (MCP), B (built-ins), F (fast mode), T (overthinking)                                             | Unsupported. Legacy/native message schemas, CoreV2 `session_message`, and the tool registry do not save historical inventories, effective tier, or a request-resolvable effort map. See [v1.2.0 source][opencode-v1] and [CoreV2 source][opencode-core].                 |
| Antigravity             | T (overthinking), S (subagents), M (MCP), B (built-ins), K (skills), F (fast mode), C (cache churn) | Unsupported. The admitted agy 1.0.16 `user_version = 1` subset and descriptor-backed fields provide no alternative native proof for these checks. See [adapter research][antigravity-adapter].                                                                           |
| Cursor                  | T, S, M, B, K, F, C                                                                                 | Unsupported or unknown. The independent JSONL and `~/.cursor/chats/<workspace>/<session>/store.db` contract do not prove complete requests, effective model fallbacks, resource inventory, routes, or IDE configuration. See [Cursor chat research][cursor-chat-source]. |
| Claude, Codex, OpenCode | M/B/K where observed evidence exists                                                                | Approved scoped observed-resource findings only. Complete observed subset plus calls is required; no session-wide clean without full inventory. Codex exact server exposure and selected documents are covered by [rollout/protocol/skills research][codex-source].      |
| Pi                      | T, S                                                                                                | T is explicitly agent-selected policy on reviewed routes. S is limited to persisted official example-extension nested results and actual models, finding-only. See [core/session and examples/extensions/subagent][pi-source].                                           |

[opencode-v1]: https://github.com/anomalyco/opencode/tree/ffc000de8e446c63d41a2e352d119d9ff43530d0
[opencode-core]: https://github.com/anomalyco/opencode/tree/ecbc6ccac85b3e8087b6445e584318419b9e2b34
[pi-source]: https://github.com/badlogic/pi-mono/tree/b2602be77cb7b0de45dd616407fd210daa48aa75/packages/coding-agent
[codex-source]: https://github.com/openai/codex/tree/e7637306bc9246a3e42e407cb94f96b7ed345e3e
[antigravity-adapter]: https://github.com/ccusage/ccusage/blob/90e296efd1bdd25a9db07019854255284588d720/rust/adapters/antigravity/src/proto.rs
[cursor-chat-source]: https://github.com/antonvp/cursor-acp-enriched/commit/4801804543f0234bdfc266fbd53d81a6f20e9508

Research anchors include OpenCode schema/session-message and tool registry;
Pi core/session and `examples/extensions/subagent`; Codex rollout policy,
protocol models, and `ext/skills/fragments`; and ccusage
`rust/adapters/antigravity/src/proto.rs`. The [Antigravity SDK runtime schema][sdk-source] at
`52ea99480960ed02be1561f6fe57b99e7186962a` describes runtime events, not proof
that those events persist in a local session.

[sdk-source]: https://github.com/google-antigravity/antigravity-sdk-python/tree/52ea99480960ed02be1561f6fe57b99e7186962a

## Deferred Source Limits

These sources have the limits listed below. Characterized formats retain the
finding and clean-result support listed in the [Source Inventory](#source-inventory)
and [Coverage Matrix](#coverage-matrix). Legacy and uncharacterized formats remain
fail-closed. `Partial` alone does not establish an implemented finding path.

| Source                             | Current boundary                                                                                                                                                                                                                                                                                                                                                     |
| ---------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Copilot CLI                        | Event logs alone omit per-call usage and loaded inventories that the official schema marks transient. The accepted v1 event and schema-v7 request-store bundle separately supports S/O results. Other model/effort, skill, and subagent-configuration fields do not establish a complete request-level check contract. Configuration alone is not actual delegation. |
| Copilot IDE                        | Chat JSON can carry model names, but model/time and other check facts are uncharacterized. CLI contracts do not apply to IDE storage.                                                                                                                                                                                                                                |
| Cline                              | The characterized messages-contract-v1 bundle supports bounded delegated and model findings only. Calls and model names do not prove paired timing, historical resources, or other clean facts; legacy sources fail closed.                                                                                                                                          |
| Kiro canonical and chat            | Separate source shapes; resource definitions, exposure, calls, models, timing, and settings lack characterized detector-grade semantics. The fallback does not inherit canonical coverage.                                                                                                                                                                           |
| Amp thread                         | The characterized thread JSON supports bounded depth and model findings only. Saved routing modes do not prove actual model effort or speed; resources, timing, and accounting remain uncharacterized.                                                                                                                                                               |
| Amp file changes                   | Not a conversation session. No check can use file-change records as request, model, or inventory proof.                                                                                                                                                                                                                                                              |
| Windsurf workspace and mirror JSON | Calls and model names can exist, but complete resource, timing, control, and accounting semantics remain uncharacterized.                                                                                                                                                                                                                                            |
| Windsurf protobuf                  | Discovery recognizes Cascade paths; no bounded protobuf session parser or supported field contract exists.                                                                                                                                                                                                                                                           |
| Generic fallback                   | No native source contract. Recognized-looking JSON does not authorize detector-grade evidence or clean.                                                                                                                                                                                                                                                              |

## Coverage Promotion Rule

Change an entry to `Assessable` only when it can support both findings and clean
results. It needs all of these conditions:

- The accepted source shape is explicit through a schema, header, or pinned
  producer commit and synthetic fixtures. Record a release range when known.
- The reader emits every fact required for both a finding and a clean result.
- Missing, malformed, truncated, capped, or unknown records produce partial or unavailable evidence.
- Positive, negative, and incomplete synthetic fixtures exist.
- Full and resumed reads produce equivalent evidence where resume is supported.
- The model and provider policy is reviewed where the check needs policy.
- The implementation does not use current configuration as historical session evidence.

If a source cannot meet these conditions, keep the supported scope as `Partial`,
`Unsupported`, or `Unknown`. Do not convert missing evidence into a clean
result. Record the source limit and any separate source used for an advisory
assessment. The confirmation ledger records current reviewed decisions.

## Test Coverage

- Agent characterization suites in `crates/antiburn-local/tests/` cover native
  records, missing facts, scoped resources, provider controls, and malformed input.
- `resume_parity.rs`, `evidence_replay_parity.rs`, and `turn_row_replay_parity.rs`
  cover supported resume and persisted-row paths. Desktop
  `analysis/tests/claude_parent_child.rs` and scan tests cover Claude sidecar joins
  and change detection.

The matrix is manually reviewed; the inventory test does not generate or prove
every cell. Unknown changed evidence-bearing shapes must make affected evidence
partial or unavailable, not pass through a generic reader as clean.
