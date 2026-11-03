# Session Parsing Coverage

This document records how Antiburn discovers and parses local session sources.
It covers source identity, framing, companion data, normalized facts, and
provider-route extraction. See [`check-coverage.md`](check-coverage.md) for YAML
result limits and the nine native detectors' capability and reachability gates.

This is a living contract. A discovered path does not prove that its contents
are understood. A parsed field can support a scoped result without proving full
historical coverage.

## Burn Check source facts

The shared `NativeEvidenceBatch` carries ordered user messages, completed public
assistant text, explicit tool calls, tool results, and supported response cycles.
It also carries native activity, recorded selection state, explicit removals, and
typed unsafe admission failures.
Native Claude, Codex, and Pi delivery uses bounded JSONL pages. OpenCode V1 uses
bounded SQLite journal pages; V2 uses its verified local event capture. Each route
retains delivery state independently of legacy metric indexing. Usable partial
content remains eligible only when the source proves identity, order, and
main-session ownership. Missing fields and request-history limits differ from
unsafe sequence or identity gaps. Metrics and private turn-content authority
remain separate.

Cursor Desktop uses a separate ordered snapshot route for tool results and their
user-slot context. It does not inherit the five live routes' public-message,
call, response-cycle, or host-session events. See
[Cursor Desktop YAML tool evidence](#cursor-desktop-yaml-tool-evidence).

Capture health is separate from historical completeness. First attachment and
recovery can leave earlier history incomplete without proving that an event was
lost. A working capture does not show an alert for that older limit. Current
connection failures remain actionable for active sessions and unfinished checks.
An expired capture or saved error alone does not make an inactive session current.
An unapplied detach publication can still need consumption, but does not by
itself justify a current capture alert. Gap consumption and evidence invalidation
remain active even when the alert is not relevant.
Its missing evidence stays unknown. File-backed agents use watched appends
and bounded suffix reads, not an OpenCode service connection.
Stored OpenCode V2 metrics do not replace or invalidate live-capture evidence.
Only the verified capture path publishes live updates and corrections.

For YAML Burn Checks, a user message means accepted content in the user slot. A
human, extension, or API can supply it. Claude meta, synthetic, plan, and explicit
non-user-origin text stays outside request history. Safe text in mixed image or
filtered-text records remains usable with a partial-content limit. SDK text does
not gain human approval. OpenCode text parts marked synthetic are
excluded from request history in snapshot and journal reads. Mixed messages keep
their real user text; a generated-only message or replacement with no real request
leaves history partial. Tool-result envelopes, assistant text, and tool output do
not become user messages. Pi user-slot messages are supported under the same
main-session, prefix, order, and completeness rules as the other accepted sources.
`UserMessageOrigin` identifies the source field pattern, not the sender. These
rules do not relax the existing detectors' human-text or approval proofs below.
Stored V2 text keeps its native user role and accepted-user-slot origin. Exact
remediation markers can use that text, but it does not prove human approval.

The local sink freezes `ToolResultContext` for results and `NativeEventContext`
for public messages and cycles with their available user-message prefix and
main-session admission. Resume retains these facts. Desktop publication retains
each event and its context in current rows and the durable outbox.
Native publications also freeze explicit session selection separately from the
requester. Corrections retain the original selection, including unknown values.
Older publications without this metadata remain unknown. V2 selection belongs
to its capture epoch and claim fence; recovery cannot reuse selection from a lost claim.
`StorePublishedEvidenceReader` converts them to core source events; runtime reads
use the exact stored environment and session CWD, with target filtering before
the page limit. Old payloads without context remain unavailable. Discovery uses
`AgentExplorer`, `Explorers`, and `SessionSource`;
`SessionReader` parses file sources; native OpenCode journal delivery uses the
local page reader before committed desktop publication. Delivery
cursors remain separate from native source order and engine positions. A watcher
notification is not a result event.

Quality gaps preserve accepted assessments when their saved input is unchanged.
Continuity resets cancel pending work and clear the affected selection window.
A V2 capture gap also invalidates unverified current user, assistant, call, and
cycle bindings. Their stored history remains, but a new baseline or replay cannot
restore their validity. Tool results use separate exact reconciliation. An unsafe source update invalidates that
branch's native assessments and pending work and resets its window. Accepted idle
observations retain their recorded boundary, not a claim about current source
health. The unsafe source cannot enroll new idle work.

Conflicting native IDs and other proven admission failures affect public users,
assistant text, calls, results, and cycles, including evidence from earlier
pages. The failure remains separate from partial history through cursor and
snapshot restore. Source publication blocks affected dispatch and acceptance
before selector consumption; a later window cannot reuse unsafe facts. This
does not erase incurred usage or permit an unknown request to retry. Ordinary
clipped history and cache limits alone do not revoke otherwise usable evidence.
An identity-summary match alone does not prove a conflict. A proven exact Claude
replay does not create another public event or revoke earlier evidence.
Pi's exact retained request IDs still prove conflicts after its general ID cache
fills. Proven Codex paginated ordinal regressions report unsafe order rather than
only clearing the current cycle. Exact Codex terminal replay emits no new activity
when the retained original or latest terminal digest proves it. Each corrected
entry keeps those two digests, not every intermediate correction.

A publisher gap that cannot prove membership, main-session scope, or ordering
is an unsafe update. It does not use the quality-only gap path.

Live Burn Checks attach to supported active native main sessions when definitions
are enabled and configured. New scopes and cold source bindings use the existing
180-second activity window and require an enabled live activation for the exact
native environment and stored session CWD. Unknown or empty CWD, remote, WSL,
known fork, known subagent, and historical-only sources do not create new live
work. Saved scopes still reconcile definitions and recover admitted work.
Compatible unfinished work drains after activity expires, including interrupted
cold claims before their first cursor. A consumption-time activity cutoff does
not discard delayed or restarted admitted publications.

After the initial source receipt and the session's publications are consumed,
fresh attachment reads at most 40 current terminal tool results within 4 MiB in
native order. Authored filters and conditions select only the latest ready
candidate, including for checks without a rolling window. The attachment marker,
selector state, observations, dependencies, and optional candidate commit
together. Empty receipts get a marker. Startup coverage is partial; this bounded
slice does not prove whole-history membership. Unchanged restart does not repeat
startup, and existing observed work is not reseeded. Other attached branches can
run while one branch attaches.

Normal live selection admits new first-terminal results after attachment. Other
native message kinds and cycles use a separate future-only source baseline and
immutable first-eligible publication. Historical messages do not enter live
windows because of a correction, restart, or parser upgrade. A later correction
keeps its earlier logical order and frozen context; new delivery or discovery
generations do not turn it into a new attempt. Corrections to seeded historical
results still invalidate stale evidence without another model attempt. Later
non-window live candidates remain separate. Historical assessment requires
explicit isolated replay.

Native source delivery has reserved capacity independent of metric indexing.
Partial first-page file publication retains the initialized startup position and
EOF baseline. Retry and restart keep that boundary, so later appends remain live
and pending output is not lost when the startup suffix moves.
Partial first-page journal publication pins the original recent window and its
result/public baselines. Retry validates that window before publishing the rest;
appended messages remain future live work. Startup seeding waits for the whole
initial journal page to commit.
The worker alternates file and journal turns. Productive continuation is due
immediately. Durable monotonic claim order rotates productive sources without
requiring a clock tick and survives restart. Active sources and changed cursors
get a one-second poll; idle attached dormant sources use a fair 30-second resume
sweep without requiring a discovery generation change. Normal appends do not
revoke admitted work solely because the discovery fingerprint changes.
Live readers restore bounded state and read suffixes with bounded validation
windows. They do not reread the historical prefix on each turn. These validation
windows are not a full-history integrity proof.

The OpenCode journal producer is pinned to
[1.18.35](https://github.com/anomalyco/opencode/tree/53d1eabb61e21162157817bf677da0a4ad3332e3).
Its mutable `part` table is a projection, not a history of every revision.
The accepted shape is fixture-characterized; it is not live certification or a
claim about every installed or historical producer version.

The live reader accepts known `message.removed.1` markers during cold attachment
and later delivery with exact session and message IDs. A deletion removes retained
request text from future contexts and keeps request history partial. Earlier
frozen contexts stay unchanged. Deleted message IDs cannot restore request text
or establish tool ownership. Their tombstones share the live request cache's
128-record and 64 KiB limits and survive ordinary context eviction and restart.
If deletion proof alone exceeds those limits, later ownership stays unavailable.
The desktop removes the deleted message's exact stored user, assistant, call, and
result bindings in native event order, including bindings outside the parser
cache. Deleted bindings cannot be restored by later updates. Unrelated new
results remain eligible; deletion alone does not make their order unsafe.
File and V1 removal expansion commits bounded progress and repeats the unchanged
source cursor until deletion completes. V2 expansion that exceeds one captured
event's budget rolls back the batch and capture sequence. An unbound removal
reports a coverage gap. Page absence never removes evidence. Strict historical
replay still does not support whole-message deletion.

The current runtime admits native session evidence under five live routes and
the separate [Cursor Desktop snapshot route](#cursor-desktop-yaml-tool-evidence).
See the [Source Matrix](#source-matrix) and
[Source Inventory](check-coverage.md#source-inventory). Those contracts do not
establish all installed producer versions, historical release ranges, remote/WSL
paths, or child sessions. Executable version output alone does not prove input
schema, finality, order, or retained-prefix completeness.

Non-final result updates normalize to `ObservationKind::Other`; final results use
`ObservationKind::ToolResult`. Other admitted events use `UserMessage`,
`AssistantMessage`, `ToolCall`, `TurnEnd`, `SessionStart`, or `SessionIdle`.
A first eligible event can trigger once. Later corrections retain its logical
order and frozen prefix, while payload/context changes update the evidence
version even without a producer revision change.
Replayed older OpenCode mutations do not replace or republish current results.
Similar arguments or output do not deduplicate distinct attempts. Known record,
branch, or partial-write gaps mark affected context partial. Unknown result
identity, session scope, and ordering remain unsafe to promote.

`ToolResultAdmission` separates usable partial content from unsafe identity,
order, or main-session ownership. Missing frozen context is unavailable. The
desktop publishes these unsafe or unavailable records as source gaps, not model
review targets. A complete payload alone cannot override that gate.

The legacy local sink retains a recent window of at most
4,096 tool-result observations within 4 MiB of result state. It can evict older
entries to retain new results; the omission marks membership partial. Native live
delivery instead continues from committed file or journal cursors. Neither
bounded retained state nor a single journal page proves exhaustive history or
absence of a corrective action.

The full-reader generic evidence collection separately retains at most 4,096
events within 4 MiB. Its user/public prefix retains the latest 256 messages
within 1 MiB. Collection overflow, invalid saved evidence, and known gaps leave
that history incomplete. These retained snapshots do not become live trigger
baselines or prove transport continuity.

YAML inputs expose available `session.id`, `agent.name`, `repo.root`, and
user-slot `session.requests[].text`. `session.requests_complete` and
`session.source_coverage` describe known history limits. `message.quality`
identifies missing arguments, result content, or source completeness. A missing
image is not analyzed, but recorded text and tool results can still be reviewed.
Unavailable tool categories stay absent. An unfiltered check can use the remaining
fields; a category filter cannot match absent evidence as `other`. A recorded
tool name can normalize to native `Other` when no more specific class applies.
Check-facing tool categories come from reviewed native tool identities, separately
from the metric categories used by deterministic checks. The compiler accepts
all nine categories. Sources emit `write`, `delegate`, `skill`, and `mcp` only
when their accepted native identity proves that category; a collapsed `other`
metric value or result text cannot establish it. Older saved publications keep
their original five-category evidence without gaining new requester/path facts.

### Captured requester and operation facts

The accepted shapes in the
[native fact fixtures](../crates/antiburn-local/tests/fixtures/native_tool_facts/README.md)
bind requester model, recorded mode evidence, check category, and operation
targets to exact native calls/results. Those facts remain in immutable
publication JSON and frozen candidates, not current metric or Settings rows.

- Claude's accepted 2.1.278 log shape supplies `message.model` and effective
  `message.usage.speed` (`fast` or `standard`). It does not prove the provider
  route, so a complete `provider/model` key remains unavailable. Top-level speed
  settings do not replace response speed.
- Codex 0.160.1 binds the recorded provider and turn model through the exact call
  ID. An owned `thread_settings_applied` snapshot with matching provider/model
  can supply a recorded requested-tier preference at the next request context:
  `priority` means fast and `default` means standard selection. Missing, unknown,
  or mismatched preferences stay unavailable. Later settings do not relabel old
  calls. This is a preference hint, not proof of a sent or served tier. A
  `turn_context.service_tier` field is not part of the pinned producer shape.
- OpenCode V1 1.18.35 requires exact retained assistant-linked `step-start` proof
  before binding requester provider/model. User-shell and direct-subtask records
  do not prove a requesting model. V1 paths remain unavailable because a custom
  tool can replace a built-in name without recording its implementation. V2
  2.0.32 binds requester provider/model
  from `Step.Started` through `assistantMessageID`. Reviewed exact OpenAI
  Fast/Ultrafast catalog IDs provide selected fast-mode evidence. This does not
  prove the tier actually served. Arbitrary variants and display labels do not
  establish mode. V2 tool records do not supply an exact operation CWD.
- Pi 0.84.4 binds the actual assistant provider/model to its direct tool result.
  A later `model_change` does not replace past requester attribution. A recorded
  assistant model ID ending in `fast` or `ultrafast`, separated from a nonempty
  prefix by `-`, `_`, or one ASCII space, supplies a low-confidence model-name
  hint. Matching ignores ASCII case but does not trim text or match interior
  tokens. This does not verify a built-in Pi fast route or actual service tier.
  Other model IDs remain unknown, not standard. Thinking settings do not supply
  mode evidence. Pi paths remain unavailable because extensions can
  replace built-in names and run on another host. This also excludes paths with
  Pi-specific `@` and Unicode-space preprocessing.

Frozen assessment input can identify these evidence kinds through
`message.quality.fast_mode_evidence`: `reported-speed`,
`requested-tier-preference`, `selected-catalog-model`, or `model-id-suffix-hint`.
The qualifier accompanies a known mode Boolean; it is not an accuracy score.
Low-confidence evidence can support a limited trial suggestion without proving
actual speed or savings. These mappings do not change the deterministic native
Fast mode overuse check.

Paths come only from supported native file-operation fields or complete Codex
file-change targets and matching turn context. The reader uses recorded POSIX
CWD and lexical paths with the exact repository binding. It does not parse shell
or patch text, read current files, or resolve symlinks. Unsupported paths,
incomplete target sets, Windows/URI paths, and parent traversal stay unavailable.
A complete supported set with only outside-repository targets becomes a known
empty repo-relative list. A target that is the repository root itself remains
unavailable in this path subset. Retained path sets have at most 256 entries and
64 KiB, with 4 KiB per identity/path string.

### Public messages and live event eligibility

The [file public-event fixtures](../crates/antiburn-local/tests/fixtures/public_events/README.md)
and [OpenCode public-event fixtures](../crates/antiburn-local/tests/fixtures/opencode_public_events/README.md)
define these accepted boundaries:

- Claude Code CLI 2.1.278 and 2.1.309 accept user-slot text and complete explicit `tool_use`
  objects under its existing UUID, parent, and root rules. Split calls retain
  native block ordinals. Duplicate call IDs in one record are unsafe. Neither
  assistant records nor `turn_duration` prove final public assistant text or a
  complete response cycle. Both remain unavailable. A validated CLI header can
  anchor a bounded cold suffix without proving the omitted parent chain. Later
  record links remain exact and request history stays partial. A reviewed
  `system/compact_boundary` with null `parentUuid` and an exact prior
  `logicalParentUuid` preserves live delivery without restoring human-history
  proof. The characterized 2.1.278-to-2.1.309 `system/bridge_status` seam keeps
  the exact retained parent chain. It advances native order without adding user
  text, activity, or human authority. Other versions, entrypoints, and broken
  ancestry remain outside this accepted subset. Cowork 2.1.275 metrics do not
  establish CLI live eligibility.
- Codex 0.160.1 at `d27764b82f7118f674371e6d6e76271d9d606edb` accepts its retained
  user proof and characterized explicit calls. Custom call input remains exact
  text, even when it looks like JSON. Public assistant text requires a completed
  `AgentMessage` and its matching raw projection with the same item ID, turn, and
  text. A bare row, UI echo, delta, or phase does not prove completion. Its
  response cycles require the interval proof below. The pinned rollout policy
  persists `WebSearchEnd` only in legacy history. It does not persist
  `WebSearchBegin` or collaboration lifecycle sidebands; protocol-shaped fixtures
  do not establish cycle support for them. Independent legacy evidence remains
  usable. A completed exec item retains its native result identity. Only an exact
  same-turn wrapper output closes the separate outer call; it adds no duplicate
  wrapper result. Recognized inert top-level metadata can invalidate the active
  cycle without making later request history incomplete. Unknown or unhandled
  evidence-bearing records still leave a history gap. Exact `mcp__server__tool`
  names, or an unambiguous MCP namespace plus tool name, supply category `mcp`;
  they do not prove server trust.
- Pi 0.84.4 at `b79e4cc834970cca69daebffab7df1da7d1e52c4` requires the accepted
  V3 root and lineage. Complete explicit `toolCall` objects supply calls. Only
  characterized persisted `stop` or `toolUse` stops supply public assistant text.
  Error, aborted, length, missing, and unknown stops do not. Text parts preserve
  ordinal holes for private blocks and calls. Reviewed linear `custom`, text-only
  `custom_message`, and `compaction` rows retain later live admission. Compaction
  and custom-message context do not restore a complete request history. Exact
  parent and root checks still apply. Pi supplies no response cycles.
  The characterized inert Antiburn `custom_message` with
  `customType: antiburn-session-checks` can preserve later native delivery. It
  does not become a user request, public user event, native activity, or complete
  human history. Incomplete rows or rows with native analysis signals still fail
  the continuation gate.
- OpenCode V1 1.18.35 at `53d1eabb61e21162157817bf677da0a4ad3332e3` accepts
  non-synthetic user text and revisions of that identity. Full running calls
  require an observed assistant lifecycle. Public text requires final text-end
  values and a completed assistant header. Reasoning, synthetic/ignored parts,
  summaries, and error headers are excluded. Cold current-row lookup can prove
  result ownership, not a public call or completion boundary. V1 supplies no
  response cycles.
- OpenCode V2 v2.0.32 at `843a6c514e606b436a7fe713422ace0a93ad3d61` supplies user
  input at inbox delivery, not enqueue. Full calls require exact native names and
  step request joins. Final retained text slots become public only at successful
  `step.ended`, with ordinal holes preserved. Streams and reasoning never become
  public text. A reused step withdraws its earlier public completion until the
  new attempt proves completion; existing calls keep their frozen requester.
  Exact native `edit` and `write` names supply their check categories only with
  explicit local-execution proof. A terminal provider-execution upgrade corrects
  the earlier call and its cycle member without moving first eligibility or
  replacing frozen context. Missing terminal content still withholds a cycle.
  Response cycles require a verified baseline and a later execution start.

These finite accepted shapes do not certify all installed releases or old
histories. The host consumes each ordered batch once; result-only API views are
not a second publication stream. The same public message normalizer serves
standalone events and cycle members. Private reasoning records can supply
activity and generating-request proof, but their text never enters public
messages or cycle content.

New message kinds retain their native first-eligible position separately from
the latest mutation. The host verifies the source claim against its frozen
baseline and activation before storing first-eligible publication metadata.
Missing first eligibility cannot authorize a new-kind live trigger. File reads
and V1 journal catch-up can retain earlier facts as context without making them
live. A fully observed Codex start before the file baseline can support a later
live terminal; an incomplete historical interval cannot.

Part removal can withdraw an exact call or revise an assistant text projection.
Whole-message tombstones prevent later updates from restoring deleted evidence.
Cycles bind every member's exact current identity, kind, revision, publication
sequence, and payload hash. A correction or removal can invalidate a cycle even
when its standalone member never matched an authored message filter. Missing or
stale member bindings leave the cycle unavailable.

`NativeEvidenceBatch.history_complete` describes retained evidence history, not
a complete membership snapshot or transport continuity. Missing old request
history and public-cache eviction do not by themselves prove a capture outage or
delete absent rows. New user/call/activity batches cannot restore omitted history
or clear an existing result window. Their contexts retain the source's history
limit.

### Recorded current selection

`session.model` and `session.fast_mode` describe separately observed selection
state where supplied. Host start/idle contexts use explicit Codex
`thread_settings_applied`, Pi `model_change`, or V2 `session.model.selected`
facts. Claude and V1 have no selection proof in this subset. Catch-up can apply
proven historical selection as state, not as a live event. Selection changes do
not supply native activity.

Current selection never fills a missing generator or requester on an earlier
message, call, result, or cycle. User messages have no generating model or mode.
Model and mode availability remain independent: an unavailable `provider/model`
key does not erase proven mode evidence. The requester qualifiers above keep
their source-specific meaning; none proves a served tier or measured savings.

### Saved-state revisions and conversion

Parser revision is 67, analyzer revision is 25, evidence schema revision is 22,
and resume snapshot revision is 25. Native file cursor and OpenCode V1 live
journal revisions are both 5. Compatible parser-63, parser-64, parser-65, and parser-66
cursor/state-5 receipts retain committed progress. The named parser-62 predecessor
uses claim-fenced conversion. Batch resume snapshots require the current parser.
Unknown parser or cursor revisions fail closed; they do not authorize a rewind
or resend.

The regular file loader accepts parser 62 with cursor 4, or parser 63, 64, 65, 66, or 67
with cursor 5. V1 live journal validation accepts parser 62 with live state 4
through its named converter, or parser 63, 64, 65, 66, or 67 with live state 5 and a
valid public baseline. Source identity, owner, sequence, and bounded source
samples still require validation. A matching layout number alone does not admit
another parser pair. The saved parser revision and cursor payload advance only
when page publication commits.
Parser-66 journal cold retries validate their pinned window with the original
parser-number hash binding. Upgrade does not change that window or replay it.

Separate named converters accept parser 64 with file cursor 1 or V1 live state 2
only from producer commit
`143d4b05681938cb362e828ecc27d2e082638176`, not other old layouts or parser
combinations. The
[synthetic old-encoder fixtures](../crates/antiburn-local/tests/fixtures/native_receipt_compatibility/README.md)
pin the accepted shape; they do not establish a published release range.

File conversion validates the retained source identity and hash anchors before
freezing the current EOF for new public event kinds. V1 conversion requires
stored and encoded parser 64 and validates the session, native root owner,
sequence, origin, and boundary before freezing the current journal frontier.
Both retain the consumed position, original result baseline, pending result
joins, and existing history limits. Unread results can continue; older public
messages, calls, and cycles do not become new work. Missing attribution and
selection proof remain unavailable. Pre-65 Claude request-origin limits still
apply.

Conversion becomes ready only after a validated layout-5 page commits under the
existing source and claim fences. Bounded publication retries retain the consumed
position and frozen public frontier across restart. Conversion does not reset
paid attempts or rewrite accepted inputs, assessments, cooldowns, or usage.
Malformed or unsupported receipts remain blocked without rewind or reseeding;
parser numbers alone do not prove wire compatibility. Clearing the index or
resetting paid attempts is not a supported recovery step.

Pre-65 Claude cursors retain byte progress and bindings but drop old request text
and pending user events whose origin they cannot prove. Pending contexts become
partial. Accepted inputs and publication hashes stay unchanged.
An old suffix cursor that has framed bytes but observed no records can retain
its valid header proof and admit the first retained record as a partial seam.
This repair requires a pristine root and no admission failure; it never restores
an invalid root or erases a known source failure.

Each native Claude source binding gets a durable parser-65 request-origin
boundary before it publishes usable new evidence. Dispatch and acceptance wait
until the live activation consumes that boundary. The reset clears unfinished
windows and pre-boundary candidates without replaying source records or changing
accepted results, cooldowns, attempts, or incurred usage. Startup selection also
excludes pre-boundary publications; a newer selector cursor cannot make their
older request context safe. The boundary binds the exact session incarnation,
source, and project directory and survives restart.
An exact proven parser-65 or parser-66 cursor-5 predecessor keeps its validated
request context when it advances to parser 67. The historical parser-65 boundary name does not
change, and the upgrade does not create another origin reset.
Transient read errors do not revoke that request-origin proof. Current evidence
and idle-continuity checks still apply; new attachments still require a ready source.

The named file V4 binary/JSON converters retain committed byte progress, framing,
partial tails, unresolved joins, pending result payloads, and the original result
baseline. Request prefixes remain subject to the Claude origin rule above.
New event kinds start at the old anchor's frozen
EOF. V4 state lacks an exact selection position, so converted selection remains
unavailable until another explicit fact arrives. V1's named parser-62/revision-4
conversion retains result progress, contexts, joins, and baseline while freezing
the separate public baseline at the validated current frontier. Each converted
cursor commits with its page, not before it.

Result-only readers, terminal lookup, and startup seeding do not admit new kinds.
Existing result payload encoding and first-terminal meanings remain unchanged.
Snapshot recovery does not reset selector checkpoints, startup/applied cursors,
attempts, cooldowns, usage, or accepted status. An incompatible snapshot needs
source recovery; it does not make historical messages new work. Deterministic
metric categories stay separate from these event contracts.

Decision-input fitting runs after these parsed facts and frozen source bindings.
Its omission report describes an author-approved reduction of provider input,
not a parser gap or a change to `message.quality`, `session.requests_complete`,
or `session.source_coverage`. It cannot recover source omissions or widen any
accepted source/version contract. See
[decision-input fitting limits](check-coverage.md#decision-input-fitting-limits)
for result eligibility and local no-fit behavior.

For source-specific claims, identify the actual producer/version, main-session
and branch identity, record/call links, finality, native order, and user prefix.
Keep missing, duplicate, late, changed, removed, and corrected records visible.
Retained-history replay cannot prove live activation or watcher behavior. A
source-specific result supports only its admitted records and execution path;
synthetic fixtures do not certify arbitrary producer histories. See
[authoring](burn-check/authoring.md#supported-subset) for runtime admission and
[check coverage](check-coverage.md#burn-check-definitions) for result limits.

### Native OpenCode metadata admission

Native OpenCode provider-DB discovery admits session metadata before the full
strict source hash completes. A session-scoped bounded admission revision binds
the database path and available file identity to session metadata, recent
message/part metadata, and available journal sequence metadata. The revision
samples at most 256 sessions in the root cluster and the latest eight message
and eight part rows per selected session. It excludes payload bodies and does
not detect every old payload edit or change outside those bounds.

A changed admission revision clears the old strict fingerprint, advances the
source generation, and makes old metric analysis stale. Pending fingerprint
state persists across restart. Legacy evidence claims wait for the strict hash;
native journal delivery and its receipts remain independent of that wait.
Hash recovery reads one consistent SQLite snapshot and checks the scoped
revision before and after the read and before publication. Unrelated database
writes or WAL checkpoints do not by themselves invalidate the scoped revision.
Stale or failed work retries with bounded backoff. Interrupted work becomes
eligible again after restart. A changed admission revision clears prior retry
backoff and becomes eligible immediately. Successful hashes schedule another
reconciliation after five minutes for sessions active within the last hour, one
hour for sessions active within the last day, or one day for older sessions,
subject to scheduled scanning and worker availability. Missing database files
and sessions are quarantined until discovery restores their source path. Other
read or snapshot failures keep their bounded retry schedule. This reconciliation
can detect current payload changes that bounded admission misses. It does not
prove original historical retention or give the live journal reader full-prefix
integrity validation.

For message/part activity, discovery uses the latest valid normalized
`time_created` or `time_updated` value across the selected rows. This lets an
update advance activity even when creation time is present. The candidate
database query still compares its source timestamps in milliseconds; this
normalization does not change that query's units or widen producer support.

### Native file delivery

`analysis::file_live::read_native_file_page` accepts native main-session
`ClaudeJsonl`, `CodexRolloutJsonl`, and `PiV3Jsonl` files under their pinned shapes.
Cold attachment reads a header within 64 KiB and starts in the latest 1 MiB.
Each turn reads at most 256 KiB of new bytes. Starting inside a record discards
that partial leading record and marks history partial. An unterminated final
record remains pending across cursor persistence and becomes live when its
newline arrives. A pending tail is not by itself a permanent malformed-source gap.

The cursor retains framing, bounded native joins, requests, separate result and
public-event EOF baselines, and continuation. A complete record whose evidence
exceeds one output page can leave pending events in the cursor; continuation can
therefore return evidence with zero new source bytes. Empty source-byte pages are
not necessarily end-of-input. Page absence is not a removal.

`baseline_reached` identifies completion of the frozen recent startup range.
Retain only the latest seed window until that flag is true, then assess it once.
It is separate from `has_more`: later appends must not force startup to chase a
moving EOF or assess an older first-page window. A partial record at the baseline
can complete later as live work.

Live source validation checks available file identity, truncation, bounded head/tail
anchors, and the exact page read. Appends are allowed. The reader does not audit
old bytes outside those windows. Native result/request byte ranges are distinct
from the full JSONL readers' physical record indices; skipped-prefix local indices
must never be presented as whole-file positions. These APIs do not add remote,
WSL, child-session, or arbitrary historical-version support.

Typed file errors distinguish unsupported input, invalid cursor, source change,
unavailability, cancellation, and read failure. Invalid cursors and detected
source changes do not authorize rewind. Cancellation publishes no replacement
cursor; transient reads retain state for bounded retry.

Native file state retains at most 4,096 unresolved call payloads within an 8 MiB
call-data budget. Completed calls release their payloads. Non-evicting 128 KiB
identity summaries retain rejected, ambiguous, and retired identities in each
tracked category across restart. A possible summary match can withhold a fact
or join; it cannot establish a new call binding. Filling the unresolved budget
does not evict an older unresolved call. Missing joins retain the source's
existing partial-content or unsafe-identity policy.

Pi retains at most 4,096 exact IDs within 1 MiB of ID bytes without eviction.
Its last record and bounded unresolved call owners also provide exact identity
proof. Claude reuses retained UUIDs and canonical record fingerprints. A possible
summary match alone cannot prove a source admission failure; an uncertain new
fact stays unavailable with a history limit. Proven exact Claude replays emit
neither public work nor activity. Current cursors retain these proofs; named V4
conversion seeds proof only from exact retained state.

Request context retains at most 256 messages within 1 MiB of text. The framer
retains at most 8 MiB for one record and drains larger records across finite
reads. Each fact must fit its source and page bounds. Combined same-record
output has a 4,096-event/20 MiB bound, with one count slot and 8 KiB reserved for
native activity. A delivery page retains at most 256 total events within 12 MiB,
including envelopes, contexts, and copied
cycles. Page limits defer admitted output in the cursor. An oversized fact adds
a history gap; `skipped_results` also counts new evidence lost to these bounds.
The EOF baselines stay fixed across restart and later appends.
Known admission failure has reserved output capacity. The page's persistent
failure field fences its receipt while the ordered failure fact is deferred.
A complete malformed or oversized record makes later file admission unavailable
for that cursor. An unfinished tail alone does not remove admission.

An older cursor already marked unsafe remains unavailable after a parser update.
A new parser does not erase an earlier admission failure or rewind the source to
try again. This differs from retaining future eligibility across a newly observed,
reviewed compaction boundary.

### OpenCode journal delivery

`read_opencode_live_tool_result_page` reads one SQLite transaction snapshot.
Cold attachment reads at most 4,096 recent events and 8 MiB of event payload and
retains at most the latest 40 terminal identities within a 4 MiB output bound.
It does not traverse older windows. Its materialized frontier is the lower of
the advertised sequence and latest persisted event; a reserved future sequence
does not extend the boundary beyond available events. Cold request context is
always partial. Recent headers/text supply request context; bounded assistant
projection lookups can establish ownership, not reconstruct old user text.
Cold attachment pins at most 40 selected identities within the existing
128-entry/128 KiB identity budget until the window ends. Unselected identities
are evicted first. These pins leave fewer slots for other pending identities;
the byte bound can reduce the retained count further.
The desktop claim query enforces exact enabled live activation and active main
cold admission. The parser itself has no host clock. Compatible admitted work
continues after activity expires; detected source-binding changes require new
admission. A successful standalone page read does not establish live enrollment.

The versioned live cursor retains request state, bounded pending/terminal and
public identities, frozen context references, separate result/public baselines,
and the consumed frontier. Warm reads use
the required unique `event_aggregate_seq_idx` on `(aggregate_id, seq)` and read
only the new suffix. A missing supported index makes the route unavailable; it
does not authorize a full-table scan. Input/output limits can require continuation. Unknown
terminal-only identities after attachment can be old corrections and remain
unsafe; new calls need a retained pending/running update before terminal. Known
corrections preserve first-terminal order and context. Evicted context stays
unavailable rather than being reconstructed from old history.
If cold publication stops early, an optional `cold_replay` cursor field stores
the frozen window's output digest. Retry rereads only that original bounded
window and uses its saved ownership lookups, including absent projections. It
checks the digest and finishes the window before reading appended events. A
changed window fails closed instead of shifting the baseline. This receipt does
not grant report readiness or startup seeding until the initial page commits.
An exact removal without a retained call binding records a coverage gap; later
independent results can remain usable. An explicit message-role change invalidates
retained tool ownership even when an older frozen context exists.

Live validation checks session, owner, parser/state revision, frontier rollback,
and bounded origin/current-boundary samples. It does not hash the full consumed
prefix or detect every interior historical rewrite. A detected boundary or
identity change and an invalid cursor require source recovery. The desktop does
not automatically rewind and replay old work.

`read_opencode_tool_result_page` is the strict audit/replay reader. It retains
full consumed-prefix hash validation and reconstruction, permits later appends,
and rejects consumed rewrites, removals, resets, owner changes, and live cursors.
Its work scales with prior journal history. Each strict page covers at most
4,096 native positions and 4 MiB of serialized results. This stronger historical
audit contract must not be attributed to the incremental live reader.

The desktop commits current evidence rows, immutable outbox events, native member
bindings, activity, selection state, the cursor, baselines, diagnostics, and
continuation state in one transaction. A
cursor-write failure rolls back the page. Publication rechecks the claim fence,
lease, prior cursor, session incarnation, source identity, exact CWD, parser
revision, and known fork admission. Stale or repeated claims cannot republish a
page. Publication-time project and incarnation filters prevent old unassigned
or other-project events from moving into a newly assigned project.

Page membership and source-history coverage are separate. Journal pages never
infer removals from absent identities; only explicit bound removals can withdraw
evidence. Page limits alone do not create a coverage gap. Bounded cold
requests remain partial. A real history gap remains partial across later pages
and restart until a new admitted source binding is established. New gap publications precede usable
observations, so independent partial work can still reach assessment.

The reserved native worker alternates bounded file and journal turns.
Productive journal continuation is due immediately, including after an advancing
empty page. A durable fence order rotates productive sessions fairly at one clock
value and across restart. Completed and failed rows retain this order; a lease
identifies an in-flight claim. Recovery requeues leased work, not every retained
fence. Changed cursors get a one-second follow-up poll. Unchanged frontiers poll
at one second for active sessions and 30 seconds for idle dormant sessions.
Disabled and ineligible cold sources create neither claims nor idle wake loops.
Cancellation preserves the cursor, consumes no retry, and keeps its one-second
retry delay. Transient read failures retain the cursor and use bounded backoff;
these source reads are separate from provider attempts. Session deletion and
**Clear index…** in Settings → Privacy remove the delivery state.

Native journal delivery and report readiness use the committed journal receipt
and current scope, independently of legacy metric freshness. Projection passes
do not also publish the legacy result tail. Outbox reads apply count and payload
byte limits and check remaining events even when a byte limit returns a short
page.

Newly delivered events, retained payload, and pages per turn are bounded. Live
delivery does not hash or reconstruct prior history on each page. Nonterminal
results carry pending context. The first terminal result freezes request context
once, and later corrections reuse it.
Reports show when an OpenCode journal is catching up instead of showing an idle
session's tool-call countdown. Progress uses the same receipt reset and scheduling
eligibility as native delivery, including native Claude, Codex, and Pi files.
A missing receipt or a binding that needs reset waits for data only when current
activity admits that work. Compatible pending or scheduled work can continue
after activity expires. Inactive sources with no eligible work do not create
indefinite catch-up progress. Source receipt, attachment, and pending delivery
remain separate from a ready selector's remaining tool-result count.
Bounded reads do not establish a measured latency
guarantee. The journal route applies only to native OpenCode provider-DB sessions, not
remote/WSL sessions, JSONL exports, or every source format.

### Live journal state limits

These bounds apply to the incremental OpenCode reader, not strict replay,
provider input size, or whole-process memory.

| State or work                          | Bound                                                                       |
| -------------------------------------- | --------------------------------------------------------------------------- |
| Event input per turn                   | 4,096 events and 8 MiB of event-data bytes                                  |
| Cold result output                     | 40 terminal identities and 4 MiB of serialized payload                      |
| Warm combined output per turn          | 40 facts and 4 MiB of serialized payload                                    |
| Retained tool identities               | 128 entries and 128 KiB of accounted identity state                         |
| Retained public state                  | 128 rows and 192 KiB; an owner and its parts are evicted together           |
| Request cache                          | 128 metadata/part records and 64 KiB of stored JSON values                  |
| One frozen request context             | 64 KiB after old-request eviction                                           |
| Shared frozen contexts                 | 16 contexts and 256 KiB total serialized context                            |
| Persisted live cursor JSON             | 1 MiB; enforced on desktop read and publication                             |
| Origin and current-boundary validation | Identity/type/length plus at most 4 KiB from each end of each sampled event |
| Cold ownership lookup                  | At most one 16 KiB assistant header per selected terminal identity          |

The input counters exclude bounded boundary samples and ownership point lookups.
An oversized event is not decoded or sampled as a full payload. Context and
identity eviction can make later corrections unavailable. It does not authorize
an old terminal-only identity as a new attempt.
The combined warm budget covers messages, results, activity, and removal facts.
If the next event cannot fit, the reader defers that whole event without advancing
its cursor. Ordinary continuation is not a coverage gap. A later completed header
cannot reconstruct an evicted public lifecycle, and
tombstone overflow fails closed.

### Journal delivery limits

These limits describe the strict journal reader and legacy sink, not the smaller
live cursor. They are not provider input budgets or whole-process memory limits.
Request-history records include message metadata and user text parts.

| Limit                        |   Value | Scope                                                           |
| ---------------------------- | ------: | --------------------------------------------------------------- |
| Parser revision              |      67 | Accepted source parser revision                                 |
| Resume snapshot revision     |      25 | File-reader resume state                                        |
| Result observations          |    4096 | Retained results per strict journal page or legacy sink window  |
| Result payload bytes         | 4194304 | Serialized result payload per strict page or legacy sink window |
| Request history records      |    4096 | Combined message metadata and retained user text parts          |
| Request history bytes        | 4194304 | Request-history cache                                           |
| Frozen request context bytes | 2097152 | Per-result context after old request eviction                   |
| Result field bytes           |  262144 | Each serialized argument value or recorded result text          |

### Journal admission and metadata gaps

Consumed-prefix change below refers to strict replay. Live delivery detects only
the bounded identity/boundary changes described above; it cannot certify every
historical payload.

| Evidence limit                                                                                     | Delivery and assessment consequence                                                                                                                   |
| -------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
| Missing result arguments/content, non-text user parts, compaction, or bounded request-history loss | Available fields can remain assessable with explicit quality limits. Unknown values stay unavailable to expressions.                                  |
| An old request is evicted but root-session and current assistant ownership remain proven           | Request history is partial; eviction alone does not make result ownership unsafe.                                                                     |
| Missing legacy model, provider, usage, or timing metadata                                          | This does not by itself block journal delivery or saved YAML results. It does not supply the missing metadata to metric checks.                       |
| Missing or conflicting result identity, call binding, native order, or main-session ownership      | Affected results are unsafe to select. Restoring an older frozen context cannot repair current unsafe ownership.                                      |
| Sequence discontinuity, malformed event data, or an unsupported event envelope                     | Affected order is unsafe. A gap that prevents cursor progress returns an explicit incomplete-history error rather than an endless empty continuation. |
| Missing supported journal                                                                          | New journal evidence is unavailable; snapshot timestamps and row IDs cannot replace native order.                                                     |
| Consumed-prefix change or invalid saved cursor                                                     | Delivery blocks for explicit source recovery; the prior cursor is retained.                                                                           |

Journal event IDs and type strings must fit their 512-byte read limit. Event data
above the 8 MiB decode limit cannot establish decoded evidence. Strict replay
hashes oversized data in bounded chunks; live delivery skips decoding it and
marks history unsafe. Oversized result arguments or content are omitted and marked
partial, not silently replaced with empty values. Missing optional metadata does
not justify a blanket zero-call rule for the session. Reading an unavailable
required expression can still make an individual candidate not assessable.

## Response-cycle evidence limits

The draft `turn_end` event means a source-proven response cycle ends, including
its automatic continuations and tools. It is not one assistant message, one tool
batch, inactivity, or permanent session completion. The native paths support
`completed`, `cancelled`, and `error` under these finite pinned contracts:

- Codex 0.160.1 at
  [`d27764b82f7118f674371e6d6e76271d9d606edb`](https://github.com/openai/codex/tree/d27764b82f7118f674371e6d6e76271d9d606edb)
  requires one exact task start and terminal, initiating and steering input/turn
  bindings, complete finalized public members, and no unresolved explicit calls.
  `task_started`/`turn_started` and `task_complete`/`turn_complete` share one cycle
  identity. Normal completion with an error maps to `error`. The four pinned
  `turn_aborted` reasons, `interrupted`, `replaced`, `review_ended`, and
  `budget_limited`, map to `cancelled`. Equivalent terminal aliases are replay;
  a conflicting later terminal removes the same cycle instead of creating one.
- OpenCode V2 v2.0.32 at
  [`843a6c514e606b436a7fe713422ace0a93ad3d61`](https://github.com/anomalyco/opencode/tree/843a6c514e606b436a7fe713422ace0a93ad3d61)
  requires an explicit verified capture baseline and a later observed execution
  start. The busy period includes all delivered prompts and steering. Native
  success/failure map to `completed`/`error`; `user` or `inactivity` interruption
  maps to `cancelled`. Shutdown and superseded interruption do not prove a usable
  cycle. A successor needs its own delivered input, not an old requester.

V2 cycle context uses the current retained session request prefix, not only the
requests that belong to that cycle. `turn.messages` remains the cycle's own public
members. Earlier missing history, prefix loss, conversion limits, and eviction
keep `session.requests_complete` false. Later valid requests do not restore lost
text or turn a known partial prefix into complete history. These rules use
retained parser state within the accepted source and producer bounds.

Capture continuity does not require every tool result to carry a complete
request prefix. A cycle can use its proven complete prefix while a tool member's
context remains partial. The member keeps its published context and exact
revision/hash binding. A complete local cycle or repeated terminal does not by
itself clear a capture gap.

Codex withholds cycles for missing starts/input, ID-less terminals, wrong or
ambiguous turns, overlaps, unknown public records, compaction/rollback, lost
projections, unresolved calls, or oversized intervals. Legacy UI/control shapes
without interval proof also remain unavailable. V2 withholds cycles for overlaps,
missing input or joins, unfinished public slots, retry ambiguity, reconnect, or
continuity loss. Increasing V2 public sequence gaps alone do not prove loss.
Independent admitted messages remain usable when a cycle is unavailable.

Cycle model and mode summaries use the complete nonempty native generating-request
set, including requests with no public text. Each field needs its own uniform
evidence. A uniform mode can remain known when models differ; missing evidence
stays unknown. A user-only cancellation has no generating model. The host does
not calculate this summary from only the public messages or current selection.
Recorded timestamps are optional integer milliseconds, not ordering keys. Native
reason and error-code metadata does not enter the authored `turn` root. Codex
error codes come from exact `error.codex_error_info` variants, not free-form text.

A cycle has at most 4,096 public members and exact member bindings. Serialized
member copies and the final envelope share a 4 MiB native bound; desktop also
bounds the payload and bindings together. Core admission adds limits of 4 MiB for
the event plus context, 262,144 JSON nodes, and depth 64. Smaller source budgets
win. V2 public state and one output batch each use 256 KiB within its 512 KiB
joiner bound. A V2 step, its calls, and its retained assistant message share one
frozen request prefix, counted once in retained public state. Output observations
own their context and remain subject to the separate output bound. Actual state
or output overflow still leaves a capture gap. Codex pending initiating input has
a 256-message/4 MiB bound; its
4,096-entry non-evicting call/turn and finished-task guards withhold later cycles
when full. Bound failures do not invent partial completed cycles.

Claude, Pi, and OpenCode V1 do not supply response cycles in their accepted source
routes. Claude `turn_duration`, Pi RPC lifecycle or terminal OSC status, V1
assistant completion, and host idle observations cannot replace that proof.

### Host session boundaries

The desktop emits `session_start` with reason `attached` once per admitted
activation, branch, source incarnation, and attachment epoch. Empty V2 sources
can attach. Compatible saved attachments initialize once after successful
upgrade validation without reseeding result windows. `new` and `resumed` are
language values, not reachable source claims in this subset.

`session_idle` records at least 120 seconds of continuous host observation
without new admitted native activity. Attachment or continuity recovery starts
a fresh interval. Genuine source work resets it using local observation time,
independently of authored filters. Private generation can count as activity
without exposing its text. Exact replay, historical baseline facts, polling,
usage-only refreshes, recorded selection changes, and UI state do not reset it.
Optional `session.last_activity_at` retains the native timestamp; it does not
control the deadline.

An overdue timer alone cannot publish idle. The host first validates a usable
file, journal, or V2 frontier against the exact source receipt, source label,
CWD, incarnation, attachment epoch, continuity, and activity revision. Gaps or
disconnects withhold idle until recovery establishes a fresh interval. One
activity revision yields at most one idle boundary. This proves an observed idle
interval, not cycle completion, historical inactivity, or session termination.
Host boundary contexts have empty requests and partial request coverage; they do
not invent a historical user prefix.

Attachment epochs, activity revisions, deadlines, and emitted boundaries persist
across restart. New activity cancels unfinished idle work before selection.
Claim, local start, provider dispatch, and acceptance recheck the exact typed
dependencies. A start needs the same current attachment but does not require
unchanged later activity. Accepted idle evidence remains valid at its saved
boundary; ordinary later activity does not erase that history.

## Cursor Desktop YAML tool evidence

The parser retains native `toolFormerData` from an ordered desktop composer
snapshot. It reads composer headers and bubble records in one SQLite read
transaction. First-record metadata must identify `desktop_state_vscdb`, the exact
session ID, `recorded_headers` order, and a known root session. The header list
must contain nonempty unique bubble IDs. Missing referenced bubbles cannot prove
complete history. Bubble keys retain their full native IDs, including colons and
newlines. A tool bubble must match its recorded header identity. Empty legacy
conversation maps do not invalidate an otherwise recorded order.

The characterized `run_terminal_command_v2` result supplies JSON arguments,
recorded output, terminal state, and accepted user-slot prefix. Its recorded exit
code helps derive status; it does not create a normalized `result.outcome` field. Native
user slots supply result context, not a separate `user_message` trigger or proof
of human authorship. Exact call, bubble, and tool identities remain required.
Interrupted or rejected work stays distinct from success. When exit code is
absent, accepted native `success`/`error` status can supply the result status; a
malformed exit code cannot use that fallback. Unknown tool result shapes remain
partial. Binary tool payloads and unrelated composer fields are excluded.

Missing first-record metadata, conflicting identities, unproven order, incomplete
records, forks, and child sessions cannot establish complete root evidence.
Winning full snapshots reconcile through the native-result outbox. Startup
attachment binds the exact current source generation and incarnation; later
result publications use activation and receipt fences.

Desktop admission requires native main-session evidence, no WSL source, an inline
source labeled `cursor-desktop:*`, and current ready cached evidence. Cursor does
not use the file or OpenCode journal cursor routes. Its snapshot result evidence
does not supply requester model/mode, operation paths, response cycles, public
assistant finality, or host start/idle observations.

This contract applies to `CursorIdeComposer` only. It does not widen Cursor CLI,
legacy stores, generic selected-input tool anchors, or retained built-in check
capabilities. It does not establish all seven public events. Native WebView and
live-source acceptance remain separate from synthetic characterization. The
[discovery tests](../crates/antiburn-local/src/discovery/agents/cursor.rs),
[parser tests](../crates/antiburn-local/src/analysis/vendors/cursor/desktop.rs),
and [desktop publication tests](../apps/desktop/src-tauri/src/installed_checks/tests/cursor_desktop.rs)
define this synthetic accepted shape. No Cursor release range or public producer
schema is claimed.

## Retained native content

These contracts describe parsed source evidence, not native model-based
detectors or additional YAML capabilities. Producer pins, source roles, exact
joins, and retained-history limits remain separate from check admission.

Parser revision 58 includes retained-root history, native completed
work, bounded reads, skill selection, and normalized human/result context.
When a read resumes from a retained suffix, the first record must link to the
saved last record. An unknown or disconnected seam invalidates root-context
admission. Full-file reads still establish the root from the complete prefix.
The four pinned native contracts are:

- Claude Code / `ClaudeJsonl`: local CLI 2.1.278 and 2.1.309 retained-root shapes. The root is
  human text or the characterized informational prelude. Exact retained parent
  chains and explicit `origin.kind:human`, `promptSource:typed`, and
  `turnOrigin:human` markers bind text-only human messages. SDK/meta/synthetic
  roots and other releases do not inherit this proof. The characterized
  `system/bridge_status` seam can connect 2.1.278 to 2.1.309 without granting
  human authority. Native Read, Bash, and failed Skill results require exact
  joins. Read returned extents remain pinned to 2.1.278. The private producer has no public
  schema pin. See the [Claude fixture contract](../crates/antiburn-local/tests/fixtures/claude_characterization/README.md#retained-cli-root-and-native-results).
- Codex / `CodexRolloutJsonl`: 0.160.1, producer
  `d27764b82f7118f674371e6d6e76271d9d606edb`, accepted paginated completed-work
  shape and characterized legacy retained-message subset. Consecutive ordinals,
  native IDs, retained revision, and text ranges constrain scope. Completed
  commands and file changes are separate work/result facts. Only the pinned
  single-file numbered shell slice supplies observed read extent. Qualified
  environment context accepts one local environment with the characterized
  restricted filesystem profile. It remains non-human and non-authorizing.
  Unqualified lookalikes, multi-environment/delta forms, inherited/clipped records,
  and other producer versions cannot justify omission from human scope history.
  See the [Codex fixture contract](../crates/antiburn-local/tests/fixtures/codex_characterization/README.md#paginated-completed-work-contract).
- OpenCode / `OpenCodeSqliteV1`: native message/part and skill producer
  `772392050500e0ddcd2ad2193411a22a3824372f`, with numbered Read renderer
  `652c090dc119b5f3dc1e5e0bf1c4b40d9721f0ef`. Exact native identities and lifecycle
  joins are required. Text-only retained roots can carry partial-context limits.
  JSONL export does not inherit this SQLite evidence contract.
- Pi / `PiV3Jsonl`: native core V3 shape inspected at 0.84.4, producer
  `b79e4cc834970cca69daebffab7df1da7d1e52c4`. Linear retained ancestry, exact
  tool-result joins, and explicit `isError` constrain work/results. Skill wrappers
  are unknown-authority document context; only an exact trailing argument range
  supplies separate user text. No extension identity or human origin is inferred.
  V1/V2 metric migrations do not establish this retained V3 content contract.
  See the [Pi fixture contract](../crates/antiburn-local/tests/fixtures/pi_characterization/README.md#core-retained-v3-contract).

These are bounded accepted shapes, not historical release ranges. Published
content retains source/session identity, order, ranges, generation, and its
publication fence. Unknown authority, unsupported attachments, compaction,
missing retained history, unresolved branches, and collection limits cannot
supply missing text or approval. Qualified Codex environment context and exact
skill-document selections remain non-authorizing. Skill selection does not
prove instruction execution, success, or historical availability.

Root identity and per-message proof do not establish that no earlier record was
deleted outside the accepted producer contract. Antiburn does not reconstruct
removed messages or claim original historical retention.

## Private file-read evidence

`ReadFileRequest` and `ReadFileResult` are optional selected fields. They do not
join ordinary metrics or report queries. Parser revision 58 populates
`values.read_file_request` from recorded source. Older rows can retain a read path
without its requested range. That range stays unknown until source reparse.
Evidence schema revision 22 remains current.

The accepted request contracts are:

- `ClaudeJsonl`: exact native `Read` calls with `file_path`, `offset`, and
  `limit`, as described by the [public SDK Read schema](https://platform.claude.com/docs/en/agent-sdk/typescript#read).
  Offsets and limits use lines. This schema and the synthetic transcript fixture
  define the accepted shape; no installed release range is claimed.
- `PiV3Jsonl`: exact native `read` calls with `path`, `offset`, and `limit`,
  pinned to [pi-mono read.ts at `b79e4cc`](https://github.com/badlogic/pi-mono/blob/b79e4cc834970cca69daebffab7df1da7d1e52c4/packages/coding-agent/src/core/tools/read.ts).
  The read fixture uses the accepted version-3 journal header. Offsets and limits
  use lines; this does not characterize all historical tool versions or migrated
  journals for read-extent semantics.
- `OpenCodeSqliteV1`: exact native `read` calls with `filePath`, `offset`, and
  `limit`, pinned to [OpenCode read.ts at `652c090`](https://github.com/anomalyco/opencode/blob/652c090dc119b5f3dc1e5e0bf1c4b40d9721f0ef/packages/opencode/src/tool/read.ts).
  File requests use lines. A matched directory result resets the requested unit
  to unknown because its offsets count entries, not file lines.

Other read aliases on shared selected-content sources can retain request
numbers and native range keys, but their unit remains unknown. Integer
`offset`/`limit` values are not converted to byte offsets or default ranges.
Equivalent native keys such as `startLine`, `endLine`, `byte_offset`, and
`byte_limit` remain literal arguments with uncharacterized semantics. Invalid
values remain recorded but do not supply a typed numeric extent. Missing
offsets and limits do not prove a full-file request. Paths stay recorded paths;
conflicting `cwd`/`workdir` values leave CWD unknown. No current filesystem
state or canonical path is reconstructed.

Returned line extents include the pinned OpenCode file
output: a `<path>`/`<type>file</type>`/`<content>` wrapper, consecutive numbered
lines, and a matching end-of-file, continuation, or 50 KB-cap footer. The extent
identifies observed returned lines, not the whole requested interval. A
continuation or byte cap records truncation. A clipped line or a locally
truncated result has no complete returned extent. Directory outputs have a
separate result kind and no file extent. Empty-file output has successful file
status without an invented nonempty interval. Image/PDF notices, previews,
outlines, searches, unknown wrappers, and malformed footers do not establish
returned file extents.

Claude 2.1.278 accepts exact matched native text payloads and tab-numbered output;
absent successful `is_error` resolves only through that matching payload. Pi
0.84.4 retains returned text separately from continuation notices and validates
native clipping details. Codex 0.160.1 accepts only the characterized
`nl -ba PATH | sed -n 'START,ENDp'` slice with a matching native parsed-command
path and consecutive numbered stdout. Completed command output is clipped only
for the exact `[output truncated]` producer marker or when it reaches the
content-part byte cap; ordinary prose such as "truncated" and ellipsis
characters are not producer framing. These contracts prove observed lines,
not requested extent or whole-file access. Images, PDF, directories, persistence
and partial-view notices, mismatches, and unsupported renderers supply no extent.
No external output file is read.

Result joins require one earlier call with the same source, thread, call ID,
and exact tool name. Missing, duplicate, cross-source, or cross-thread identities
leave the request join unknown. Matched OpenCode completed/error state supplies
success/failure only with a recorded result. The pinned Claude and Pi native
adapters supply bounded result status/extent; other shapes remain unknown.
Recorded output byte counts and
digests describe UTF-8 transcript text, including framing. They are not file
sizes or file versions. No current adapter supplies a recorded file version.

Search and listing tools are not file reads. Dedicated outline and preview
tools do not become reads. A simple `cat path` remains Bash evidence. The only
accepted shell-read exception is the pinned Codex numbered slice above.
Repeated or overlapping requested/returned slices do not by themselves prove
waste. The [synthetic read fixtures](../crates/antiburn-local/tests/fixtures/read_characterization/)
and `read_characterization` tests cover persisted requests, repeated,
overlapping and disjoint slices, changed output without version proof, Unicode,
truncation, failures, and unavailable extents. `OpenCodeJsonl` and `OmpV3Jsonl`
remain outside the shared selected-content source gate.

## Status Rules

| Status          | Meaning                                                                                                         |
| --------------- | --------------------------------------------------------------------------------------------------------------- |
| Characterized   | Committed fixtures define the accepted source shape and important failure cases.                                |
| Partial         | The reader parses useful facts, but source shapes, versions, companions, or completeness rules still have gaps. |
| Uncharacterized | Discovery can identify the source, but no detector-grade parsing contract exists. The reader must fail closed.  |
| Not a session   | The discovered data does not contain a conversation session and must not inherit session coverage.              |

## Pipeline Contract

### Normalized skill evidence

Native adapters retain source/thread digests, native record and call IDs,
request/result references, roles, record positions, available timestamps, and
the publication fence in selected private content. The
[synthetic native characterization contract](../crates/antiburn-local/tests/fixtures/skill_use_characterization/README.md)
defines the accepted shapes and source pins for `ClaudeJsonl`,
`OpenCodeSqliteV1`, `CodexRolloutJsonl`, and `PiV3Jsonl`. `OpenCodeJsonl` does not
inherit these contracts. Parsed evidence does not add skill inventory or
skill-use judgments to the YAML runtime.

Adapters validate native skill envelopes and persist selected generic facts.
Shared consumers read those facts, not native tool-name or markup guesses. Requests,
selected documents, failures, and unknown outcomes remain distinct. Exact
source/session/message/part/call identities, ranges, digests, selection, and
publication fences bind them. Supplemental records cannot repair an invalid
persisted proof.

Claude's 2.1.278 failed Skill shape and its characterized 2.1.309 CLI variant
retain failure/error state and exact result ranges, not successful document
delivery. The older conditional launch decoder contract stays separate; a launch
report is not task success. OpenCode document-result proof is pinned to
[`anomalyco/opencode@772392050500e0ddcd2ad2193411a22a3824372f`](https://github.com/anomalyco/opencode/blob/772392050500e0ddcd2ad2193411a22a3824372f/packages/opencode/src/tool/skill.ts)
and needs a selected complete result, exact join, completed state, full document,
and native metadata. Codex full documents and Pi core wrappers supply recorded
document selection without human approval or successful execution. Explicit Pi
requests do not prove an extension's identity or success. Mentions, listings,
aliases, and current files do not establish native use or past availability.
OpenCode JSONL remains unavailable. `OtherToolOutput` selection is required for
result text; input-only projections cannot carry it.

Missing IDs, timestamps, fields, or native metadata, duplicate calls, partial content, and
unknown producer shapes remain explicit limits. Complete means complete accepted
records in that selected window. It never proves session-wide or equivalent-use
absence. Existing aggregates remain inferred requests with unknown time/order
and cannot prove absence.

### Existing session pipeline

Every supported source passes through these boundaries:

1. Discovery identifies the agent, native source, surface, session identity, and companions.
2. Source validation pins the accepted file boundary or database snapshot.
3. The shell supplies `SessionInput.source_format`; `reader_for` selects the
   `SessionReader` by agent label, and the reader consumes that source contract.
4. Bounded framing rejects oversized, malformed, truncated, or unreadable records.
5. Parsing emits normalized metrics, content, and evidence observations.
6. The evidence sink records complete, partial, or unavailable facts.
7. The check contract decides whether a finding or clean result is valid.

Unknown changed evidence-bearing shapes must not fall through to a generic
interpretation that can produce clean. A known shape need not have a universal
release range: an accepted schema, header, or pinned producer commit with
synthetic fixtures can define its contract. This does not prove all historical
versions. Full and resumed reads must agree where resume is supported.

Private message content remains in `turn_content`; it does not join ordinary
turn, metrics, or report queries. The dedicated published-content query reads
only the winning publication under the store lock and caps output at 256 parts
and 1 MiB. It returns explicit truncation and omission flags. Each part retains
source authority and available native tool name/call ID. Thinking remains
stored locally but shared selected-content preparation excludes it.
Existing rows migrated before turn schema V9 have `unknown` content authority
and no reconstructed tool joins.

The engine also defines optional typed question-answer and plan-reference
metadata in private turn content. Explicit field selection and source-bound
retrieval preserve those records without a schema migration. OpenCode extraction
accepts the message/part shapes pinned at `anomalyco/opencode` commit
`772392050500e0ddcd2ad2193411a22a3824372f`, with synthetic SQLite and wrapped
JSONL characterization. The Jev projection accepts `OpenCodeSqliteV1`; its
existing gate excludes `OpenCodeJsonl`. Conditional typed records also come from
`ClaudeJsonl`, `CodexRolloutJsonl`, and `PiV3Jsonl` under the producer pins below.
Cursor and Antigravity do not emit typed question-answer or plan-reference forms.
These are private parser/storage facts, not new YAML context fields or events.

OpenCode questions retain headers, full prompts, option descriptions, ordered selections,
and exact custom strings. Submitted structured answers require matching native
output. Text-only fallback accepts one question with one delimiter-free answer.
Dismissed, interrupted, pending, compacted, truncated, malformed, and unmatched
results do not establish submitted user authorization. The pinned `plan_exit`
wrapped local completion records a build-switch approval. Provider-executed
records do not establish user authorization. The completion does not identify
the approved plan path, contents, or version. Synthetic text may retain a plan
path, but it cannot establish approval or bind that path to a nearby tool call.
Historical plan content stays unresolved; current mutable files do not prove the
approved version. No historical release range is claimed.

Claude question extraction accepts assistant `message.content[]` tool calls with
`type: "tool_use"`, `id`, `name: "AskUserQuestion"`, and object `input`. User
`tool_result` blocks join by `tool_use_id` within the same source, session,
agent, sidechain, and resolved branch. The structured result must belong to one
result block; an available `sourceToolUseID` must agree. Duplicate or conflicting
call IDs, malformed-record gaps, and unmatched results do not resolve answers.
The accepted answer-map shape is pinned to the public decoder and fixtures at
[`futpib/claudex@0ad5073`][claude-question-source]. The native question-input
consumer at [`TrafficGuard/typedai@34139ae`][claude-question-input-source]
supports retaining headers, prompts, option labels/descriptions, and the question-text
answer join. `multiSelect` retains its recorded boolean. A string answer remains
one exact value: combined selections and free text are not split on commas.
Only an exact unique option label receives an option index.

Top-level `toolUseResult.answers` maps question text to strings. The result can
also retain `questions`; conflicting question versions remain unknown. Intact,
nonempty string answers have submitted status but **unknown origin**, never
proven human authority. Missing, malformed, interrupted, denied, and synthetic
results do not prove submission. SDK `updatedInput` and freeform `response`
are not accepted CLI result fields. Rejection is not proof of a deliberate user
decline: public issue [81223, comment 5080551532][claude-question-interruption]
reports shutdown interruptions stored as rejection. Comment
[5127759342][claude-question-free-text] reports single-select free text delivered
later as ordinary user text; the parser retains it without inventing a question
join. These issue observations do not establish a historical release range.

Claude plan/file result shapes are pinned to published `vct-core 2.7.1`, archive
SHA-256 `fc41d67b80db72fc9e13cd6a71e76846cc20a399fb8c9d3592849a42f58c2f82`,
and its [native decoder fixtures][claude-plan-source]. The package's VCS metadata
reports a dirty tree, so its Git SHA alone is not the accepted pin. `ExitPlanMode`
input `plan` retains a proposal; result `toolUseResult.plan` and `filePath` retain
exact recorded text and a path. Neither completion, approval wording, permission
mode, nor an auto-approved tool grants scope approval. Result text is feedback
with unknown authority. No approved revision or digest is inferred.

For direct Markdown paths under `/.claude/plans/`, Write input retains a proposal.
A successful matched Write result with `type: "create"` or `"update"`, matching
`filePath`, and matching `content` retains a recorded version. Full Read results
require `type: "text"`, a matching `file.filePath`, `startLine: 1`, and equal
integer `numLines`/`totalLines`; their exact `file.content` is retained. Read shape
and extent fields also have public fixtures at
[`folke/zaly@5a11351`][claude-read-source]. Each exact text has a SHA-256 content
identity. Partial reads, absent results, mismatched writes, and mutable references
remain unresolved. Custom plan directories, Edit reconstruction, result-text-only
plan extraction, and historical companion-file loading are not supported.
Recorded versions are not assigned to nearby approvals or later mutable paths.
The native `planContent` user-record shape reported in [issue 24302][claude-plan-user-source]
retains exact text with synthetic origin and unknown approval; that issue reports
stale cached plan text in Claude Code 2.1.37. Synthetic/meta/compaction user text
has unknown content authority. Ordinary later user feedback remains user text.
Pending call metadata is bounded to 4,096 identities and 1 MiB of input text;
limit loss remains explicit. `scope_records.jsonl` and the Claude characterization
tests cover these accepted shapes and negative controls.

Codex scope extraction accepts the persisted shapes at `openai/codex` commit
`e7637306bc9246a3e42e407cb94f96b7ed345e3e`, not a historical release range.
The [fixture contract](../crates/antiburn-local/tests/fixtures/codex_characterization/README.md)
links the producer definitions. Plain `request_user_input` calls join results by
exact call identity and compatible turn metadata. Questions retain headers,
prompts, option descriptions, ordered answers, and exact custom strings. Missing
results remain pending; the exact cancellation string records cancellation.
Malformed results, duplicate identities, rollback, and source gaps do not prove
submission. Human origin remains unknown. Retained answers and checkpoints keep
optional native acceptance order separately from question order. A later retained
record with acceptance order remains typed even when its answer matches an earlier
tool result. All typed occurrences remain recorded, including the richer original
tool answers and repeated answers after a correction. Incomplete checkpoints
remain bounded excerpts, not full scope history. Inherited fork context and
`metadata.inherited_user_message` records retain unknown content authority;
they cannot grant child-local authorization.
`update_plan`, native completed `Plan` items, and assistant proposed-plan sections
retain recorded proposals. Completion and later implementation wording do not
bind an approved plan version. Live protocol events are not persisted substitutes.

Pi scope extraction accepts official example shapes at pi-mono `0.85.1`, commit
`b2602be77cb7b0de45dd616407fd210daa48aa75`, and `pi-ask-user` `0.16.0`, commit
`adcc7b2ee22bb290f9d6693efe6c0d0cd1273280`. The
[fixture contract](../crates/antiburn-local/tests/fixtures/pi_characterization/README.md)
defines the accepted journal shapes. Calls and results join only through recorded
ancestry. Questionnaire display labels map to `header`; `ask_user` retains context,
selection comments, freeform text, and skipped batch status in typed records.
Cancelled partial questionnaires retain answers without submission authority.
Timeout and cancellation share a persisted shape, so timeout is not inferred.
The journal does not prove an installed extension's identity; origin and question
provenance remain unknown. The exact plan-mode state/list/execution chain can
record a synthetic execution choice. Full proposal text and its digest require
matching recorded todos and complete contributing ancestors. Cyclic, missing,
oversized, or incomplete ancestry cannot bind answers or approved versions.
Noninteractive questionnaire errors remain unknown, not user cancellation.
Mutable external files are not loaded. Branched journals remain incomplete.
These pins do not establish all historical extension versions.

Cursor generic store envelopes are pinned to `antonvp/cursor-acp-enriched` commit
`4801804543f0234bdfc266fbd53d81a6f20e9508`. Characterized top-level messages
preserve structured tool calls/results, provider options, repeated user text,
and literal formatting. Tool payloads and unknown wrappers cannot supply nested
user authority. Database row order and IDE bubble-ID fallback order are not
chronology proof; synthesized records carry an explicit attribution gap.
The pin does not establish dedicated AskQuestion answers, Build approval, or
approved plan-version linkage. ACP session-store discovery remains unavailable.

Antigravity brain records are pinned to `nizos/probity` commit
`f750c1d82d2bcc842d4bfe7f00401758a3584b56`. Only scalar `USER_INPUT` content
with `source: USER_EXPLICIT` establishes human origin. Nested and cascade
compatibility text remains unknown authority and creates an attribution gap.
The agy 1.0.16 SQLite subset requires `user_version = 1`; its decoder pin is
`ccusage` commit `90e296efd1bdd25a9db07019854255284588d720`. A brain companion
must match the owning database stem and is rechecked during claimed reads.
Artifact files do not become sessions. Neither pin establishes review comments,
Proceed approval, or approved artifact versions. Notifications and Always Proceed
policy do not establish user acceptance. Mutable artifact text cannot prove a
historical approved version.

Scope bindings carry an optional native record ID for each string range. Claude
and Pi can bind matched call and result strings to different known records.
Codex retains both encoded argument and output string ranges; absent native IDs
remain absent. Headers, context, comments, skipped lifecycle, and acceptance order
survive explicit selected storage and contribute to its digest. Parser revision
67 and resume snapshot revision 25 fence incompatible parser and scope snapshots.

### Current instruction discovery

Shared current-file discovery uses async filesystem APIs. The scan caps each
file at 128 KiB, all instruction text at 512 KiB, the source set at 64 files,
imports at eight levels, and rule trees at four levels, 128 directories, and
256 entries per directory. A complete directory scan means only that the adapter
finished its known paths; it does not establish historical activation.
Conditional scope, such as Claude `paths`, remains attached to the observed
file. This discovery API is separate from the YAML collector below.

Instruction-source adapters use these current file contracts: Claude loads
project/user `CLAUDE.md`, local variants, `.claude/rules/*.md`, and bounded local
`@path` imports; AGENTS applicability remains conditional when Claude's file
selection setting or version is not known. Codex follows AGENTS override
precedence and reads `project_doc_fallback_filenames` from `CODEX_HOME/config.toml`.
Pi reads hierarchical AGENTS/CLAUDE context files and project/user SYSTEM and
APPEND_SYSTEM files. OpenCode follows AGENTS then CLAUDE fallback and reads
literal local paths from strict-JSON `instructions` arrays. OpenCode JSONC
configuration, remote URLs, and config globs remain explicit limits. Cursor
reads hierarchical AGENTS and `.cursor/rules/*.mdc`; `alwaysApply` rules are
distinct from file-glob/manual rules, which stay conditional without matching
file evidence. Cursor User and Team Rules do not have a supported local file
source. Antigravity reads GEMINI.md and `.agents/rules` files; conditional rule
activation stays explicit. Claude managed inline policy is not read.
`instructions::CurrentInstructionSnapshot` records current file text, identity,
digest, source, scope, imports, and limits. It does not prove historical model
context.

All six supported first-tier adapters discover the user-global `~/AGENTS.md` and
applicable project `AGENTS.md` files in the session worktree. They also retain
each agent's supported global and project instruction paths and override behavior.
OpenCode additionally discovers
`~/.config/opencode/AGENTS.md`. Global and project sources stay separate. A disk
snapshot cannot prove what an older session received or when a rule became active.

### Current skill definitions

The production inventory uses the checked reader for bounded local `SKILL.md`
definitions within the native agent, project, and environment boundary. Readable
definitions with malformed contents or invalid UTF-8 can still produce named
inventory resources without token estimates.

The retained `agent_config::skill_definitions::parse_frontmatter` parser is
separate from production listing. It returns a name, declared description, and
semantic frontmatter. It bounds files at 256 KiB, descriptions at 16 KiB, and
frontmatter at 32 KiB. Invalid YAML and non-string descriptions produce no parsed
result. A missing description stays empty; a skill body never supplies the
description. Parsed current contents do not prove past visibility or activation.
This parser does not add skill snapshots to YAML model input.

### YAML current-file inputs

YAML checks use the separate bounded `agent_config::current_context` collector.
It observes current configured definitions, not historical activation or an
effective session inventory. The accepted current-file subset covers native
Claude, Codex, Pi V3, and separately reviewed OpenCode V1/V2 configuration shapes.
Other agents, remote/WSL environments, and mismatched source/agent pairs leave
collections unavailable. A format tag alone does not establish a producer release
range or resolve process overrides.

Claude observations include default user/project instruction files, supported
conditional rule globs, and skill metadata. Imports, managed inline policy,
plugins, and effective skill controls remain limits. Codex observations use
default AGENTS precedence and `.agents/skills`; custom `CODEX_HOME`, fallback
filenames, and effective skill controls remain limits. Pi observations use
default AGENTS/system files and default local skill roots; custom agent
directories, settings-selected paths, packages, and CLI overrides remain limits.

OpenCode V1 retains its AGENTS/CLAUDE fallback and `skills.paths` object shape.
V2 uses AGENTS without the Claude fallback, array-based skill configuration, root
Markdown definitions, and bounded nested `SKILL.md` definitions. A root
`SKILL.md` does not hide valid siblings or nested definitions. V1 objects do not
silently become V2 arrays. Only literal local configured instruction paths are
supported; URLs, glob expansion, managed/runtime authority, and effective winners
remain limits. The current V2 traversal follows producer
[`843a6c514e606b436a7fe713422ace0a93ad3d61`](https://github.com/anomalyco/opencode/blob/843a6c514e606b436a7fe713422ace0a93ad3d61/packages/core/src/config/plugin/skill.ts).
Pi's current-file subset follows producer
[`936e5f40aede9327b9142b63506ed2b68bc6b4ac`](https://github.com/badlogic/pi-mono/blob/936e5f40aede9327b9142b63506ed2b68bc6b4ac/packages/coding-agent/src/core/resource-loader.ts).
Claude and Codex current-file scope follows their documented
[memory](https://code.claude.com/docs/en/memory),
[AGENTS](https://developers.openai.com/codex/guides/agents-md), and
[skill](https://developers.openai.com/codex/skills.md) contracts reviewed on
2026-10-23, not an all-version claim.

The collector requires source-owned home, CWD, repository binding, and recorded
protected-directory grants. It checks every file and directory before I/O,
including configured targets. It does not probe for consent. Symlink targets,
symlink ancestors, and paths outside admitted roots remain unavailable. Safe
siblings remain usable beside denied or malformed entries. On Windows, ordinary
path components that end in a space or period remain unavailable because Windows
can resolve them differently during a prefix check and a full-path read.
Drive and UNC paths can use the verbatim prefix to retain literal names, but
they still require root and symlink checks. Other Windows namespaces remain
unavailable. Ambiguous conditional
frontmatter is omitted with a limit, never made unconditional. Skill descriptions
come only from bounded declared metadata; missing descriptions stay empty and
duplicate names at distinct paths remain separate. Skill bodies do not enter this
input.

One capture shares limits of 64 instruction records, 128 skill records, 256 file
attempts, 1 MiB of file/header reads plus one over-limit probe byte, 128 directory
attempts, and 4,096 charged directory entries. Instruction/config files have a 128 KiB limit; skill frontmatter has a
32 KiB limit and descriptions have a 16 KiB limit. Limits preserve known partial
entries beside separate availability/completeness metadata. The
[collector tests](../apps/desktop/src-tauri/src/agent_config/current_context/tests.rs)
and [review regressions](../apps/desktop/src-tauri/src/agent_config/current_context/tests/review.rs)
define these supported file shapes and bounds.

Capture occurs only when a condition or candidate needs these current inputs.
The host freezes the snapshot in candidate JSON before provider work. Native
observations, source identities, and evidence versions stay unchanged. A crash
before candidate commit can cause another capture; committed delayed decisions,
safe retries, restart, and acceptance reuse the saved snapshot without reading
files again. Collection timestamps describe observation time, not the time of
earlier agent work.

### Shared selected-content evidence

`analysis::jev_evidence` owns normalized fields, projection, availability, and
local source bindings. Its shared source gate accepts `ClaudeJsonl`,
`CodexRolloutJsonl`, `PiV3Jsonl`, `OpenCodeSqliteV1`, `OpenCodeSqliteV2`,
`OpenCodeV2SessionExport`, `CursorCliAgentJsonl`, `CursorCliStoreDb`,
`CursorChatStoreDb`, `CursorIdeComposer`, `AntigravityBrainJsonl`, and
`AntigravitySqlite`. This private-content gate is not live YAML admission or
detector eligibility. The shared private Cursor projection supplies user and
assistant fields, not typed file-read, question-answer, or plan-reference proof.
The separate Cursor Desktop YAML result route does not add those private typed
forms. Antigravity SQLite content needs an assessable paired brain transcript.

`JevFieldAvailability` separates excluded, unsupported, not-observed, and observed
fields. Source capability does not prove that a field occurs in one page.
Availability retains observed, empty, malformed, and truncated part counts.
`UserAnswer`, `PlanReference`, `ReadFileRequest`, and `ReadFileResult` require
explicit selection. `JevInputSelection::ALL` retains its text-only mask.
Unselected metadata does not change a selected-input digest at the same fence.
Selected records and their revisions do affect that digest. Private thinking
cannot supply selected text or typed question/plan records.

Private question and plan records live in `JevOperationMetadata`, stored under
`metadata` in `turn_content.normalized_fields_json`. They retain source format,
native role/record/call identity, question order, optional producer acceptance
order, provenance, revisions, native ranges, and truncation. Answer status and
origin stay separate. Plan records distinguish recorded text, mutable references,
missing text, and unresolved approved versions. Queries retain session and
publication fences, paging, and selected-byte limits. Oversized metadata is
rejected or omitted explicitly, not summarized. Ordinary tool names or answer
strings cannot create these typed records.

Submitted, skipped, cancelled, timed-out, pending, and unknown answer statuses
retain selections and exact free text. User, synthetic, automatic, and unknown
origins remain distinct. Questions retain full prompts, optional headers,
context/comments, ordered options and descriptions, and multi-select metadata.
Plan records retain proposed/approved/rejected/feedback status, recorded text,
feedback, path, identity, revision, and available current/approved content digests.
A stored plan status is source evidence, not a consumer's approval decision.
Current mutable bytes do not prove the version that received approval.

Parser revision 58 includes normalized request envelopes, operation metadata,
native field bindings, truncated-request isolation, and Claude result
identities. Claude joins an unnamed result only to a recorded call in the same
resolved branch. The bounded map persists across resume; missing, conflicting,
over-limit, and cross-branch identities do not create a join. Explicit empty
message and result text remains observed, while non-text result blocks do not
become text. Antigravity SQLite companion assistant content remains assistant
content while duplicate companion usage remains excluded from metrics. This is
a native-shape contract, not a new producer-version range.
Bash context retains
recorded CWD, workdir, shell, login, and timeout scalars without outputs. Search
constraints reject arbitrary nested objects. Recorded `patchText` in an
OpenCode `apply_patch` request supplies patch operations and edit paths; empty
or malformed input does not supply a path. Patch renames retain both paths and
exclude path headers from content-only selection. Native equivalent-event
fixtures cover all six admitted formats through fenced storage. OpenCode's
SQLite lifecycle fixture separates pending/running requests from completed
output and error text. Its typed native lifecycle labels survive selected
storage. The pinned Claude, Codex, and Pi result contracts above also retain
lifecycle facts; uncharacterized shapes remain unknown. Direct retained native
request strings have decoded-field UTF-8 bindings in all six admitted shapes;
encoded JSON arguments, nested wrappers, arrays, and patch-derived paths have
no native-range claim. Truncated known requests retain metadata without a raw
script fallback. Read/edit envelopes retain recorded CWD/workdir strings.
Platform, session-header CWD propagation, and native glob dialect remain
unavailable. Host state cannot supply missing recorded context. Unsupported native identities remain
unavailable, and request presence does not prove execution. These parsed facts do
not widen producer-version or check-admission claims.

Claude's branch-scoped call-identity map retains at most 4,096 calls and accepts
identities up to 512 bytes. It survives validated resume. Missing, reused,
conflicting, cross-branch, and over-cap identities cannot acquire tool names from
adjacency or another branch. Ownership alone does not prove execution success.

The shared normalizer accepts object arguments, encoded JSON arguments, and up
to four `arguments`/`args`/`params`/`input` wrappers. It bounds input and normalized
requests at 256 KiB, objects/arrays at 256 entries, and decoded structures at
16 levels and 4,096 values. These generic decoding forms do not prove producer
support. Malformed known tools cannot become other-tool input. Projection removes
unselected bodies and bindings; changes to those bodies do not change the digest
at an unchanged publication fence.

Dedicated edit content accepts recorded string bodies and arrays of edits with
string bodies. Numbers, nested result objects, and unknown edit items cannot
become edit text. An explicit empty body is observed and differs from a missing
or malformed field. Read paths, edit paths, outputs, and bodies stay separate.

Native field ranges are UTF-8 offsets in decoded source strings, not transcript,
SQLite-cell, or escaped-JSON byte positions. Lifecycle labels retain their native
meaning; `completed` alone proves neither command success nor resulting file state.
Recorded path facts cap each path, CWD, and glob at 4,096 bytes and lists at 256
entries. Conflicting CWD aliases stay unknown. Recorded paths and glob strings
do not prove resolved file identity, symlink identity, shell expansion, platform
semantics, or a native glob dialect. These facts do not implement YAML path filters.

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

Codex `thread_rolled_back` is an explicit request-history boundary. It is retained
as a compaction-boundary event so depth, cache accounting, metrics, and resumed
reads cannot join requests across the rollback.

Maintainer confirmation (2026-09-12): repair delayed exact-copy deduplication,
request pairing, and full-denominator accounting. Reviewed passive alternatives
include increasing the time limit and adjusting thresholds; neither repairs
all three accounting errors. Per-session thresholds remain unchanged.

Claude JSONL usage parses a nested `cache_creation` breakdown
(`ephemeral_1h_input_tokens`, `ephemeral_5m_input_tokens`) into
`cache_write_1h_tokens`, the subset of cache-creation tokens Anthropic bills
at the one-hour premium rate instead of the catalogue's default (five-minute)
rate. The flat `cache_creation_input_tokens` total takes the larger of itself
and the breakdown's sum; the one-hour count never exceeds that total. A
present breakdown always wins. Claude Code has run with one-hour caching
configured throughout, so a Claude record with no nested breakdown
classifies its whole cache-creation total as one-hour writes instead,
mirroring the cadence parser's own default for sessions that predate the
breakdown. Non-Claude sources carry no such default: an absent breakdown
there reports zero one-hour tokens.

Inline materialized sources use a fingerprint of the full bounded content, not
only a head region. The content is already materialized and size-bounded before
this fingerprint is calculated. OpenCode SQLite fingerprints stream every
selected value from the accepted root and descendant `session`, `message`, and
`part` cluster in stable table and row order. This detects a content change even
when row counts and saved timestamps do not change.

Durable evidence stores the verified parent-source fingerprint separately from
the aggregate analysis fingerprint, which can include child transcripts. The
source identity must match the discovery claim before publication. Startup
reconciliation requeues old ready evidence when these identities differ. This
repairs stale transcript evidence through native reindexing; it does not enroll
historical YAML assessments.

## Desktop Refresh

The desktop watcher requests a scoped refresh when a native source changes.
A metadata poll also checks active native file sessions every five seconds.
It compares file size and modification time because a writer can keep a file
open without a watcher notification. The poll waits fifteen seconds when no
native file session is active. It stops refresh work when discovery is paused.
Changed paths use the existing scoped refresh queue and admission limits.
The full scan remains the fallback for inactive files and WSL sources.
Refresh timing does not change accepted source formats or check eligibility.

## Remote Copies

The Linux x64/ARM64 helper discovers only Claude Code and Codex and exports
their accepted `ClaudeJsonl` and `CodexRolloutJsonl` evidence. Remote support
does not add a source format or widen the producer/version contracts below.
The desktop distinguishes hosts with immutable IDs, analyzes explicitly located
private cache files, and keeps native and WSL discovery separate.

Each listing returns up to 200 supported sessions from seven days, ordered
newest first within the examined set. Entry, candidate, byte, and elapsed-time
budgets can truncate discovery before every candidate is examined. Listings skip
expired transcripts before preview reads. Codex exports use bounded first-record
reads to retain linked children even when a child's modification time is older
than the parent or the listing window.
Listing absence does not prove deletion. Exported transcript, child, fork-parent,
and supported sidecar inputs must pass the helper's association and
descriptor-relative admission checks. Symlink components and non-regular files
are rejected. An incomplete companion search cannot replace a cached bundle.
Missing or rejected companions remain unavailable or partial; the desktop must
not substitute this computer's files or current configuration. A synced session
is a cached copy, not evidence of current remote activity or local quota use.
See [remote sessions](remote-sessions.md) for setup, transfer bounds, and retention.

## Review Scope

The reviewed targets are OpenCode, Pi, Codex, Claude Code, and Antigravity.
Their accepted source contracts and exact unsupported checks are recorded here
and in the [confirmation ledger](check-coverage.md#confirmation-ledger).
Cursor has characterized stored surfaces and the separate ordered YAML
tool-result snapshot route. Its other surfaces and other agents retain their
listed support limits; broader work has no assigned release.
Dedicated reader registration alone does not establish usable session analysis.

## Source Matrix

The table lists all 35 `SourceFormat` names from
`crates/antiburn-local/src/analysis/evidence.rs`, each exactly once.

Parsed content in a row does not by itself authorize a live event. The
[public-event contracts](#public-messages-and-live-event-eligibility) and
[host boundary contract](#host-session-boundaries) define finality and first
eligibility for the five native live routes. Other source rows retain their
existing stored-analysis scope, except the separate
[Cursor Desktop tool-result snapshot contract](#cursor-desktop-yaml-tool-evidence).

Journal page membership does not establish source-history coverage. Missing
optional metadata or bounded request history can leave safe results usable;
sequence discontinuity and unknown identity, order, or main-session ownership
remain unsafe. See [OpenCode journal delivery](#opencode-journal-delivery) for
atomic publication, baseline, restart, and recovery limits.

`OpenCodeSqliteV1` keeps the historical serialized value
`open_code_sqlite_v2` and old saved watch spelling `OpenCodeSqliteV2`. Both map
to V1. True V2 uses the distinct `open_code_v2_sqlite` value and
`OpenCodeSqliteV2` source row; its V2 reader and bounded live event route stay
separate from the V1 parser and journal cursor.

### Native OpenCode database locations

Both native SQLite source rows below use the same path rules. The default is
`opencode.db` under each supported OpenCode data root, including the platform
equivalent of `~/.local/share/opencode`. Discovery also reads sibling
`opencode-<channel>.db` files. Each root has a 256-directory-entry limit and a
32-channel-database limit. A failed root does not make an unrelated healthy
source unavailable.

`OPENCODE_DB` overrides the database filename and limits discovery to the first
data root. An absolute value selects that path. A relative value resolves under
that root. `:memory:` disables persisted database discovery; it does not expose
the running process's in-memory database. These path rules do not establish
support for a database's schema or producer version.

### OpenCode V2 source limits

OpenCode 2 releases use the V2 reader by default. The `v2.0.32` producer pin
below defines the characterized data shape, not a release allowlist. Native
stored reads validate the SQLite schema. Live capture, current config reads,
and WSL exports accept other 2.x releases when their data shapes and identity
checks pass. Readers accept extra fields only where their shape checks permit
them; WSL export envelopes remain fixed. Missing required fields, unsafe
bindings, and unsupported shapes remain unavailable.
This policy does not extend clean-result or remediation capabilities.

Current OpenCode inventory uses a verified registered V2 consumer receipt,
not the transcript generation. A report captures each CWD once outside its
store snapshot and reuses the resulting inventory or its unavailable result.
An older V1 transcript cannot prove which config generation runs now.

Once the native index selects V2 for a session ID, it remembers that choice
across deletion and restart. Deleting the V2 row does not restore its retained
V1 migration copy. New V1-only IDs remain eligible. **Clear index…** in Settings →
Privacy removes the saved selection along with local session evidence.

Native discovery groups V2 children by `parent_id` and keeps forks separate.
It reads fork parents from `fork_session_id`, not titles or transcript text.
Archived rows remain available for stored analysis. Native row deletion removes
that source. Session metadata keeps the exact native CWD, including later moves.
Source preference checks candidate IDs in one pass with shared database reads.
It keeps valid discovered sources when another database fails. An older V2 copy
still takes precedence over a recent V1 copy. Direct lookup and title lookup use
the same first-healthy-V2, then first-healthy-V1 policy.
Title refresh queries only requested IDs, in batches of 256, and retains at most
1 MiB of title text per database read. An oversized V2 title does not restore a
V1 title.

Native V1-only, V2-only, and mixed discovery first preserves recent session-header
sources. It adds message/part-only activity when the bounded metadata scan
completes. If that scan reaches its reserved limit, known recent-header sources
remain visible. An old session changed only through a message can stay omitted
while that limit persists; there is no eventual backfill guarantee. Returned
activity times then come from session headers. In a mixed database, a malformed
V1 or V2 header row does not discard usable sources from the other family.
Migrated V1 copies remain excluded even when V2 header decoding fails. A row
error with no usable sibling source still fails discovery. Required metadata
still must fit the hard budget; cancellation, schema, query, and budget failures
do not become partial success.

Fast native admission reads at most 256 related sessions, eight recent message
headers per session, and 1 MiB of metadata. It does not read message bodies.
Strict reconciliation hashes mutable rows, deleted rows, descendants, archive
state, and database file identity. Fork hashes also include the exact external
parent-boundary sequence that the parser uses to exclude copied work. They do
not include unrelated parent usage. Strict hashing rejects work above 500,000
rows, 256 MiB total, or 8 MiB per row. These limits do not guarantee a fixed SQL
execution time. Schema, source selection, and admission metadata stop after five
million SQLite VM instructions, or at the next progress check after one second
per bounded work chunk. Discovery shares one five-million-instruction, one-second
budget across V1 and V2. Optional activity scans leave one million instructions
and 200 ms for required work in that budget. V2 admits at most 1,000 recent session
candidates before root collapse. An invalid parent chain skips only that candidate;
V2 parent chains permit at most 32 links. V1 has no separate candidate count cap
and keeps its ancestry policy within the shared budget. A malformed V2 schema
still blocks V1 fallback within that database.
These bounds do not promise discovery of unlimited history. Progress checks run
every 1,000 instructions and observe discovery cancellation. Strict
hashing allows 25 million instructions and ten seconds. SQL projections guard
combined row sizes before returning fields, and these reads limit SQLite values
to 9 MiB. SQLite busy waits have a 250 ms limit. File-system waits can exceed the
progress-check interval.

The pinned V2 snapshot contract covers public message types, direct tool inputs,
outer Code Mode results, and recorded nested activity. It includes bounded child
sessions and excludes copied fork work when the native boundary resolves.
Malformed row JSON makes the result partial without discarding later valid rows.
Compaction usage contributes once. Durable rows retain native variants and keep
reasoning tokens separate from visible output tokens. Session aggregates that do
not match retained assistant usage remain explicitly unallocated. Costs retain
their native calculated-cost provenance and are not billing claims. These facts
do not establish a complete accounting result. No companion output files are read.

Unavailable shell output and migration-cleared tool output remain partial in
stored selected content, even when the native tool completed. The program exit
state stays separate from that content limit. Typed file-read ranges require the
returned filename to match the requested path. An alternate filename can leave
the text useful but does not prove which requested file lines were read.

Native live V2 checks use captured native event order, not stored message order.
A cold attachment writes an empty result-seed marker and starts from future
native evidence; it can separately emit the host `attached` boundary. Cycles
also need a verified baseline and later observed execution start. Request history
before attachment stays partial. Disconnects reset only the
affected selection segments and cancel their pending work. Stored assessments
retain their status and saved input, but capture gaps invalidate unverified public
message and cycle bindings. Tool-result validity needs its separate exact
reconciliation; accepted idle history keeps its recorded boundary. Corrections
and removals can still invalidate affected evidence. A matching receipt can drain already captured work after its
lease expires; this does not report a live connection. D/S/O/C findings
and prompts have partial support and cannot produce clean results. Model and
worker edits require an exact current-consumer target. O verification stays
unavailable because saved evidence does not prove the provider API route. M/B/K
resource findings remain unsupported.

Recency gates new or cold V2 admission. A compatible owned capture can finish
an open step or non-terminal call after activity expires. It retires when that
work ends. Saved capture rows and completed-call duplicate fingerprints alone
do not keep an idle source admitted.
Execution success, failure, or interruption releases unfinished V2 steps and
calls, including retry steps without a step terminal. This cleanup creates no
tool result and keeps accepted evidence.

Before a production session enters the local index, its CWD must resolve to a
Git repository. When a file transcript's CWD is a parent folder of
repositories, the scan reads the first 2 MB of the transcript, probes at most
eight folders below that CWD, and uses the repository that holds the most
distinct touched folders as the session CWD. The count is folders, not edits.
The scan maps linked worktrees to the canonical main root and rejects missing
or unresolved CWDs. The Sources setting "Include folders without git"
(`includeNonRepoFolders`, off by default) keeps a session whose CWD resolves to
no repository under its recorded CWD, with no repository root. The inference
and this setting apply only when Git reports that the CWD is not in a
repository; any other Git failure rejects the session. The ignored-path
set still applies to that CWD. A disabled repository is rejected when either its CWD or
its canonical root is in the existing ignored-path set. A scan pass that
rejects a session, or keeps one as a folder, writes a `scan_repo_gate` debug
event with the count for each reason.
Newly discovered repositories remain enabled by default.

| `SourceFormat`                 | Agent         | Native source                                                                                                                                                                                                                                                                                                     | Discovery and framing                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   | Parsed facts                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                | State                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| ------------------------------ | ------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ClaudeJsonl`                  | Claude Code   | `~/.claude/projects/<workspace>/*.jsonl`; Claude Desktop Cowork `<app-config>/Claude/{local-agent-mode-sessions,claude-code-sessions}/**/.claude/projects/<workspace>/*.jsonl` (at most four levels for `**`; observed with Claude Desktop 2.2553.1 and embedded Claude Code 2.1.275); `audit*.jsonl` is excluded | Native discovery; bounded JSONL with source claims and resume. Fixture-bounded child sidecars need a unique `toolUseId` join. Retained-root/YAML result shapes accept CLI 2.1.278 and 2.1.309, including the exact linear bridge. Private producer; no public schema pin.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               | Usage, including one-hour/five-minute cache writes, token classes, time, models, request controls/routes, calls, observed resource injection, thread links, compactions, exact Task/Agent child pairing, quota/provider incidents, and conditional question/recorded-plan metadata. Native results preserve UUID/block and physical record positions, unique direct-parent call binding, explicit error state, and completeness. User-slot context can include API input without human authority. Read returned extents remain pinned to 2.1.278.                                                           | Characterized subset, not a historical release range. Known lifecycle-only records are inert; unknown evidence-bearing records deny clean. Parent/inherited-result, replay, partial-write/resume, and frozen-prefix claims need exact source proof. Cowork metric parsing does not grant CLI live eligibility.                                                                                                                                                                                                                                                                                                               |
| `CodexRolloutJsonl`            | Codex         | `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`                                                                                                                                                                                                                                                                    | Native discovery with child rollouts; bounded JSONL; resume supported; recorder source pin defines `session_meta`, `turn_context`, `event_msg`, `response_item`, ordinal, and `compacted` rows; legacy reverted forks use a bounded metadata/timestamp boundary. YAML result/context contract uses synthetic 0.160.1 at `d27764b82f7118f674371e6d6e76271d9d606edb`.                                                                                                                                                                                                                                                                                                                                                                                                                                     | Per-response usage and context window, time, models, provider/control inheritance, service tier, tools, harness version, spawn records, selected skill documents, exact tool-search MCP exposure, compactions, quota and provider incidents from `task_complete` errors. Recognized linked response-item outputs and accepted completed command/file-change items retain native line order, available status, and completeness. Complete retained root `user.text` supplies context; UI echoes are not extra requests.                                                                                      | Characterized accepted core; protocol lifecycle echoes are inert; ambiguous fork boundaries remain partial. Output/completed-item, cross-projection deduplication, call/result/status, and retained-prefix claims need exact-version evidence.                                                                                                                                                                                                                                                                                                                                                                               |
| `OpenCodeJsonl`                | OpenCode      | Legacy exported session JSONL                                                                                                                                                                                                                                                                                     | Native persisted export or WSL CLI export; bounded JSONL with validated history wrappers/order                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          | Usage, time, models, provider/API fields where saved, task proof, selected skills, tools, compactions, session/message identities; cache episodes use validated order and distinct message IDs, with a five-minute default Anthropic lifetime unless a prior write records one-hour TTL evidence; that lifetime carries across hits, which refresh it at request start                                                                                                                                                                                                                                      | Characterized accepted export; WSL is not disk-only; variant labels do not prove effort or speed; no resource inventory                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| `OpenCodeSqliteV1`             | OpenCode      | `~/.local/share/opencode/opencode.db` or platform equivalent                                                                                                                                                                                                                                                      | Legacy metrics read `session`, `message`, and `part` snapshots. Native YAML live delivery reads bounded recent `event`/`event_sequence` input, then indexed suffixes, under the fixture-backed 1.18.35 contract at `53d1eabb61e21162157817bf677da0a4ad3332e3`. Cursor, outbox, and attachment state persist atomically. Live integrity uses bounded identity/boundary checks; strict replay retains full-prefix validation. See [journal delivery](#opencode-journal-delivery).                                                                                                                                                                                                                                                                                                                         | Native messages, parts, task/child models, selected skills, usage, provider/API fields, compactions, and tool errors. Journal results retain exact ownership, mutation order, first-terminal position, corrections/removals, and frozen request context. Synthetic user text is excluded; mixed messages keep real parts. Cold request history is partial. Page absence does not prove removal or whole-session absence. Missing optional content can remain usable; unsafe identity/order/ownership cannot.                                                                                                | Synthetic characterization, not live certification, CoreV2 `session_message`, or a historical release range. Live reads do not audit all old payloads. Detected boundary changes require recovery; strict replay rejects consumed-prefix rewrites. Corrections reuse retained frozen context; evicted context remains unavailable. Missing journals cannot use timestamp/row-ID fallback.                                                                                                                                                                                                                                    |
| `PiV3Jsonl`                    | Pi            | `~/.pi/agent/sessions/<workspace>/*.jsonl`, with the agent root redirected by `PI_AGENT_DIR` or `PI_CODING_AGENT_DIR`; deeper nested directories are not scanned                                                                                                                                                  | Native discovery; version 1, 2, and 3 headers with documented read-time metric migrations; bounded JSONL; resume supported. YAML result/context uses synthetic core V3 0.84.4 at `b79e4cc834970cca69daebffab7df1da7d1e52c4`; V1/V2 metric migration does not establish live eligibility.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                | Usage at the nested request-start timestamp, top-level event time, provider/API/model, agent-selected thinking policy paired with positive same-record usage, branch/fork state, tools, links, compactions, and cache episodes on continuous native routes. Empty zero-usage aborts update branch-local selection but add no effort or token evidence. Reviewed example-extension workers are finding-only. Native V3 results preserve physical order, exact call/tool/parent joins, status, and completeness. User-slot context needs no human-origin proof; branch and partial-record gaps limit context. | Characterized migrated core; bounded legacy migration overflow is partial. Live support needs the exact V3 contract; metric migration does not admit V1/V2 live checks.                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| `OmpV3Jsonl`                   | Oh My Pi      | `~/.omp/agent/sessions/**/*.jsonl`, renamed by `PI_CONFIG_DIR`, or a default-profile `PI_CODING_AGENT_DIR` inside the OMP root                                                                                                                                                                                    | Native discovery. The reader drops the fixed-width 256-byte `type: "title"` prologue, then requires an exact version 3 header. The shared Pi-family reader accepts `message` with role `user`/`assistant`/`toolResult`/`bashExecution`, `model_change`, `thinking_level_change`, `compaction`, and `custom` with `customType` `tool_execution_start`/`session_exit`/`todo_hud_state`/`user_todo_edit`/`goal-completed`. It also admits `title_change`, `credential_pin`, `ttsr_injection`, and `session_init` as signal-free housekeeping rows that keep their thread link. The `<skills>` listing in a `session_init` system prompt is startup context. A `read` of a `skill://<name>` path is skill use. Bounded JSONL and resume are supported. Named profiles and XDG redirects are not discovered. | Usage at the nested request-start timestamp, top-level event time, provider/API/model, agent-selected thinking policy, tools, links, and compactions from the allowlisted core                                                                                                                                                                                                                                                                                                                                                                                                                              | Only the characterized OMP core is supported. Startup context lists skills only, from subagent journals that have `session_init`. OMP records no tool or MCP definitions, and top-level journals have no `session_init`. The reader does not recognize `custom` and housekeeping rows with a shared parser signal, other `customType` values, Pi-only rows, other OMP record types (including `custom_message`, `model_usage`, `service_tier_change`, and `reset_boundary`), or pre-v3 headers. It does not resolve in-file branches to an active leaf or open sibling subagent files. D/T/O findings only; no clean result. |
| `CursorJsonl`                  | Cursor        | In-memory or compatibility JSONL without a source marker                                                                                                                                                                                                                                                          | Dedicated reader with bounded JSONL; native surface is unknown                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          | Generic Cursor role, content, timestamp, model, tool call, and record ID fields                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             | Uncharacterized compatibility format                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| `CursorCliAgentJsonl`          | Cursor        | `.cursor/projects/*/agent-transcripts/**` with chat metadata                                                                                                                                                                                                                                                      | Native discovery; transcript and metadata synthesis; bounded JSONL reader; subagent paths provide an explicit parent observation; the JSONL export is independent from the store contract                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               | Role, text/thinking/tool input/tool result content, timestamps, models, tool calls, redacted-block handling, and selected record IDs                                                                                                                                                                                                                                                                                                                                                                                                                                                                        | Partial; no model fallback or configuration inference                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| `CursorCliStoreDb`             | Cursor        | Legacy Cursor CLI `chats/**/store.db`                                                                                                                                                                                                                                                                             | Read-only database extraction into marked JSONL; reviewed `blobs(id,data)` and `meta(key,value)` subset                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 | Scalar user/assistant messages, title, workspace, timestamps, model, IDs, and fork-prefix hints; structured tool input is reduced during synthesis                                                                                                                                                                                                                                                                                                                                                                                                                                                          | Partial; synthesized content has no typed native tool evidence                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| `CursorChatStoreDb`            | Cursor        | Cursor chat `~/.cursor/chats/<workspace>/<session>/store.db`                                                                                                                                                                                                                                                      | Read-only database extraction; same reviewed `blobs(id,data)` and `meta(key,value)` subset, separate path contract; `subagentInfo.parentAgentId` is retained when present                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               | Scalar user/assistant messages and explicit child-parent metadata; structured tool input is reduced during synthesis                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        | Partial; synthesized content has no typed native tool evidence                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| `CursorIdeComposer`            | Cursor        | Workspace and global `state.vscdb` composer data                                                                                                                                                                                                                                                                  | Paired database discovery; composer headers and native bubbles read in one SQLite transaction and synthesized into marked JSONL                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         | Composer metadata, user/assistant text, exact bubble IDs, and the characterized `run_terminal_command_v2` arguments/result with accepted user-slot prefix                                                                                                                                                                                                                                                                                                                                                                                                                                                   | Partial overall. The separate [YAML snapshot result contract](#cursor-desktop-yaml-tool-evidence) requires recorded header order, exact identities, native main ownership, and current ready inline source evidence. No public-message/call/cycle/session event, requester/path, typed file-read, or historical-version claim.                                                                                                                                                                                                                                                                                               |
| `CursorLegacyChatJson`         | Cursor        | VS Code-family `chatSessions/*.json`                                                                                                                                                                                                                                                                              | Native file discovery; dedicated fail-closed profile                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    | No detector-grade fact contract                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             | Uncharacterized                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| `AntigravityJson`              | Antigravity   | Internal compatibility profile                                                                                                                                                                                                                                                                                    | Not emitted by current source classification                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            | Shared partial Antigravity JSON facts                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       | Internal profile; not a native source                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| `AntigravityBrainJsonl`        | Antigravity   | Brain transcript JSONL from CLI, IDE 2.0, or legacy paths                                                                                                                                                                                                                                                         | Native file discovery; bounded JSONL; truncated-field markers remain partial                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            | Step usage where present, timestamps, direct models including USER_INPUT setting changes, thinking, tool calls, and selected tool input                                                                                                                                                                                                                                                                                                                                                                                                                                                                     | Partial                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| `AntigravityCascadeJson`       | Antigravity   | API cascade or configured mirror JSON                                                                                                                                                                                                                                                                             | Native or configured file discovery; bounded whole-document parsing                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     | Nested steps, usage, timestamps, direct models, thinking, tool calls, and selected arguments                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                | Partial                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| `AntigravityWorkspaceChatJson` | Antigravity   | Workspace `chatSessions/*.json`                                                                                                                                                                                                                                                                                   | Native file discovery; dedicated fail-closed profile                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    | No detector-grade fact contract                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             | Uncharacterized                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| `AntigravitySqlite`            | Antigravity   | Native `conversations/<uuid>.db` with an optional sibling brain transcript                                                                                                                                                                                                                                        | Read-only transaction snapshot, including visible WAL rows; requires `PRAGMA user_version = 1`, reviewed `gen_metadata(idx,data)` or `steps(idx,metadata)` columns, and a private protobuf subset; companion fingerprinting                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             | Generation and step usage, retries, token classes, direct timestamps/model strings, companion user/assistant/tool content with companion usage suppressed; bounded joins retain exact response identities and conflicting model joins are partial                                                                                                                                                                                                                                                                                                                                                           | Partial; selected content needs an assessable companion; missing model/time stays missing; identity, enums, routes, and linkage remain incomplete                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| `CopilotCliJsonl`              | Copilot       | `~/.copilot/session-state/<uuid>/events.jsonl` plus sibling `session-store.db`                                                                                                                                                                                                                                    | Native CLI discovery; bounded JSONL; strict public v1 envelope, typed event graph, and schema-v7 read-only request store; source changes reject publication                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             | Shutdown model usage, selected model changes, request usage, and started/completed or failed subagent model relations; prompts, content, tool arguments, and results are not read                                                                                                                                                                                                                                                                                                                                                                                                                           | Characterized v1 event and schema-v7 bundle contract; no inventory, speed, request-depth, or cache-churn evidence                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            |
| `CopilotIdeChatJson`           | Copilot       | VS Code-family `chatSessions/*.json`                                                                                                                                                                                                                                                                              | Native file discovery; dedicated fail-closed reader                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     | No IDE-specific fact contract                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               | Uncharacterized                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| `ClineSessionJson`             | Cline         | Legacy Cline metadata JSON and message companion                                                                                                                                                                                                                                                                  | Metadata-only legacy source; message schemas are not pinned and the companion is not loaded as one analysis source                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      | No paired detector-grade fact contract                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      | Uncharacterized; fail-closed                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| `ClineMessagesContractV1`      | Cline         | `.cline/data/db/sessions.db` with root manifest and canonical message artifacts under `data/tasks/` or `data/sessions/`                                                                                                                                                                                           | Read-only SQLite snapshot includes WAL rows; requires exact `sessions` columns including `agent_id`, terminal root/child rows, matching root manifest, canonical agent-named paths, and child origin joins                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              | Terminal assistant timestamps/models/token classes, tool names, direct root/child scope, and child models; message text, prompts, tool payloads, results, paths, and secrets are discarded                                                                                                                                                                                                                                                                                                                                                                                                                  | Characterized v1 bundle; S/O findings only; no clean, request depth, inventory, effort, speed, or cache claim                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| `KiroSessionJson`              | Kiro          | Canonical workspace-session JSON                                                                                                                                                                                                                                                                                  | Native file discovery; dedicated fail-closed reader                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     | No canonical detector-grade fact contract                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   | Uncharacterized                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| `KiroChat`                     | Kiro          | `.chat` fallback                                                                                                                                                                                                                                                                                                  | Native file discovery; dedicated fail-closed reader                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     | No fallback detector-grade fact contract                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    | Uncharacterized                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| `KiroCliV2Bundle`              | Kiro CLI V2   | `~/.kiro/sessions/cli/<uuid>.json` plus matching `.jsonl`                                                                                                                                                                                                                                                         | Both exact UUID siblings are required. Metadata requires `session_state.version = "v1"`; journal requires only V1 `Prompt`, `AssistantMessage`, and `ToolResults` envelopes. `.history` is ignored; `.lock` is liveness only.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           | Model identity, safe token fields, tool names, and a child `parent_session_id`; no prompt, message, tool payload, path, or permission retention.                                                                                                                                                                                                                                                                                                                                                                                                                                                            | Characterized V2 fixture contract; D and S are unavailable, C is unsupported, and clean is disabled.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| `KiroCliV3Bundle`              | Kiro CLI V3   | `~/.kiro/sessions/<workspace>/sess_<uuid>/session.json` plus `messages.jsonl`                                                                                                                                                                                                                                     | Separate directory discovery requires both files; both files are included in one source fingerprint. The producer has not published a stable `session.json` contract, so parsing fails closed.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          | No detector-grade fact contract                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             | Uncharacterized and fail-closed                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| `KiroChatSaveExport`           | Kiro CLI      | Manual `/chat save` JSON export                                                                                                                                                                                                                                                                                   | Public docs confirm a user-chosen JSON export path but do not define its JSON schema. It is not scanned or parsed.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      | No detector-grade fact contract                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             | Unsupported manual export shape                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| `KiroCrewJsonl`                | Kiro Crew     | `~/.kiro/crew/sessions/<name>.jsonl`                                                                                                                                                                                                                                                                              | One JSONL file per session; `cron_*`, lock, and `.index/` files are skipped. First line is a `{"_type":"metadata",…}` header; remaining lines are `{"role",…,"content",…}` records.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     | Model identity per assistant turn; no per-turn token usage, no prompt, message, or tool payload retention.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  | Uncharacterized; usage-free reader                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| `AmpThreadJson`                | Amp           | `threads/*.json`                                                                                                                                                                                                                                                                                                  | Explicit full-export JSON only; requires envelope version 39, matching `threadId`, ordered messages, and bounded field sizes; file-change artifacts are separate                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        | Direct assistant usage, total/max input context, model, timestamp, tool names, and activated skill identities; no child-model inference or cache-churn claim                                                                                                                                                                                                                                                                                                                                                                                                                                                | Characterized v39 export; D/O findings only; no clean result and no S/C support                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| `AmpFileChanges`               | Amp           | `file-changes/**/*.{json,jsonl}`                                                                                                                                                                                                                                                                                  | Native fallback discovery                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               | File changes only                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           | Not a session                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| `WindsurfWorkspaceJson`        | Windsurf      | Workspace chat JSON                                                                                                                                                                                                                                                                                               | Native file discovery; dedicated fail-closed reader                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     | No workspace detector-grade fact contract                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   | Uncharacterized                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| `WindsurfMirrorJson`           | Windsurf      | Configured mirror JSON                                                                                                                                                                                                                                                                                            | Configured file discovery; dedicated fail-closed reader                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 | No mirror detector-grade fact contract                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      | Uncharacterized                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| `WindsurfCascadeProtobuf`      | Windsurf      | Cascade `.pb` data                                                                                                                                                                                                                                                                                                | Discovery walks `~/.codeium/windsurf/cascade`; no bounded protobuf session parser                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       | No parsed session facts                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     | Uncharacterized                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| `DevinLocalSqlite`             | Devin         | `~/.local/share/devin/cli/sessions.db`                                                                                                                                                                                                                                                                            | Read-only WAL-visible transaction; requires migration 17 and the reviewed `sessions`, `message_nodes`, `subagent_heads`, and `tool_call_state` columns; one source per `sessions.id`; active path follows `main_chain_id`; freshness fingerprints include all reader inputs                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             | Timestamped messages, models, deduplicated tool calls, and exact `run_subagent` child relations when child agent ID, child chain node, and actual child model agree; ACP schema 6 is optional child-only context                                                                                                                                                                                                                                                                                                                                                                                            | Partial; S findings only, no D/C or clean result                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| `Uncharacterized`              | Unknown agent | Generic JSONL fallback                                                                                                                                                                                                                                                                                            | No native source contract; bounded generic framing                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      | No detector-grade fact contract                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             | Uncharacterized                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| `OpenCodeSqliteV2`             | OpenCode      | Core V2 `session_v2` / `session_message` database                                                                                                                                                                                                                                                                 | Read-only snapshot of the pinned shape, including its unique session sequence index; OpenCode v2.0.32 producer commit `843a6c514e606b436a7fe713422ace0a93ad3d61`. Caps at 100,000 rows, 128 MiB total row data, and 8 MiB per row; a cap makes coverage partial. Live reads use the registered local `/api/event` feed only when the server process has the selected database open. Admission is native, main-session, recent-activity, exact-CWD Live scope, and limited to one bound database and 256 active sessions; larger scopes fail closed.                                                                                                                                                                                                                                                     | Bounded public text, per-step model and variant, separate input/output/reasoning/cache token classes, native calculated cost, and direct terminal results after the per-session `event_sequence` boundary. Aggregate usage not assigned to retained messages stays unallocated. Request history before attach, missing attachments, and missing result content stay partial. The feed has no replay; disconnects and overflow publish a gap. Outer `execute` stays one result; nested activities do not become results.                                                                                     | Partial D/S/O/C findings and prompts for the pinned schema; no clean-result support. Model and worker edits need an exact current-consumer target. O verification is unavailable because saved evidence does not prove the provider API route. M/B/K remain unsupported. Stored costs are not billing claims. Unsupported schemas remain unavailable. WSL uses the separate transfer format below.                                                                                                                                                                                                                           |
| `OpenCodeV2SessionExport`      | OpenCode      | V2 `SessionTransfer.Data` JSON from a registered WSL service                                                                                                                                                                                                                                                      | OpenCode v2.0.32 producer commit `843a6c514e606b436a7fe713422ace0a93ad3d61`; existing Linux curl and `service.json` under an absolute WSL command-environment `XDG_STATE_HOME/opencode` or the default `.local/state/opencode`. Shared registration validation requires numeric loopback and matching PID/version. GET requests reject redirects and proxies and stream into bounded pipes. Selects at most 100 exports from 500 metadata rows. Caps at 16 MiB/export, 100,000 messages/export, 32 MiB/batch including session-list responses, 15 seconds/request, and 60 seconds/V2 batch. At most four exports share a streaming byte budget; pending requests do not reduce another response's 16 MiB limit. Each reader also has an 8 KiB scratch buffer and bounded HTTP-status lookbehind.        | Bounded stored public text and direct tool content through a dedicated partial transfer reader. Validates required session fields and all 11 pinned message tags. Completed exports survive later export errors and the export deadline if the final service identity checks pass. No detector-grade facts or native live-result order.                                                                                                                                                                                                                                                                     | Bounded WSL stored-session support, not a complete history or hierarchy claim. V1 discovery also runs and retains V1-only history. No service auto-start, offline V2 database read, or native live eligibility. State-root lookup reads the WSL command environment, not the running service's process environment. Synthetic producer-shaped and command-harness tests; real WSL runtime is not verified on the macOS test host.                                                                                                                                                                                            |

Claude Desktop Cowork (agent mode) sessions use the same `ClaudeJsonl`
format. `<app-config>` is `~/Library/Application Support` on macOS,
`%APPDATA%` on Windows, and `$XDG_CONFIG_HOME` or `~/.config` on Linux.
Discovery reads each nested `.claude/projects` root whose `.claude` directory
is at most four directory levels below
`<app-config>/Claude/local-agent-mode-sessions` or
`<app-config>/Claude/claude-code-sessions`. The observed layouts are
`<org>/<account>/local_<workspace>/` and
`<org>/<account>/agent/local_ditto_<id>/`. Discovery treats each nested root
like `~/.claude/projects`: it reads the top-level `*.jsonl` files of each
project directory, and `<session>/subagents/agent-*.jsonl` files only promote
their parent. A desktop manifest `cliSessionId` also resolves against these
roots. The path gives the IDE/Desktop surface, whatever the `entrypoint` marker says.
Discovery never reads `audit*.jsonl`. The Cowork audit log repeats the
transcript's `message.usage` objects and is not a token ledger. For the
desktop watcher, a nested transcript write is session activity; audit logs,
manifests, and other files in these trees only schedule a rediscovery. The
synthetic `cowork_transcript.jsonl` and `cowork_audit.jsonl` fixtures
characterize these shapes from a key-only observation of Claude Desktop
2.2553.1 with embedded Claude Code 2.1.275. That install wrote no local
transcript for ordinary Claude Desktop chat, so ordinary chat is not
discovered. A Cowork session enters the local index only when its `cwd`
resolves to a Git repository, like every other session.

These local Cowork transcripts cover older sessions only. Since 16 Sep 2026,
Claude Desktop chat and Cowork share one chat box, and a chat can become a
Cowork session. From 6 Oct 2026, new Cowork tasks on Pro and Max plans run in
the cloud. A cloud Cowork task writes no local transcript. Claude Desktop
2.2553.1 wrote only
`<app-config>/Claude/local-agent-mode-sessions/<org>/<account>/remote-session-spaces.json`,
which holds a cloud session ID and the allowed folders but no messages or
token usage. Discovery does not read this file, so cloud Cowork sessions are
not discovered. Their usage appears only in the Claude provider limits. It is
not confirmed whether Team and Enterprise Cowork tasks still run locally.

The Claude API-error fixture also characterizes quota limit families and reset
clocks in `isApiErrorMessage` text. Session-limit and weekly-limit messages
retain their family, hour, minute, and named time zone without retaining the
message text. The desktop resolves the clock against the incident timestamp;
a missing or unusable reset does not erase the observed refusal. This shape
adds no clean-result eligibility. See the quota incident contract in
[check coverage](check-coverage.md).

## Provider Routes

These rows describe recorded coding-agent inference routes used by local analysis.
They are separate from the desktop decision connection. **Settings → Checks →
Model-based checks** selects Jev, compatible Ollama, Cloudflare, or Custom for YAML
assessments. Custom supports direct System One and Cloudflare-envelope responses.
Decision-level YAML `model` is an unknown field. Observed coding-agent model
fields and filters remain source facts; they do not route the assessment.
Language support and selected-connection capabilities are separate gates. See
[decision connections](burn-check/authoring.md#decision-connections) for verified
output support and unknown limits. Provider choice does not widen source
admission or authority. See the [supported subset](burn-check/authoring.md#supported-subset)
for the fields that YAML checks can select and send. Private thinking stays local.

Provider identity, API shape, and model identity are separate facts. A model
name alone does not establish option or accounting semantics.

Lifecycle execution metadata reads the provider and model from the same newest
published modeled turn. It keeps harness identity, recorded provider, canonical
route, and model-family vendor separate. Pi's `openai-codex` route normalizes to
`openai`; intermediary routes never become the model vendor. Missing or custom
routes do not use a harness or model fallback for HUD provider sweeps. This is a
consumer of existing durable evidence, not new parser or check coverage. See
[`session-lifecycle-events.md`](session-lifecycle-events.md#scoped-sweep-evidence).

| Agent and format          | Provider evidence                                                                      | API evidence                                                                                                                      | Model evidence                                                   | Current policy state                                                                                                                                                                  |
| ------------------------- | -------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Claude Code JSONL         | Explicit provider/API retained when present; absent pair uses the reviewed fixed route | `anthropic` / `messages`                                                                                                          | Request model and parent-call/actual child models                | Core controls/accounting assessable on reviewed routes; explicit unknown or incomplete routes do not use the fallback                                                                 |
| Codex rollout             | `session_meta.model_provider`, thread settings, and explicit inherited fork state      | `openai` / `responses` reviewed route                                                                                             | `turn_context` and request model fields                          | Request changes retain their own route and controls; custom or invalid providers fail closed                                                                                          |
| OpenCode JSONL and SQLite | Assistant `providerID` retained in durable rows                                        | Optional API retained; direct OpenAI, Anthropic, and Google provider IDs use their reviewed native API only for model remediation | Assistant `modelID`                                              | Anthropic cache-write and OpenAI uncached-input accounting on compatible history; arbitrary variants remain unsupported effort                                                        |
| Pi V3                     | Assistant provider retained per request                                                | Native assistant API retained in durable rows                                                                                     | Assistant model and branch-local model/policy changes            | Reviewed agent-selected policy and compatible-request accounting; cache-miss episodes require complete native route and identity; missing routes are not copied from an earlier model |
| Cursor formats            | No complete provider route is retained                                                 | No API contract is characterized                                                                                                  | Some records and metadata retain model names                     | Model aliases and complete request coverage remain partial or unknown                                                                                                                 |
| Antigravity formats       | No complete provider route is retained                                                 | Installed 2.11.0 descriptor subset is researched; no complete persisted route contract                                            | Direct model strings exist; private numeric enums are incomplete | D/O findings only where facts exist; reviewed native C is unsupported                                                                                                                 |
| Other formats             | No reviewed route contract reaches evidence                                            | Unknown                                                                                                                           | Partial names can appear in generic data                         | Fail closed                                                                                                                                                                           |

Pi T uses `EffortSemantics::AgentSelectedPolicy`. Its saved level is not the
provider's final effort after model maps or overrides. Reviewed provider/API
pairs in `model_catalog.rs` are `openai` with `responses`, `openai-responses`,
or `openai-completions`; `openai-codex` with `openai-codex-responses`;
`anthropic` with `messages` or `anthropic-messages`; and `google` with
`generate-content` or `google-generative-ai`. Each still needs a reviewed model.
Google cache policy is not reviewed. Native API recognition does not authorize
custom providers or prove provider-translated effort.
Pi's above-cap effort evidence requires positive token usage on the same
model/effort observation. An explicitly zero-token empty aborted attempt is
ignored; missing usage leaves effort assessment incomplete and cannot produce a
finding or clean result.

OpenCode and Pi C use persisted provider/API fields in `TurnRow` and the shared
compatible-request query. Unknown routes, mixed accounting, missing linkage,
compactions, or incomplete history prevent clean. OpenCode uses validated
ordered history, not `parentID` as a fabricated predecessor link.

The route columns use engine turn migration 7 and desktop migration 39. Current
revisions are parser 67, analyzer 25, evidence schema 22, coverage schema 6, and
resume snapshot 25. Existing revision gates invalidate old projections and
snapshots; JSON and binary
evidence round trips and full/resumed replay are covered by tests.

Discovery carries the selected `SourceFormat` and surface identity with each
source descriptor. Readers use that metadata rather than reclassifying a raw
path after discovery. SQLite readers fingerprint rows visible through the live
connection, including uncheckpointed WAL rows, and compare again after a
transaction snapshot completes.

Claude Code, the VS Code extension, and the Claude Desktop Code tab all write
`~/.claude/projects`. The scan reads the Claude surface from the first
`entrypoint` marker in the first eight transcript lines. `claude-desktop`,
`claude-vscode`, other IDE markers, and Cowork's `local-agent` markers give
`ide_desktop`; other markers give `cli`. A transcript without a marker uses
the path, which gives `cli`. A transcript in a Claude Desktop session tree is
always `ide_desktop`, because Desktop's embedded Claude Code can write an SDK
or CLI marker.

The evidence accumulator retains at most 16,384 distinct thread UUIDs. Each UUID
must be at most 256 bytes. A new UUID after the set is full, or an oversized
UUID, makes attribution incomplete and records `Partial(CapExceeded)`. Resume
deserialization rejects an oversized set or UUID. Defensive reconstruction also
caps invalid in-memory resume state and keeps the evidence partial.
The retained evidence memory ceiling is 8 MiB per accumulator. A synthetic
16,384-record linked chain with maximum-length identities verifies complete
coverage and resume round trips within that ceiling.

## Companion Sources

| Agent       | Companion                                                                                   | Current use                                                                                        | Required contract                                                                                                                                                                                                                             |
| ----------- | ------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Claude Code | `subagents/agent-*.meta.json` and child transcripts                                         | Exact Task/Agent call ID to unique `toolUseId` pairing; actual child models                        | Scan cursors and analysis/publication fingerprints include sidecar presence and bounded content. Changed claims are rejoined on resume; source changes reject publication. Missing, invalid, duplicate, or nested-parent claims stay partial. |
| Codex       | Child rollout files                                                                         | Discovery relates owned child rollouts                                                             | Preserve parent, child, model, effort, and speed inheritance.                                                                                                                                                                                 |
| Codex       | `state_5.sqlite` thread data                                                                | Not part of the current rollout reader contract                                                    | Use only thread-scoped historical rows with a stable schema and snapshot contract.                                                                                                                                                            |
| OpenCode    | V1 SQLite `session`, `message`, and `part` rows; V2 `session_v2` and `session_message` rows | Read in separate versioned snapshots; V1 task metadata must agree with child ancestry and model    | V2 uses a separate pinned reader, capture epoch, and event route. It does not enter the V1 SQLite reader or reuse its journal cursor.                                                                                                         |
| Pi          | Fork source named by the version 3 header                                                   | Removes inherited usage while retaining explicit policy state; branch links select their own state | Fork ancestry is not delegation. Unresolved ownership remains partial.                                                                                                                                                                        |
| Pi          | Official example-extension nested `toolResult` messages in the session                      | Passive parsing of actual worker models and native call identity                                   | Finding-only; no installation or execution of the extension, no clean for arbitrary extension output.                                                                                                                                         |
| Cursor      | Chat metadata, workspace metadata, and paired `state.vscdb` databases                       | Used during synthesis                                                                              | Fingerprint every contributing source and preserve structured data instead of display-only text.                                                                                                                                              |
| Antigravity | Sibling brain transcript                                                                    | Paired and fingerprinted with native SQLite                                                        | Preserve its tool facts and define database/transcript ownership rules.                                                                                                                                                                       |
| Antigravity | History metadata and spawn-edge data                                                        | History enriches discovery; spawn edges do not reach evidence                                      | Prove passive provenance, fingerprinting, delegation meaning, and both models before check use.                                                                                                                                               |
| Cline       | Messages-contract-v1 database, root manifest, and canonical child artifacts                 | Loaded as one validated source for v1; legacy metadata remains separate and fail-closed            | Pair every consumed artifact and the session-scoped SQLite rows before publishing claims.                                                                                                                                                     |

Current configuration can support a current-state assessment of a model,
compaction setting, or resource inventory. It cannot prove what a historical
request exposed unless the session records the inputs needed to select that
catalog entry. Keep current-state and historical claims separate.

The desktop also has a bounded read-only advisory resource inventory for native
Claude Code, Codex, Cursor, Copilot, Cline, OpenCode, Kiro, Amp, Antigravity,
and Devin/Windsurf contexts. It returns logical MCP server and skill candidates
with current enabled state, global or project scope, provenance, and explicit
limits. It returns no path, selector, writable target, or historical exposure.
A displayed unused resource also needs a supporting failed session. Skill
candidates can carry a proportional estimate for listing frontmatter only. The
body is not read for the estimate. This inventory is not session evidence.

The reviewed current inputs are standard Claude user and project MCP files,
`.claude/skills`, skill overrides, and exact permission controls; trusted Codex
user and project `mcp_servers` layers plus `.agents/skills` and compatibility
`.codex/skills`; OpenCode JSON/JSONC `mcp` and `mcp.servers` shapes, standard
skill roots, `tools`, and permission controls; and Pi `defaultTools`, standard
skill roots, explicit non-pattern skill directories, and
`pi-mcp-extension` 1.5.0. Copilot uses `~/.copilot/mcp-config.json`, project
`.mcp.json`, and `.github/mcp.json` with its top-level `servers` map. Cline uses
its reviewed MCP settings and canonical skill roots. These are current-state
inputs, not historical session evidence. Pi MCP activation additionally requires an installed
manifest with the exact package name, version, and `./src/index.ts` extension.
Its reviewed producer is commit
`8a01fc53f3289d2e8eb492d67ba45cd84d64e7f2`.

Known candidates survive malformed or dynamic unrelated inputs. The inventory
records those inputs as limits. Indexed `SessionEvidence` can add observed MCP,
skill, and catalog-backed built-in candidates, including candidates from a
partial observed subset. It never turns that subset into current enablement or
historical completeness.

The native 30-day report reduction builds a separate bounded resource
assessment. It reads positive use from tool counts, invoked loaded sources,
catalog-backed tool definitions, and persisted initial-context source rows.
Positive facts from partial evidence can suppress an exact candidate. Partial
evidence cannot prove non-use or a clean result. The reducer retains at most
4,096 distinct positive-use identities, 4,096 resource turn groups, 4,096
measured resource-session entries, 512 candidates and unused targets per
detector, three supporting sessions per target, 256 repository roots, and 256
distinct agent, working-directory, and repository inventory contexts. A cap
blocks clean but does not remove retained findings.

Global use applies only to the same agent. Project use applies only to the
canonical accessible repository root selected by longest path containment.
Unknown source origin does not create a scoped fallback target. When a raw call
has no source origin, a same-name project candidate in that repository takes
precedence over the global candidate. Same-name resources in different scopes
or repositories remain separate.

The pinned positive identities are case-insensitive exact resource names;
`mcp__<server>__<tool>` for Claude Code and Codex; `<server>_<tool>` for
OpenCode; and the default `mcp_<sanitized-server>_<tool>` shape from
`pi-mcp-extension` 1.5.0. Claude Code and Codex additionally accept one unique
bare suffix for a namespaced skill and the final segment of a catalog-backed
built-in alias. Ambiguous skill aliases suppress every possible matching target
and block clean. An ambiguous OpenCode or Pi MCP call suppresses all possible matching server
findings and blocks clean without counting a server as used. OpenCode and Pi use
exact skill and built-in identities.

The Checks payload and target action command use this assessment for M/B/K
status, counts, named targets, and estimates. Skill listings replicate only the
frontmatter estimate across applicable turns. MCP estimates require measured
indexed definition tokens. Built-in estimates use measured Claude Code and
Codex definitions or pinned OpenCode and Pi catalog captures. Measured resource
attribution is preferred. When a category has findings but measured attribution,
its denominator, or arithmetic is unavailable, the report uses the detector's
bounded finding-rate fallback. This is an estimated workload share, not measured
tokens or price evidence. Zero findings never create a fallback.

Resource Auto Fix requires both indexed provenance and one exact current editor
resolution for the same agent, name, kind, scope, value, and physical key.
Inventory-only and unresolved targets remain prompt-only. Exact M/B/K prompts do
create durable action attempts and references. Copying leaves the check Failing;
marker activation makes it Awaiting even though verification and savings remain
unavailable. A successful resource Auto Fix keeps a crash-recovery record whose
verification and savings states are unavailable. It does not claim that later
evidence will verify the change.

Historical session subsets cannot verify M/B/K absence or recurrence. The
engine accepts only a complete, bounded later current inventory with matching
agent, source, scope, and use coverage. The desktop remediation assessment path
currently supplies historical subsets, not that inventory, so M/B/K
verification is unavailable in the product.

For native sessions, the desktop can add nullable model and reasoning remediation
metadata when it publishes `Ready` evidence. This applies to `ClaudeJsonl`,
`CodexRolloutJsonl`, `OpenCodeJsonl`, `OpenCodeSqliteV1`, and `PiV3Jsonl` as the
vendor matrix allows. It hashes the physical setting and saves the effective
scope and value only when complete model evidence matches the effective setting.
Claude Code and Codex require their reviewed fixed routes. OpenCode and Pi
require the saved provider and model route. Reasoning also requires the exact
saved level. Codex project attribution requires explicit trust and resolves
reviewed `.codex/config.toml` layers from the repository root through the
session CWD. Untrusted workspaces and unsupported precedence store no
attribution. Native Windows can store attribution but cannot apply a change.
WSL stores no native attribution. This metadata describes publication-time
configuration. It is not session evidence or historical truth.

Model and reasoning Auto Fix remains pinned to this publication-time scope and
physical target. Current resolution must match both before prepare and apply.
An inherited setting therefore edits its global or user winner, while an exact
explicit project setting edits that project winner. Scalar controls are not
batched across layers. Findings from different projects group when they resolve
to the same global target, and every grouped project context is revalidated.
The editor never creates a project config.

Fast-mode remediation does not use publication-time configuration attribution.
It needs explicit persisted fast-tier session evidence and an existing current
winning Claude `fastMode = true` or Codex `service_tier = "fast"` target. Model
names, variants, labels, and latency do not qualify. Claude writes
`fastMode = false` to the one winning control rather than removing the key.

The resolver rejects runtime, environment, managed, remote, dynamic,
split-route, malformed, and ambiguous winners. Cursor and Antigravity
configuration remains separate from their accepted session contracts, so it
cannot attribute a historical setting. See the
[config attribution contracts](check-coverage.md#config-attribution-contracts).

K remediation uses the finding's exact skill identity and complete invocation
coverage. Current accepted sources do not retain a durable skill path. The
editor therefore derives a path only when one current standard `SKILL.md`
definition wins under the vendor's documented configuration roots. A missing or
ambiguous definition makes Auto Fix unavailable. The edit changes only the
vendor control and never deletes or changes `SKILL.md`.

Claude B remediation uses one exact name from the
[optional built-in allowlist](check-coverage.md#evidence-boundaries). A project
target requires the bare canonical name in that project's exact
`permissions.allow` array. Otherwise an inherited tool resolves to global
settings. This is the only approved remediation path that can create a missing
global config, and it never creates a project config. This editor behavior does
not expand the accepted `ClaudeJsonl` history or B finding eligibility.

The winning evidence-publication transaction enrolls at most 100 exact
passive T, O, and F findings. Enrollment starts only after desktop schema V45 is installed;
the V45 migration does not scan or infer attempts from older evidence. The
publication time in milliseconds is the immutable verification boundary, so a
historical session first published after rollout cannot become a retroactive
win. A losing claim publishes no attempt. A replay reuses the active durable
target and does not move its boundary. This work reads the bounded published
evidence and normalized turn rows. It does not read prompts, target-list state,
or window state, and it does not add a source scanner.

When no trusted workspace identifies a target, the remediation fallback scope
hash includes both the agent and session ID. Equal session IDs from different
agents cannot share a fallback target identity.

The evidence worker alternates ready evidence and remediation work when both
queues have work. Each publication and verification pass keeps its existing
bound. A winning publication can inspect its normalized user-content rows for a
bounded exact remediation marker. This activates only the matching copied-prompt
attempt at the publication boundary. It does not retain markers in evidence,
analytics, diagnostics, or derived finding facts. Restart recovery uses the
database rows; it does not rescan retained or deleted transcripts to reconstruct
attempts or contributions.

## Agent Memory

Claude Code auto-memory under `~/.claude/projects/<slug>/memory/` is
inventoried from disk. The scan reads `*.md` files and the `MEMORY.md` index
with lenient frontmatter and index parsing, and it caps bytes per file, files
per project, and bytes per project. Usage facts come from `Read`, `Write`,
`Edit`, `MultiEdit`, and `Bash` tool inputs that the turn content store already
holds; sessions from remote hosts are not counted. Windows-style paths (`C:\`, `C:/`, Git Bash `/c/` and `~` forms) are
matched by shape. No Windows session has been checked against this yet, so
Windows usage facts are a best guess. A `Bash` reference matches path text in
the command, so a command that builds the path from variables or a relative `cd` is
not found. The index injection attachment is not parsed. Claude Code prunes old
transcripts, so a memory with no recorded call has unknown history, not no use.
Session detail lists the memories that one Claude Code session referenced or
wrote, from the same tool-input evidence. An index entry is any bullet that
links a `.md` file, and text may come before the link. A project whose folder
is gone shows a best-effort path built from the slug and the folders that still
exist. The Memories page shows the index entry, the frontmatter, and the body
of each memory as separate sources. Codex and Cursor have no file-based memory
and are not covered.

## Known Contract Gaps

- OpenCode WSL discovery launches child processes. V1 uses the OpenCode executable
  for bounded DB queries and exports. V2 uses existing Linux curl for bounded
  read requests to an already-running registered service and a separate
  characterized transfer format. Neither route is
  disk-only passive file access. The V2 limits above do not establish full WSL
  history, hierarchy, or native live-check support.
- Generic Cursor store projections do not supply typed native tool evidence.
  The separate [Cursor Desktop snapshot route](#cursor-desktop-yaml-tool-evidence)
  retains characterized `run_terminal_command_v2` arguments and results. Other
  fields retain the limits listed for their source format.
- Antigravity private protobuf parsing does not yet preserve stable request
  identity, numeric model enums, retry meaning, provider boundaries, or
  compaction semantics.
- No current M/B/K reader proves a full historical inventory. Observed subset
  completeness plus complete calls can support scoped findings, not clean. B
  Auto Fix uses the Claude Code source only when the finding provides one exact
  canonical tool name. The advisory inventory has no mutation path. The report
  and action integration can select an Auto Fix target only when indexed
  provenance and the existing exact mutation resolver identify the same current
  resource, scope, value, and physical key. Other targets remain prompt-only.
- Selected OpenCode skills and Codex skill documents are observed injection and
  invocation, not unused listing overhead. Claude M/K also remain subset-scoped.
- OpenCode CoreV2 `session_message` is not the `OpenCodeSqliteV1` table contract.
  V2 parser/helper facts do not inherit V1 evidence or check eligibility.
- OpenCode variants have no historical effort map. Pi policy is characterized
  as agent-selected, not translated provider effort.
- Cursor permits direct O findings; Antigravity permits D/O findings where
  facts exist. Both deny clean by source gate. Neither borrows a previous record's model/time;
  Cursor retains its explicit synthesized header-model behavior.
- Generic and fail-closed readers must not promote recognized-looking fields to
  detector-grade evidence.

## Pinned First-Tier Sources

- Claude Code's producer is private. Core assistant/user tool envelopes have
  public decoder fixtures at [claudex][claude-question-source] and
  [zaly][claude-read-source]; optional question/plan contracts are detailed above.
  The previously cited cclens `8246ffa3` URL returns 404 and its commit API returns 422. It is not an accepted pin and cannot support the former 2.1.220–2.1.246
  historical claim. Child `.meta.json` joins remain bounded to repository
  characterization fixtures, separately from main JSONL. A missing sidecar,
  missing worker model, ambiguous join, or unknown evidence-bearing record blocks
  clean results.
- Codex rollout rows are pinned to the public recorder at
  [`e7637306`][codex-recorder-source]. The source writes `session_meta`,
  `turn_context`, `event_msg`, `response_item`, ordinals, and `compacted` rows.
  Synthetic fixtures cover accepted rows and loss paths. Persisted config is
  not treated as historical session evidence.
- Pi V3 is pinned to the public session-format document at
  [`b2602be7`][pi-session-source]. It defines `responseModel`,
  `providerThinkingLevel`, diagnostics, cache buckets, `id`/`parentId`, and
  the separate legacy `firstKeptEntryId` and newer retained-tail compaction
  forms. `providerThinkingLevel` is not agent-selected effort.
- Cursor's legacy CLI store and chat store use different contracts. The reviewed
  chat source uses `~/.cursor/chats/<workspace>/<session>/store.db`; both use
  only the reviewed `blobs` and `meta` subset.
  Neither source establishes an effective model fallback, inventory, route, or
  IDE configuration contract.
- Antigravity CLI SQLite is pinned to agy 1.0.16 reverse-engineering and the
  descriptor-backed field subset in ccusage. The reader admits only
  `user_version = 1`; it does not claim protobuf fields outside the tested
  usage, model, timestamp, retry, and identity subset.

[claude-question-source]: https://github.com/futpib/claudex/blob/0ad5073179efbfcc9dd9d6a9c19cca4575431653/src/transcript/parser.test.ts
[claude-question-input-source]: https://github.com/TrafficGuard/typedai/blob/34139aec65bb70f7062cf7f92667c11ffde4fcb1/.claude/hooks/extract-qa.py
[claude-read-source]: https://github.com/folke/zaly/blob/5a113518e6b0790fa0a63d058c3b5e76f8858e55/packages/agent/test/claude.test.ts
[claude-plan-source]: https://docs.rs/vct-core/2.7.1/src/vct_core/session/claude.rs.html
[claude-question-interruption]: https://github.com/anthropics/claude-code/issues/81223#issuecomment-5080551532
[claude-question-free-text]: https://github.com/anthropics/claude-code/issues/81223#issuecomment-5127759342
[claude-plan-user-source]: https://github.com/anthropics/claude-code/issues/24302
[codex-recorder-source]: https://github.com/openai/codex/blob/e7637306bc9246a3e42e407cb94f96b7ed345e3e/codex-rs/rollout/src/recorder.rs
[pi-session-source]: https://github.com/badlogic/pi-mono/blob/b2602be77cb7b0de45dd616407fd210daa48aa75/packages/coding-agent/docs/session-format.md

## Update Rules

Update this document in the same change when any of these items changes:

- Agent discovery paths or source precedence.
- A `SourceFormat` variant or its classification rules.
- Framing, size limits, snapshot, streaming, or resume behavior.
- Parsed metrics, content, evidence, or unknown-record behavior.
- Companion discovery, pairing, fingerprinting, or ownership.
- Provider, API, model, option, or accounting extraction.
- Characterization fixtures or supported version ranges.

Update [`check-coverage.md`](check-coverage.md) in the same change when the
parsing change affects a check's finding, clean, partial, or unavailable state.

## Coverage Promotion Rule

A source is `Characterized` when it can support its documented scope. A source
is complete for clean results only when:

- Its source path and accepted shape are explicit through a schema, header, or
  pinned producer commit plus synthetic fixtures; release ranges are recorded
  where known.
- Discovery reaches the production reader with the expected `SourceFormat`.
- Framing and snapshot behavior are bounded and fail closed.
- Every contributing companion is paired and fingerprinted.
- Positive, negative, incomplete, malformed, and unknown-shape fixtures exist.
- Full and resumed reads are equivalent where resume is supported.
- Provider and model semantics are reviewed where parsing exposes controls or accounting.
- The corresponding rows in `check-coverage.md` match tested behavior.

Use `Partial` for a safe scoped result. Use `Uncharacterized` when the reader
does not support a claim. Agent characterization, resume, replay, and desktop
companion tests check behavior separately. The
[confirmation ledger](check-coverage.md#confirmation-ledger) records reviewed
source limits.
