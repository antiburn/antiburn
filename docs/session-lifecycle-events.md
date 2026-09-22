# Session lifecycle and event contract

This contract explains how scan facts become ordered session state and frontend
events. Use it when changing ownership, delivery, evidence guards, recovery,
or resource bounds across the scan, registry, projection worker, and webviews.
Parser and check support live in [session parsing coverage](session-coverage.md)
and [check coverage](check-coverage.md).

## Ownership

```text
scan / analysis / retention / pricing / commands
        |
        | compact typed observations (session_lifecycle::Observation)
        v
SessionRegistry actor (session_lifecycle.rs)
        |
        | canonical sequenced events (one broadcast bus)
        v
projection worker + bridge (session_projection.rs)
        |
        | frontend projections only
        v
Tauri events -> webviews
```

- Producers report facts (`Touched`, `Anonymous`, `Indexed`,
  `AnonymousCovered`, `RowChanged`, `Removed`, `IndexChanged`). They never
  emit a session event to Tauri. An audit test in
  `session_projection/tests.rs` enforces this.
- The registry actor owns lifecycle state, deduplication, deadlines, event
  order, and the versioned live snapshot.
- The projection worker owns enriched `ActivityEntry` reads (on the
  blocking pool, coalesced and bounded) and is the only Tauri emitter for
  session events. Its separate compact model loader returns execution evidence
  to the registry through a capacity-one result mailbox and acknowledgement;
  the registry, not the loader or bridge, derives scoped counts.

## Scan and publication boundaries

Discovery and evidence analysis run separately. The watcher sends bounded bursts to the
scan scheduler. Classification reports keyed or anonymous activity before scoped refresh
floors; store upserts supply incarnation and revision facts for `Indexed`. A scan does
not wait for transcript analysis.

