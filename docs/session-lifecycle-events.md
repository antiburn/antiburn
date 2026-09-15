# Session lifecycle and event contract

Current baseline for how session state flows from the scan pipeline to the
webviews. This documents the shipped contract, not a plan and not parser or
check coverage — see `docs/session-coverage.md` and `docs/check-coverage.md`
for those.

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
  session events.

## Store migration order

V48 retains the typed remediation attribution fields in `session_evidence`.
V49 adds the session `incarnation`, the increasing `session_incarnation_seq`
counter, and the `session_recency_keyset` index. Existing session rows receive
incarnation zero. Updates keep the incarnation; deleting and recreating a
session assigns a higher value. Clearing local session data keeps the counter.

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
*active* (for lists and pills) while it is in the registry, working or
quiet.

## Frontend events

Every payload carries `seq`, the registry sequence. Sequences are global
across all three scopes, so gaps between events on one scope are normal;
only `resync` means loss.

| Event | Payload | Meaning |
| --- | --- | --- |
| `session:lifecycle` | `{seq, kind: started\|activity\|quiet\|idle\|anonymous_cleared\|resync, aggregate?, …}` | Lifecycle transitions and resync metadata. `activity` carries `resumed`; a null `session` is anonymous agent-level activity the store has not indexed yet. `anonymous_cleared` carries `agent` and `cause: resolved\|expired`. The last lifecycle event of each atomic registry batch carries `aggregate: {working, total, anonymous}`, the exact counts after the batch; `resync` never does. |
| `session:updated` | `{seq, session, facets, entry}` | One enriched row per coalesced registry update. `facets` name what changed (metadata, title, analysis, usage, checks, limits). |
| `session:index-changed` | `{seq, cause: scan_pass\|invalidated\|removed\|resync, session?, removal?}` | List membership changed: refetch list data. |
| `scan:finished` (and started/progress) | `ScanStatus` | Scan progress and status only. Not a freshness or liveness signal. |
| `checks:report-changed` | none | The global checks report changed. Stays global until checks become session-scoped. |

Removed events: `sessions:entry-changed` and `sessions:invalidated` no
longer exist. Rows travel as `session:updated`; membership as
`session:index-changed`.

A removal of a session the registry still tracked also publishes `idle`
on `session:lifecycle` before the `removed` index change, so working
indicators always hear an end.

The `usage`, `limits`, and `checks` facets are reserved: no producer sets
them yet. Consumers already refresh selectively on them, so the usage and
checks workers can adopt them without a listener change.

## Snapshot, counts, and named presence

`get_live_sessions(limit?)` returns `{seq, working, total, sessions,
anonymous}`. `sessions` holds at most `limit` (default 128) of the most
recent live sessions, each with `agent`, `lastActivityAt`, and the
registry's own `quiet` flag, so no reader derives lifecycle windows from
timestamps. `working` and `total` are exact and independent of the row
limit: `sessions.length < total` means the rows are truncated, and an
identity the rows omit is unknown, not absent. `anonymous` is complete; it
is bounded by the number of agent kinds. The registry keeps the counts as
it applies each batch, so a snapshot never counts the map, and a snapshot
read sees only completed batches.

`get_live_sessions_for(sessions)` answers named identities: `{seq,
present, absent}`, all read under one registry lock at one sequence. At
most `MAX_ACTIVITY_ROWS` (500) identities per call; the shell rejects
more. Every window that may call `get_live_sessions` may call it
(`permissions/default.toml` for the default windows; `capabilities/main.json`
for the main window, which the `lib.rs` capability audit pins).

The reader protocol (implemented by `lib/sessionLifecycle.ts`):

1. Subscribe to `session:lifecycle` first.
2. Read the snapshot; buffer events that arrive while it is in flight.
3. Replace state with the snapshot only when `snapshot.seq` is at or above
   what the buffered deltas built; otherwise discard the stale snapshot.
   The accepted snapshot is the base: nothing below its sequence applies.
4. Apply only events with `seq` above the base, and move a key only when
   the event is newer than that key's own evidence (its presence or
   absence sequence). Take counts from the highest-sequence stamp seen.
5. On `resync` (transport lag between the registry and the projection
   worker; the registry itself never drops a fact), repeat from 2.
6. Each visible list registers its rows as an interest after every load
   and removes them when it stops. After every accepted truncated
   snapshot, and on every interest change, the tracker asks the registry
   by name for the interests the base did not answer, one read in flight,
   the union chunked to the command's limit; a change during a read runs
   one more read after it. An answer below the base is discarded; a row
   merges only when newer than the key's evidence; absence evidence lives
   only for registered interests. A failed read retries through the
   snapshot retry. A complete snapshot needs no presence read.

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

## Notes and limits

- A repository opt-out, retention, and clearing the index report a broad
  removal stamped with the store revision of the purge. The registry checks
  every live entry that predates that revision against the store, one
  bounded page at a time, and publishes `idle` then `removed` for each row
  that is gone; one `invalidated` index change follows so lists refetch.
  Until a page answers, the live snapshot can briefly name a purged
  session.
- Environment keys lowercase the WSL distro. Rust uses ASCII lowercasing
  and the webview uses locale-aware lowercasing, so a distro name with
  non-ASCII uppercase could split the pill overlay from its row. Known,
  vanishingly rare, and limited to the pill overlay.

## Consumers (current)

- HUD (`OverlaySession`): working indicator from the tracker's exact
  counts only, including anonymous activity until the registry clears it.
- Popover (`PopoverSession`): rows patch from `session:updated`; list and
  repositories refetch on `session:index-changed`; usage refreshes on
  lifecycle `activity` under a 30-second floor plus a visible-only poll;
  checks and limit allocations refresh selectively by facet; active pills
  come from the registry, with the listed rows registered as its interest.
- Main window (`MainActivitySession`): same pattern; analysis reloads when
  an update's facets touch the open session.
- Main window Overview (`MainOverviewSession`): the recent rows' pills come
  from the registry the same way; the page refreshes on index changes and
  on row updates that move its totals.
- Hygiene (`useSessionHygiene`): analysis/checks facets per session;
  index changes re-read the requested set.
- Settings (`SettingsWindowSession`): index changes refresh app info.