The durable evidence queue feeds analysis workers. Only a winning
`Store::publish_projections` transaction reports `RowChanged` with the `analysis` facet.
Processing and failed passes do not report success. This invalidates compact execution
evidence and requests an enriched row, without changing activity timestamps. Global
Checks notifications remain separate. Analysis rebuilds published evidence from
stored turn facts and coverage, not only its in-memory fold. A published fence
identifies a visible row set and may be reused; execution reads also require the
writer revision and incarnation.
Snapshot and named-presence commands read registry memory, not SQL. For source
parsing and publication coverage, see the
[session parsing pipeline](session-coverage.md#pipeline-contract).

## Delivery and reporter classes

- `report_async(Observation).await` is the scan reporting path, including
  command-requested scan passes. Tokio `mpsc` waits for capacity. Only this
  path reports keyed existence, anonymous touches, and anonymous covers.
- `report(SyncObservation)` serves commands, retention, pricing, and analysis
  workers. Its type permits only `RowChanged`, `Removed`, and `IndexChanged`.
  It tries inbox capacity without awaiting. A full inbox uses a short spill
  mutex; this is not a lock-free or wait-free API.
- Spill cells merge facets and maximum epochs, incarnations, and revisions.
  Excess keyed removals become a broad removal. Excess row changes become
  one list invalidation. Neither operation establishes a live identity.
- Producers release Store guards before either report call. The actor holds
  no lock across an await and never waits for projection. Four analysis
  workers retain their incident-ingestion analytics and global Checks output.
- Each actor round services a compact model reply when available, expiry,
  one completed page (or retained recovery suffix), the spill,
  and a bounded inbox batch, then yields. A blocked admission retains its
  fact while pages, expiry, and spill continue. Failed pages retain their
  requirements and cursor, with a 2–30 second exponential retry interval.
  Admission, purge-walk, and startup-recovery reads share round-robin turns.
  There is one blocking page task at a time, outside registry and spill locks.
  A retained recovery suffix uses the next available page-service round before
  another read starts; capacity-blocked recovery permits admission reads.
  Page selection prunes expired pending entries. If pruning frees capacity,
  the next round retries the blocked fact even when no page is issued.
- Shutdown cancels pending sends and discards blocking-task results. A closed
  inbox returns promptly and logs rather than retaining more reports.

Startup silently seeds the registry before projection and scan start. Seed
pages carry row evidence but publish no events: a populated snapshot can
therefore have `seq = 0`. Launch indexing does not duplicate seeded starts.
A failed seed preserves its cutoff and last accepted cursor, then starts the
actor normally. The lifecycle read boundary retries after 2 seconds, then
4, 8, 16, and at most 30 seconds between consecutive failures. Successful
pages reset the shared page backoff. Unchanged launch discovery remains silent;
it is not the recovery mechanism.

After startup, recovered rows use current-state page matching and normal
revision/incarnation-guarded establishment, never silent seed insertion.
Persisted presence alone produces `Activity`, not `Started`; sequenced events
carry canonical aggregates so already-subscribed clients converge. Newer live
or pending incarnations reject old activity. Deletion guards, current activity
windows, and same-incarnation transient timestamps apply as for presence reads.
A full pending set retains the unaccepted suffix of one recovery page. The
cursor advances only after every row is accepted, deferred safely, or rejected
as stale. Exhaustion completes recovery; shutdown discards late read results.

The special repair path for early V49 databases applies the missing main
migrations through V51 in one transaction. It preserves existing session
incarnations and their counter. Failure rolls back the repair so the database
remains retryable at V49.

## Evidence, guards, and convergence

These numbers have different scopes and must not substitute for one another:

| Evidence       | Scope and meaning                                                                                                                                                                                                                                                      |
| -------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `Revision`     | Process-local `total_changes()` on the single writing connection, read under the Store mutex. Rows and their revision come from the same critical section. Rollbacks can leave gaps, never decreases. Read-only report/export connections cannot supply this evidence. |
| `Incarnation`  | Persisted row identity, stable on update and increasing across recreation of the same key. Clearing local session data keeps its counter. It remains internal; `SessionRef` is unchanged.                                                                              |
| `seq`          | Process-local canonical event order, assigned by the actor under the registry lock. Snapshot and presence reads include the completed batch at their sequence.                                                                                                         |
| `AnonymousGen` | Scheduler-local causal order for anonymous reports and pass covers, independent of activity timestamps.                                                                                                                                                                |

`TouchedSession` carries a key, incarnation, and lookup revision (`seen`).
Each `IndexedSession` carries its incarnation and activity epoch; its batch
carries the upsert revision. Presence pages return incarnation, epoch, and
revision together. `RowChanged` and `IndexChanged` never establish presence.
Rust and JavaScript lowercase only ASCII A–Z in WSL environment keys;
non-ASCII letters remain distinct.

Admission applies these rules:

1. Ignore activity outside the 180-second window.
2. For a live key, reject older incarnations. Within the same incarnation,
   retain the maximum revision and activity time, even for an older `seen`.
   A higher incarnation replaces the old one without importing its activity.
3. Reject an incarnation at or below the remembered deleted incarnation.
4. For a key not live, require its revision to be at least both
   `forgotten_through` and `broad_through`. Equality is safe. Otherwise retain
   it for a presence page. A full pending set blocks rather than drops it.
5. Merge pending activity only within the same incarnation. A newer page
   incarnation uses its own epoch; an older page cannot erase newer evidence.

`forgotten_through` retains the highest revision of an evicted deletion.
`broad_through` retains the highest broad-removal revision. An absent
entry in bounded deletion memory is not evidence that admission is safe.

`RemovalScope::One(key, incarnation)` carries the delete's returned identity
and revision. It cannot remove a newer live incarnation. `RemovalScope::Broad`
names no keys; a paged presence walk checks live entries whose existence
revision predates it. An absent page removes only evidence no newer than the
page. A requirement received during a walk survives for a later walk when
its revision is higher. Reasons are `deleted`, `purged`, `rejected`, and
`reconciled`; replacement by a higher incarnation uses `reconciled`.

Repository opt-out reports a broad purge, then index invalidation. Clearing
local data reports broad deletion, then invalidation, before requesting its
refill pass. Retention reports broad purge when it removes rows; that removal
itself triggers a list refetch, without a separate invalidation requirement.

The guarantee is guarded admission, no cross-incarnation activity import,
preservation of valid transient activity, and convergence after pending facts
and reconciliation complete. It is not instantaneous agreement with SQLite.
A delayed removal can leave a deleted incarnation visible until delivery or
reconciliation. Narration, deletion-memory eviction, and derived absence
records can depend on receipt order; exact equality of those memories is not
claimed. Idle expiry independently removes sessions outside the live window.

## Lifecycle state machine

```text
absent --first indexed--> working  (Started)
working --30 s no write--> quiet   (Quiet)
quiet --new write--------> working (Activity, resumed = true)
quiet --180 s total------> idle    (Idle; leaves the registry)
idle --later write-------> working (Activity, resumed = true)
```

`Quiet` means "not currently receiving writes". `Idle` means "no longer a
current session". Neither claims the provider process exited. A session is
_active_ (for lists and pills) while it is in the registry, working or
quiet. `resumed` is metadata on `Activity`, not a separate state. Recent-idle
memory affects narration only; it does not authorize admission after deletion.
Deadline wakes include one second of slack, and delivery can add delay.

Quiet paths still schedule rediscovery without reporting activity. In
particular, Claude Desktop manifest rewrites do not keep a session working;
transcript writes do. Title-only work changes row metadata, not liveness.

## Frontend events

Each of the three `session:*` payloads carries `seq`, the registry sequence.
Scan status and global Checks notifications do not. Sequences are global
across all three session scopes, so gaps between events on one scope are normal;
a lifecycle `resync` requests recovery from transport lag or an unsafe backlog.

| Event                                  | Payload                                                                                                | Meaning                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| -------------------------------------- | ------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `session:lifecycle`                    | `{seq, kind: started\|activity\|quiet\|idle\|anonymous_cleared\|sweep_changed\|resync, aggregate?, …}` | Lifecycle transitions, execution-evidence changes (`sweep_changed`), and resync metadata. `activity` carries `resumed`; a null `session` is anonymous agent-level activity the store has not indexed yet. `anonymous_cleared` carries `agent` and `cause: resolved\|expired`. The last lifecycle event of each atomic registry batch carries `aggregate: {working, total, anonymous, sweep}`, the exact counts after the batch; `resync` never does. |
| `session:updated`                      | `{seq, session, facets, entry}`                                                                        | One enriched row per coalesced registry update. `facets` name what changed (metadata, title, analysis, usage, checks, limits).                                                                                                                                                                                                                                                                                                                       |
| `session:index-changed`                | `{seq, cause: scan_pass\|invalidated\|removed\|resync, session?, removal?}`                            | List membership changed: refetch list data.                                                                                                                                                                                                                                                                                                                                                                                                          |
| `scan:finished` (and started/progress) | `ScanStatus`                                                                                           | Scan progress and status only. Not a freshness or liveness signal.                                                                                                                                                                                                                                                                                                                                                                                   |
| `checks:report-changed`                | none                                                                                                   | The global checks report changed. Stays global until checks become session-scoped.                                                                                                                                                                                                                                                                                                                                                                   |

A removal of a session the registry still tracked also publishes `idle`
on `session:lifecycle` before the `removed` index change, so working
indicators always hear an end.

The `usage`, `limits`, and `checks` facets are reserved: no producer sets
them yet. Consumers already refresh selectively on them, so the usage and
checks workers can adopt them without a listener change.

## Snapshot, counts, and named presence

`get_live_sessions(limit?)` returns `{seq, working, total, sweep, sessions,
anonymous}`. `sessions` holds at most `limit` (default 128) of the most
recent live sessions, each with `agent`, `lastActivityAt`, the registry's own
`quiet` flag, and nullable `execution` metadata. Named presence returns the
same row shape. No reader derives lifecycle windows from timestamps. `working`
and `total` are exact and independent of the row limit:
`sessions.length < total` means the rows are truncated, and an omitted identity
is unknown, not absent. `anonymous` is complete and bounded by the number of
agent kinds.

The registry maintains the global working count and reads total and anonymous
map lengths directly. Scoped `sweep` counts walk live identities and group
execution metadata under the registry lock. Each atomic mutation compares scoped
counts before and after; a snapshot also computes them. These aggregate
operations are not constant-time. A snapshot reads only completed batches. Row
selection clones the live map and sorts outside the lock; the payload limit does
not bound that temporary allocation or sorting work.

`get_live_sessions_for(sessions)` answers named identities: `{seq,
present, absent}`, all read under one registry lock at one sequence. At
most `MAX_ACTIVITY_ROWS` (500) identities per call; the shell rejects
more. Every window that may call `get_live_sessions` may call it
(`permissions/default.toml` for the default windows; `capabilities/main.json`
for the main window, which the `lib.rs` capability audit pins). Both commands
are registered in `app_commands.rs` with autogenerated permissions. Default
windows are popover, settings, nudge, onboarding, overlay, and HUD detail.
Main receives both explicit grants, without the broad default capability.
The old `get_latest_session_activity` command and permission are removed.

The reader protocol (implemented by `lib/sessionLifecycle.ts`):

1. Subscribe to `session:lifecycle` first. A failed attachment retries every
   five seconds. No snapshot makes the tracker ready before attachment succeeds.
2. Read the snapshot; buffer events that arrive while it is in flight.
3. Replace state with the snapshot only when `snapshot.seq` is at or above
   the already-applied lifecycle state; otherwise discard the stale snapshot.
   Then replay buffered deltas above the accepted base. Nothing below the
   base sequence applies.
4. Apply only events with `seq` above the base, and move a key only when
   the event is newer than that key's own evidence (its presence or
   absence sequence). Take counts from the highest-sequence stamp seen.
5. On lifecycle `resync`, repeat from 2. The bridge requests this for bus
   lag or a completion backlog that makes row emission unsafe. It does not
   allocate a canonical sequence or reconstruct lost facts.
6. Each list registers its rows as an interest after every load and removes
   them when it stops. Main-window lists also clear interests while hidden or
   section-inactive; the retained popover keeps its interests while hidden.
   After every accepted truncated
   snapshot, and on every interest change, the tracker asks the registry
   by name for the interests the base did not answer, one read in flight,
   the union chunked to the command's limit; a change during a read runs
   one more read after it. An answer below the base is discarded; a row
   merges when the key has no evidence or the answer is newer than its known
   evidence. A newly registered identity also rejects answers below the
   lifecycle watermark at registration. Removing the last owner removes that
   watermark; re-registration uses the current watermark. This prevents an
   old answer from crossing an idle event during interest churn without
   retaining absent identities after their owners leave. The watermark map
   contains only current interests. Absence evidence also lives only for
   registered interests. A failed read retries through the snapshot retry
   after five seconds. A complete snapshot needs no presence
   read. Stopping the tracker invalidates pending callbacks and retry timers.

### Unknown evidence and sequence zero

A silent seed can contain 130 live sessions while its snapshot returns only
128 rows at sequence zero. Omitted interests remain unknown until a named
presence answer arrives. Missing per-key evidence is distinct from known
presence or absence at zero: the tracker queries unknown interests even when
`baseSeq = 0` and accepts their present or absent answers at that same sequence.
Once the key has evidence, duplicate or older answers cannot change it.
A newer delta also prevents an older presence answer from overwriting the key.
For an omitted current interest, the tracker retains a quiet-transition sequence
without inventing a last-activity timestamp. An older present answer supplies
that timestamp but keeps the newer quiet state. An older absent answer cannot
replace the quiet evidence; a coalesced follow-up read resolves the identity.
Newer row evidence or an accepted newer snapshot replaces this transition.
Removing the last interested owner or stopping the tracker clears unresolved
quiet evidence. This memory contains only current interests.
The base-sequence and aggregate guards remain independent of per-key evidence.
A complete sequence-zero snapshot already answers every identity and needs no
presence read. Startup stays silent; this protocol requires no synthetic events
or sequence increments.

Pills: a row is active when the registry names it live, inactive when the
registry says it is not (a complete base, an `absent` answer, or an `idle`
for a registered row), and keeps its own flag while the registry has said
nothing. The HUD's working indicator reads `working` and `anonymous`
counts, never the rows.

## Anonymous activity

A watcher can report a write under an agent root before the store indexes
the session. The scan scheduler gives each such report an anonymous
generation from a checked monotonic counter and reports it as `Anonymous`.
The registry publishes it as `activity` with a null session, keeps the
highest generation per agent with its latest time, and shows it in the
snapshot. It cannot appear in a session list and receives no `quiet` or
`idle`.

Only the registry clears it, and it says so with `anonymous_cleared`:

- `resolved`: a scheduler-owned pass that started after the touch finished
  successfully. The pass captures the outstanding generations for its scope
  (every agent for a full pass, the burst's agents for a scoped pass) when
  it starts and reports `AnonymousCovered` after its last `Indexed` report,
  on the same ordered path. A cover clears an entry only when the entry's
  generation is at or below it, so a touch reported during the pass, in the
  same second, or after a clock rollback survives. A failed, busy, or
  cancelled pass, and a pass a command asked for, cover nothing.
- `expired`: the registry's own 30-second quiet deadline passed.

`started` never clears anonymous state, and the frontend keeps no timer for
it. After a clear, a touch at the same epoch is new activity again. A keyed
touch whose row the lookup no longer finds reports nothing; it is not
downgraded to anonymous activity.

## Projection worker

One injected `RowLoader` loads `ActivityEntry` rows on the blocking pool; one
`ProjectionEmitter` bridges all frontend session scopes. The bus remains
receivable while a load runs. Lifecycle transitions, including anonymous
clears, do not wait for row projection.

The first pending row fixes a 250 ms deadline; later rows do not postpone it.
Only one load runs. A second batch already due starts when that load finishes.
Removing the final pending row clears its deadline.

A keyed removal suppresses that row in flight. A newer update suppresses the
older projection and merges its facets into pending work. Broad removal,
index invalidation, overflow, and lag discard obsolete batches and pending
patches. A scan-pass membership change alone does not invalidate other rows.
Before completing a load, the worker receives up to 1024 queued bus results.
A larger remaining backlog requests recovery instead of emitting unsafe rows.

Missing rows cause one index invalidation after the found rows. Load failures
retain one bounded retry; another failure becomes one invalidation. A blocking
task panic follows the same failure path. Shutdown discards in-flight output.
Lag requests a lifecycle resync at the registry's current sequence and an
index refresh. Repeated lag in a cycle coalesces, but a gap above the earlier
watermark retains a follow-up recovery request. The bridge invents neither
canonical sequence numbers nor aggregate counts.

## Resource bounds and limits

| Resource                | Bound                                                               |
| ----------------------- | ------------------------------------------------------------------- |
| Ordered inbox           | `INBOX_CAPACITY = 256` observations                                 |
| Indexed report          | `INDEXED_CHUNK = 256` sessions                                      |
| Inbox work per round    | `DRAIN_BATCH = 64` observations; `DRAIN_WEIGHT = 256` session facts |
| Due deadlines per round | `EXPIRE_BATCH = 256`                                                |
| Each keyed spill map    | `SPILL_KEY_CAP = 1024` cells; overflow broadens the fact            |
| Broadcast ring          | `BUS_CAPACITY = 1024` events; lag remains possible                  |
| Deletion memory         | `DELETION_MEMORY_CAP = 1024`; eviction raises the guard             |
| Pending admission       | `ADMISSION_PENDING_CAP = 1024`; overflow blocks the caller          |
| Presence/seed page      | `RECONCILE_PAGE = 256`; one page in flight                          |
| Retained seed recovery  | At most one unaccepted page suffix, up to `RECONCILE_PAGE` rows     |
| Recent-idle narration   | `RECENT_IDLE_CAP = 256` keys                                        |
| Projection              | `PENDING_ROW_CAP = 512` pending and 512 in flight; one load         |
| Default snapshot        | `DEFAULT_SNAPSHOT_LIMIT = 128` returned rows                        |
| Named presence          | `MAX_ACTIVITY_ROWS = 500` references per call; sequential chunks    |
| Watcher burst           | `MAX_BURST_PATHS = 64`; overflow requests a full pass               |

The live map has no arbitrary cap. The deadline index is proportional to live
sessions and anonymous agents; compact metadata slots and their work queue also
scale with live identities. Scoped aggregate construction, snapshot cloning/sorting,
and filtering the live map for a purge page are proportional work, not
constant-time operations. Model reads contain at most `MODEL_PAGE = 256`
identities, with one model load and a capacity-one result mailbox; these bounds
do not cap the registry's metadata map.
Seed queries use the keyset index with `LIMIT` and no temporary sort. The
nested cursor predicate can scan already-visited ties at the cursor epoch;
a page limits returned rows, not necessarily examined index entries.

1. Anonymous activity whose answering pass fails, is busy, is cancelled, or
   cannot run remains working until the registry's 30-second expiry.
2. Narration can compress: page admission can use `Activity` instead of
   `Started`. Spill can omit an intermediate `Idle`; replacement can narrate
   `Idle` and reconciled removal before the old delete fact arrives.
3. Deferred admission waits for a successful page. Full pending admission
   plus failing pages backpressures scan. Presence-page failures log and retry;
   the storage-health banner tracks write failures, not every read failure.
4. Store/registry agreement is eventual. There is no hard delivery-lag bound.
5. Watcher bursts during scan backpressure retain their existing bounded
   overflow-to-full-pass behavior.
6. No end-to-end row-projection latency or freedom-from-broadcast-lag guarantee
   follows from the 250 ms batch deadline or the queue capacities.
7. Seed page count is at most `ceil(rows / 256) + 1` in a quiescent window.
   Concurrent inserts behind the cursor add work. Startup seeds before scan,
   so the normal seed is quiescent. Recovery retains the original cutoff and
   cursor, but tests activity against the current window. Rows that age out
   during failures are not made live. Concurrent writes during recovery can
   move rows ahead of its cursor; those writes report their own discovery.
   Persistent read failures delay recovery without stopping actor fact,
   spill, or expiry service. No fixed recovery completion time is promised.

No token or cost aggregation moves into this lifecycle pipeline.

## Consumers and tests

The HUD uses exact working, anonymous, and positive canonical-route sweep counts, not
bounded rows. Popover and main-window lists patch enriched rows from `session:updated`,
refetch membership on `session:index-changed`, and register visible rows as
named-presence interests for active pills. Main-window activity and overview clear
interests while hidden or section-inactive; the retained popover keeps its interests
while hidden. On resume, the main window overlays current registry evidence,
restores interests, and refreshes its active views. Popover usage refreshes on
lifecycle activity with a 30-second floor and an independent visible-only poll;
membership changes can refresh it immediately. Analysis, checks, and limits
refresh from their relevant facets. Settings refreshes app info on index changes.

Extend the protocol at its owner: producers add typed facts, the registry defines
ordering and evidence, the projection worker owns Tauri events and bounded rich-row
work, and frontend lists register only their current interests. A new scoped signal
needs an explicit provenance rule and sequence-safe snapshot/presence recovery. Do not
infer liveness from row timestamps or event delivery alone.

Use synthetic, deterministic schedules for delayed and reordered reports,
deletion/recreation, page failures, capacity pressure, lag, shutdown, and frontend
snapshot/delta races. The registry tests in `session_lifecycle/tests/`, projection tests
in `session_projection/tests.rs`, and tracker tests in `sessionLifecycle.test.ts` cover
these boundaries. Run the relevant Rust and desktop tests and the repository checks in
[CONTRIBUTING.md](../CONTRIBUTING.md).

## Scoped sweep evidence

`Aggregate` and `LiveSnapshot` carry `sweep` counts by harness. `SessionRef.agent`,
live-row `agent`, and `sweep.agent` name the coding harness, not the provider route or
model vendor. Each harness distinguishes named working, anonymous, pending-model,
failed-model, and unmodeled activity from positive execution groups. The model
categories sum to named working. Counts cover all canonical live identities, regardless
of snapshot row limits or list interests.

A positive execution group and a row's nullable `execution` carry four distinct facts:
`model` is the newest nonempty modeled turn in the published fence; `recordedProvider`
is the nullable provider from that same turn without alias rewriting; `providerRoute` is
its nullable canonical route; and `modelVendor` comes only from a recognized model
family. The route never falls back to harness or vendor. Counts group by harness and all
execution fields, so recorded aliases remain distinguishable. For example, Pi's
`openai-codex` route normalizes to `openai`, while Claude on OpenRouter, AWS, or Azure
retains that route with an Anthropic vendor. Vertex remains `google-vertex`; a custom or
absent provider has no canonical route. These facts identify recorded execution, not a
paid subscription or account. Historical spend attribution is separate.

Displayed provider meters use positive canonical route counts. Pending, failed,
unmodeled, missing-route, and anonymous activity remains global only. Anonymous activity
proves no provider or model. An account-wide meter can sweep for any working
model on its canonical route. A model-scoped meter sweeps only when the same
canonical route has a working published model that matches the meter's scope.

The registry retains compact execution metadata per live identity. Checked
tickets never reset after idle or re-admission. A broad invalidation advances an
epoch and immediately removes positive evidence. Selection walks at most 256
metadata slots before keyed work; model pages contain at most 256 identities and
retry failed reads after 2–30 seconds without delaying other due pages. One
blocking model read can run alongside one rich-row read. The worker continues
relaying lifecycle events during those reads and acknowledgements. The registry
retains model work until acknowledged, and the capacity-one result mailbox is
independent of blocked admission. A constant-size resolved counter detects broad
metadata invalidation without walking every slot in the actor.

The Store reads incarnation, published fence, newest nonempty model and same-turn
provider, and writer revision together. An answer requires matching incarnation, epoch,
and ticket plus sufficient revision; the fence alone cannot validate it because a later
publication may reuse that fence. The query skips unmodeled turns and does not
join historical provider hints or model runs. Missing rows establish no presence.
`sweep_changed` carries batch-final counts. Resolved metadata also advances the
sequence for quiet rows even when working counts do not change. Metadata never
changes activity timestamps. Invalidation removes execution immediately even if
reload fails.

The shared frontend tracker retains liveness and aggregate counts, not per-row
execution. A future row-level consumer must reconcile its bounded snapshot or named
presence on lifecycle events, including `sweep_changed`, with the same sequence and
interest guards. On explicit resync, the tracker clears scoped positives and rejects
scoped evidence before the recovery watermark. Presentation selectors use counts without
expiry timers.
