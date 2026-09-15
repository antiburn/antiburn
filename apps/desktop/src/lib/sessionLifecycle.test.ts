import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type * as Ipc from "./ipc"
import { LIVE_PRESENCE_REQUEST_LIMIT } from "./ipc"
import {
  hasWorkingActivity,
  listInterests,
  LiveSessionsTracker,
  registryActivity,
  sessionRefKey,
  SNAPSHOT_RETRY_MS,
  withRegistryActivity,
} from "./sessionLifecycle"

const ipcMocks = vi.hoisted(() => ({
  getLiveSessions: vi.fn(),
  getLiveSessionsFor: vi.fn(),
  onSessionLifecycleEvent: vi.fn(),
}))

vi.mock("./ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof Ipc>()),
  getLiveSessions: ipcMocks.getLiveSessions,
  getLiveSessionsFor: ipcMocks.getLiveSessionsFor,
  onSessionLifecycleEvent: ipcMocks.onSessionLifecycleEvent,
}))

type LifecycleHandler = (event: Ipc.SessionLifecycleEventPayload) => void

let lifecycleHandler: LifecycleHandler | null = null

function ref(sessionId: string): Ipc.SessionRefPayload {
  return { environmentKey: "native", agent: "claude-code", sessionId }
}

function liveSession(
  sessionId: string,
  lastActivityAt: number,
  quiet = false,
): Ipc.LiveSessionPayload {
  return { session: ref(sessionId), agent: "claude-code", lastActivityAt, quiet }
}

function emit(event: Ipc.SessionLifecycleEventPayload): void {
  lifecycleHandler?.(event)
}

/** The counts the registry stamps on a batch's last lifecycle event. */
function counts(working: number, total: number, anonymous = 0): Ipc.AggregatePayload {
  return { working, total, anonymous }
}

/** A snapshot whose counts match its rows: nothing omitted. */
function completeSnapshot(
  seq: number,
  sessions: Ipc.LiveSessionPayload[] = [],
  anonymous: Ipc.LiveAnonymousPayload[] = [],
): Ipc.LiveSnapshotPayload {
  return {
    seq,
    working: sessions.filter((live) => !live.quiet).length,
    total: sessions.length,
    sessions,
    anonymous,
  }
}

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason: unknown) => void
  const promise = new Promise<T>((done, fail) => {
    resolve = done
    reject = fail
  })
  return { promise, resolve, reject }
}

/** The presence requests made so far, as identity keys per call. */
function requestedKeys(): string[][] {
  return ipcMocks.getLiveSessionsFor.mock.calls.map((call) =>
    (call[0] as Ipc.SessionRefPayload[]).map(sessionRefKey),
  )
}

beforeEach(() => {
  lifecycleHandler = null
  ipcMocks.getLiveSessions.mockReset()
  ipcMocks.getLiveSessions.mockResolvedValue(completeSnapshot(0))
  ipcMocks.getLiveSessionsFor.mockReset()
  ipcMocks.getLiveSessionsFor.mockResolvedValue(null)
  ipcMocks.onSessionLifecycleEvent.mockReset()
  ipcMocks.onSessionLifecycleEvent.mockImplementation(async (handler: LifecycleHandler) => {
    lifecycleHandler = handler
    return () => {
      lifecycleHandler = null
    }
  })
})

afterEach(() => {
  vi.useRealTimers()
})

async function startTracker(): Promise<{ tracker: LiveSessionsTracker; stop: () => void }> {
  const tracker = new LiveSessionsTracker()
  const stop = tracker.subscribe(() => undefined)
  await vi.waitFor(() => expect(tracker.getSnapshot().ready).toBe(true))
  return { tracker, stop }
}

describe("LiveSessionsTracker", () => {
  it("attaches the listener before it reads the snapshot", async () => {
    const order: string[] = []
    ipcMocks.onSessionLifecycleEvent.mockImplementation(async (handler: LifecycleHandler) => {
      order.push("listen")
      lifecycleHandler = handler
      return () => undefined
    })
    ipcMocks.getLiveSessions.mockImplementation(async () => {
      order.push("snapshot")
      return completeSnapshot(3, [liveSession("seeded", 100)])
    })

    const { tracker, stop } = await startTracker()

    expect(order).toEqual(["listen", "snapshot"])
    expect(tracker.getSnapshot().seq).toBe(3)
    expect(tracker.getSnapshot().sessions.get(sessionRefKey(ref("seeded")))).toEqual({
      agent: "claude-code",
      lastActivityAt: 100,
      quiet: false,
    })
    stop()
  })

  it("replays only the buffered events above the snapshot's sequence", async () => {
    let resolveSnapshot!: (value: Ipc.LiveSnapshotPayload) => void
    ipcMocks.getLiveSessions.mockReturnValueOnce(
      new Promise<Ipc.LiveSnapshotPayload>((resolve) => {
        resolveSnapshot = resolve
      }),
    )
    const tracker = new LiveSessionsTracker()
    const stop = tracker.subscribe(() => undefined)
    await vi.waitFor(() => expect(lifecycleHandler).not.toBeNull())

    // Both events land while the snapshot is in flight. The first is
    // already included in the snapshot's sequence; the second is newer.
    emit({ seq: 5, kind: "activity", session: ref("a"), agent: "claude-code", at: 100, resumed: false })
    emit({ seq: 6, kind: "quiet", session: ref("a"), agent: "claude-code", at: 130 })
    resolveSnapshot(completeSnapshot(5, [liveSession("a", 100)]))

    await vi.waitFor(() => expect(tracker.getSnapshot().ready).toBe(true))
    const session = tracker.getSnapshot().sessions.get(sessionRefKey(ref("a")))
    expect(session?.quiet).toBe(true)
    expect(tracker.getSnapshot().seq).toBe(6)
    stop()
  })

  it("discards an in-flight snapshot older than what events already built", async () => {
    const { tracker, stop } = await startTracker()
    emit({ seq: 10, kind: "activity", session: ref("a"), agent: "claude-code", at: 100, resumed: false })
    expect(tracker.getSnapshot().seq).toBe(10)

    // A resync forces a re-read, and the re-read resolves stale.
    let resolveSnapshot!: (value: Ipc.LiveSnapshotPayload) => void
    ipcMocks.getLiveSessions.mockReturnValueOnce(
      new Promise<Ipc.LiveSnapshotPayload>((resolve) => {
        resolveSnapshot = resolve
      }),
    )
    emit({ seq: 11, kind: "resync" })
    emit({ seq: 12, kind: "activity", session: ref("b"), agent: "claude-code", at: 200, resumed: false })
    resolveSnapshot(completeSnapshot(8))

    await vi.waitFor(() =>
      expect(tracker.getSnapshot().sessions.has(sessionRefKey(ref("b")))).toBe(true),
    )
    // The stale snapshot did not erase the state the deltas built.
    expect(tracker.getSnapshot().sessions.has(sessionRefKey(ref("a")))).toBe(true)
    expect(tracker.getSnapshot().seq).toBe(12)
    stop()
  })

  it("re-reads the snapshot on resync", async () => {
    const { tracker, stop } = await startTracker()
    expect(ipcMocks.getLiveSessions).toHaveBeenCalledTimes(1)
    ipcMocks.getLiveSessions.mockResolvedValueOnce(
      completeSnapshot(20, [liveSession("recovered", 300, true)]),
    )

    emit({ seq: 15, kind: "resync" })

    await vi.waitFor(() => expect(tracker.getSnapshot().seq).toBe(20))
    expect(ipcMocks.getLiveSessions).toHaveBeenCalledTimes(2)
    expect(tracker.getSnapshot().sessions.get(sessionRefKey(ref("recovered")))?.quiet).toBe(
      true,
    )
    stop()
  })

  it("follows working, quiet, resumed, and idle transitions", async () => {
    const { tracker, stop } = await startTracker()

    emit({
      seq: 1,
      kind: "started",
      session: ref("s"),
      agent: "claude-code",
      at: 100,
      aggregate: counts(1, 1),
    })
    expect(hasWorkingActivity(tracker.getSnapshot())).toBe(true)
    expect(tracker.getSnapshot().sessions.get(sessionRefKey(ref("s")))?.quiet).toBe(false)

    emit({
      seq: 2,
      kind: "quiet",
      session: ref("s"),
      agent: "claude-code",
      at: 130,
      aggregate: counts(0, 1),
    })
    expect(hasWorkingActivity(tracker.getSnapshot())).toBe(false)
    expect(tracker.getSnapshot().sessions.size).toBe(1)
    expect(tracker.getSnapshot().sessions.get(sessionRefKey(ref("s")))?.quiet).toBe(true)

    emit({
      seq: 3,
      kind: "activity",
      session: ref("s"),
      agent: "claude-code",
      at: 140,
      resumed: true,
      aggregate: counts(1, 1),
    })
    expect(hasWorkingActivity(tracker.getSnapshot())).toBe(true)
    expect(tracker.getSnapshot().sessions.get(sessionRefKey(ref("s")))?.quiet).toBe(false)

    emit({
      seq: 4,
      kind: "idle",
      session: ref("s"),
      agent: "claude-code",
      at: 320,
      aggregate: counts(0, 0),
    })
    expect(tracker.getSnapshot().sessions.size).toBe(0)
    expect(hasWorkingActivity(tracker.getSnapshot())).toBe(false)
    expect(tracker.getSnapshot()).toMatchObject({ working: 0, total: 0, anonymous: 0 })
    stop()
  })

  it("ignores an event at or below the sequence already applied", async () => {
    ipcMocks.getLiveSessions.mockResolvedValue(completeSnapshot(9))
    const { tracker, stop } = await startTracker()

    emit({ seq: 9, kind: "activity", session: ref("late"), agent: "claude-code", at: 100, resumed: false })

    expect(tracker.getSnapshot().sessions.size).toBe(0)
    expect(tracker.getSnapshot().seq).toBe(9)
    stop()
  })

  it("keeps anonymous activity until the registry clears it, with no local timer", async () => {
    vi.useFakeTimers()
    const tracker = new LiveSessionsTracker()
    const stop = tracker.subscribe(() => undefined)
    await vi.advanceTimersByTimeAsync(0)
    expect(tracker.getSnapshot().ready).toBe(true)

    emit({
      seq: 1,
      kind: "activity",
      session: null,
      agent: "codex",
      at: 100,
      resumed: false,
      aggregate: counts(0, 0, 1),
    })
    expect(tracker.getSnapshot().keylessAgents.has("codex")).toBe(true)
    expect(hasWorkingActivity(tracker.getSnapshot())).toBe(true)

    // No frontend timer decides lifecycle state: well past the registry's
    // window, the agent is still working until the registry says otherwise.
    await vi.advanceTimersByTimeAsync(10 * 60_000)
    expect(tracker.getSnapshot().keylessAgents.has("codex")).toBe(true)
    expect(hasWorkingActivity(tracker.getSnapshot())).toBe(true)
    expect(vi.getTimerCount()).toBe(0)

    emit({
      seq: 2,
      kind: "anonymous_cleared",
      agent: "codex",
      at: 130,
      cause: "expired",
      aggregate: counts(0, 0, 0),
    })
    expect(tracker.getSnapshot().keylessAgents.has("codex")).toBe(false)
    expect(hasWorkingActivity(tracker.getSnapshot())).toBe(false)
    expect(tracker.getSnapshot().seq).toBe(2)

    // A clear for an agent the tracker does not hold only moves the sequence.
    emit({
      seq: 3,
      kind: "anonymous_cleared",
      agent: "cursor",
      at: 131,
      cause: "resolved",
      aggregate: counts(0, 0, 0),
    })
    expect(tracker.getSnapshot().keylessAgents.size).toBe(0)
    expect(tracker.getSnapshot().seq).toBe(3)
    stop()
  })

  it("keeps anonymous activity across started and clears it when the pass covers it", async () => {
    const { tracker, stop } = await startTracker()

    emit({
      seq: 1,
      kind: "activity",
      session: null,
      agent: "codex",
      at: 100,
      resumed: false,
      aggregate: counts(0, 0, 1),
    })
    emit({
      seq: 2,
      kind: "started",
      session: { environmentKey: "native", agent: "codex", sessionId: "resolved" },
      agent: "codex",
      at: 101,
      aggregate: counts(1, 1, 1),
    })

    // The start alone says nothing about which anonymous touches the pass
    // accounted for; the registry's cover does.
    expect(tracker.getSnapshot().keylessAgents.has("codex")).toBe(true)
    expect(
      tracker
        .getSnapshot()
        .sessions.has(JSON.stringify(["native", "codex", "resolved"])),
    ).toBe(true)

    emit({
      seq: 3,
      kind: "anonymous_cleared",
      agent: "codex",
      at: 101,
      cause: "resolved",
      aggregate: counts(1, 1, 0),
    })
    expect(tracker.getSnapshot().keylessAgents.size).toBe(0)
    // Still working: the anonymous state resolved into a keyed session.
    expect(hasWorkingActivity(tracker.getSnapshot())).toBe(true)
    stop()
  })

  it("replaces anonymous state from the snapshot on resync", async () => {
    const { tracker, stop } = await startTracker()
    emit({ seq: 1, kind: "activity", session: null, agent: "codex", at: 100, resumed: false })
    emit({ seq: 2, kind: "activity", session: null, agent: "cursor", at: 100, resumed: false })
    expect(tracker.getSnapshot().keylessAgents).toEqual(new Set(["codex", "cursor"]))

    // The registry cleared codex while events were lost; the snapshot is
    // the canonical anonymous set now.
    ipcMocks.getLiveSessions.mockResolvedValueOnce(
      completeSnapshot(9, [], [{ agent: "cursor", lastActivityAt: 100 }]),
    )
    emit({ seq: 5, kind: "resync" })
    await vi.waitFor(() => expect(tracker.getSnapshot().seq).toBe(9))
    expect(tracker.getSnapshot().keylessAgents).toEqual(new Set(["cursor"]))
    expect(hasWorkingActivity(tracker.getSnapshot())).toBe(true)

    // A stale delta below the snapshot cannot bring codex back.
    emit({ seq: 4, kind: "activity", session: null, agent: "codex", at: 100, resumed: false })
    expect(tracker.getSnapshot().keylessAgents).toEqual(new Set(["cursor"]))
    stop()
  })

  it("retries a failed snapshot read and recovers on the retry", async () => {
    vi.useFakeTimers()
    ipcMocks.getLiveSessions
      .mockRejectedValueOnce(new Error("ipc failed"))
      .mockResolvedValueOnce(completeSnapshot(7, [liveSession("recovered", 100, true)]))
    const tracker = new LiveSessionsTracker()
    const stop = tracker.subscribe(() => undefined)
    await vi.advanceTimersByTimeAsync(0)
    expect(tracker.getSnapshot().ready).toBe(false)

    // Deltas keep applying onto the event-built state while the retry waits.
    emit({ seq: 3, kind: "activity", session: ref("interim"), agent: "claude-code", at: 90, resumed: false })
    expect(tracker.getSnapshot().sessions.has(sessionRefKey(ref("interim")))).toBe(true)

    await vi.advanceTimersByTimeAsync(SNAPSHOT_RETRY_MS)
    expect(ipcMocks.getLiveSessions).toHaveBeenCalledTimes(2)
    expect(tracker.getSnapshot().ready).toBe(true)
    expect(tracker.getSnapshot().seq).toBe(7)
    expect(tracker.getSnapshot().sessions.get(sessionRefKey(ref("recovered")))?.quiet).toBe(
      true,
    )
    stop()
  })

  it("stays not-ready without a shell so surfaces keep their fallback", async () => {
    ipcMocks.getLiveSessions.mockResolvedValue(null)
    const tracker = new LiveSessionsTracker()
    const stop = tracker.subscribe(() => undefined)
    await vi.waitFor(() => expect(ipcMocks.getLiveSessions).toHaveBeenCalled())
    await Promise.resolve()

    emit({ seq: 1, kind: "activity", session: ref("solo"), agent: "claude-code", at: 100, resumed: false })

    // Events still build state, but `ready` stays false: nothing proved a
    // registry exists, so pill overlays keep the backend row flag.
    expect(tracker.getSnapshot().sessions.has(sessionRefKey(ref("solo")))).toBe(true)
    expect(tracker.getSnapshot().ready).toBe(false)
    stop()
  })
  it("maps listed rows to interests and keeps their flags before the registry answers", () => {
    const rows = [
      { agent: "claude-code", sessionId: "a", wslDistro: null, isActive: true },
      { agent: "claude-code", sessionId: undefined, isActive: false },
      { agent: "codex", sessionId: "b", wslDistro: "Ubuntu", isActive: false },
    ]
    expect(listInterests(rows)).toEqual([
      { environmentKey: "native", agent: "claude-code", sessionId: "a" },
      { environmentKey: "wsl:ubuntu", agent: "codex", sessionId: "b" },
    ])
    const notReady = new LiveSessionsTracker().getSnapshot()
    expect(withRegistryActivity(notReady, rows)).toBe(rows)
    expect(registryActivity(notReady, sessionRefKey(ref("a")))).toBeNull()
  })
})

/** Many identities: more than the default snapshot row limit. */
function manyRefs(count: number, prefix = "s"): Ipc.SessionRefPayload[] {
  return Array.from({ length: count }, (_, index) => ref(`${prefix}-${index}`))
}

function present(session: Ipc.SessionRefPayload, quiet = false): Ipc.LiveSessionPayload {
  return { session, agent: "claude-code", lastActivityAt: 100, quiet }
}

describe("LiveSessionsTracker counts and presence", () => {
  it("reads exact counts from the snapshot and stamped deltas, not the bounded rows", async () => {
    // 130 live identities, all quiet in the 128 rows; the counts alone say
    // two work and two more are live beyond the rows.
    const rows = manyRefs(128).map((session) => present(session, true))
    ipcMocks.getLiveSessions.mockResolvedValue({
      seq: 10,
      working: 2,
      total: 130,
      sessions: rows,
      anonymous: [],
    })
    const { tracker, stop } = await startTracker()
    expect(tracker.getSnapshot()).toMatchObject({ working: 2, total: 130, complete: false })
    expect(hasWorkingActivity(tracker.getSnapshot())).toBe(true)
    expect(tracker.getSnapshot().sessions.size).toBe(128)

    // A delta below the base moves nothing, whatever it carries.
    emit({
      seq: 8,
      kind: "quiet",
      session: ref("s-0"),
      agent: "claude-code",
      at: 130,
      aggregate: counts(99, 99),
    })
    expect(tracker.getSnapshot()).toMatchObject({ working: 2, total: 130, seq: 10 })

    // A delta without a stamp leaves the counts; a stamped one moves them.
    emit({ seq: 11, kind: "quiet", session: ref("s-129"), agent: "claude-code", at: 131 })
    expect(tracker.getSnapshot()).toMatchObject({ working: 2, total: 130, seq: 11 })
    emit({
      seq: 12,
      kind: "quiet",
      session: ref("s-128"),
      agent: "claude-code",
      at: 132,
      aggregate: counts(0, 130),
    })
    expect(hasWorkingActivity(tracker.getSnapshot())).toBe(false)
    expect(tracker.getSnapshot()).toMatchObject({ working: 0, total: 130, seq: 12 })
    stop()
  })

  it("asks the registry by name for interests a truncated snapshot omitted, one read at a time, chunked", async () => {
    const rows = manyRefs(128).map((session) => present(session))
    ipcMocks.getLiveSessions.mockResolvedValue({
      seq: 10,
      working: 700,
      total: 700,
      sessions: rows,
      anonymous: [],
    })
    const first = deferred<Ipc.LivePresencePayload>()
    const second = deferred<Ipc.LivePresencePayload>()
    ipcMocks.getLiveSessionsFor
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise)
    const { tracker, stop } = await startTracker()
    expect(ipcMocks.getLiveSessionsFor).not.toHaveBeenCalled()

    // One list shows 600 rows the base did not name, plus 10 it did.
    const owner = {}
    const listed = [...manyRefs(600, "list"), ...manyRefs(10)]
    tracker.setInterest(owner, listed)
    await vi.waitFor(() => expect(ipcMocks.getLiveSessionsFor).toHaveBeenCalledTimes(1))
    const calls = requestedKeys()
    expect(calls[0]).toHaveLength(LIVE_PRESENCE_REQUEST_LIMIT)
    expect(calls[0]).toEqual(manyRefs(500, "list").map(sessionRefKey))

    // Nothing else is read while the first chunk is in flight, even when
    // the interest changes: the change is remembered for after the read.
    tracker.setInterest(owner, listed)
    tracker.setInterest({}, [ref("late")])
    await Promise.resolve()
    expect(ipcMocks.getLiveSessionsFor).toHaveBeenCalledTimes(1)

    first.resolve({
      seq: 11,
      present: [present(ref("list-0"), true)],
      absent: manyRefs(500, "list").slice(1),
    })
    await vi.waitFor(() => expect(ipcMocks.getLiveSessionsFor).toHaveBeenCalledTimes(2))
    expect(requestedKeys()[1]).toEqual(manyRefs(100, "list").map((_, i) => sessionRefKey(ref(`list-${500 + i}`))))
    expect(registryActivity(tracker.getSnapshot(), sessionRefKey(ref("list-0")))).toBe(true)
    expect(registryActivity(tracker.getSnapshot(), sessionRefKey(ref("list-1")))).toBe(false)
    expect(registryActivity(tracker.getSnapshot(), sessionRefKey(ref("list-500")))).toBeNull()

    second.resolve({ seq: 12, present: manyRefs(100, "list").map((_, i) => present(ref(`list-${500 + i}`))), absent: [] })
    // The remembered change runs one more read, for the still-unknown key only.
    await vi.waitFor(() => expect(ipcMocks.getLiveSessionsFor).toHaveBeenCalledTimes(3))
    expect(requestedKeys()[2]).toEqual([sessionRefKey(ref("late"))])
    await vi.waitFor(() =>
      expect(registryActivity(tracker.getSnapshot(), sessionRefKey(ref("list-599")))).toBe(true),
    )
    expect(tracker.getSnapshot().absent.size).toBe(499)
    expect(tracker.getSnapshot().sessions.size).toBe(128 + 1 + 100)
    stop()
  })

  it("merges presence and deltas only when newer than a key's own evidence", async () => {
    ipcMocks.getLiveSessions.mockResolvedValue({
      seq: 10,
      working: 3,
      total: 3,
      sessions: [present(ref("row"))],
      anonymous: [],
    })
    const answer = deferred<Ipc.LivePresencePayload>()
    ipcMocks.getLiveSessionsFor.mockReturnValueOnce(answer.promise)
    const { tracker, stop } = await startTracker()
    tracker.setInterest({}, [ref("row"), ref("k"), ref("j")])
    await vi.waitFor(() => expect(ipcMocks.getLiveSessionsFor).toHaveBeenCalledTimes(1))
    expect(requestedKeys()[0]).toEqual([sessionRefKey(ref("k")), sessionRefKey(ref("j"))])

    // While the read is in flight the bus says `k` went idle at 12.
    emit({ seq: 12, kind: "idle", session: ref("k"), agent: "claude-code", at: 300, aggregate: counts(2, 2) })
    expect(registryActivity(tracker.getSnapshot(), sessionRefKey(ref("k")))).toBe(false)

    // The answer, read at 11, still saw `k` live: older than the idle, so it
    // cannot bring `k` back. `j` had no evidence, so the answer stands.
    answer.resolve({ seq: 11, present: [present(ref("k")), present(ref("j"))], absent: [] })
    await vi.waitFor(() =>
      expect(registryActivity(tracker.getSnapshot(), sessionRefKey(ref("j")))).toBe(true),
    )
    expect(registryActivity(tracker.getSnapshot(), sessionRefKey(ref("k")))).toBe(false)
    expect(tracker.getSnapshot()).toMatchObject({ working: 2, total: 2 })

    // A late delta at the answer's own sequence is not newer than it.
    emit({ seq: 11, kind: "quiet", session: ref("j"), agent: "claude-code", at: 130 })
    expect(tracker.getSnapshot().sessions.get(sessionRefKey(ref("j")))?.quiet).toBe(false)
    // A newer one is.
    emit({ seq: 13, kind: "quiet", session: ref("j"), agent: "claude-code", at: 131, aggregate: counts(1, 2) })
    expect(tracker.getSnapshot().sessions.get(sessionRefKey(ref("j")))?.quiet).toBe(true)
    // A delta at the sequence of the absence evidence for `k` is not newer.
    emit({ seq: 12, kind: "activity", session: ref("k"), agent: "claude-code", at: 301, resumed: true })
    expect(registryActivity(tracker.getSnapshot(), sessionRefKey(ref("k")))).toBe(false)
    emit({ seq: 14, kind: "activity", session: ref("k"), agent: "claude-code", at: 302, resumed: true, aggregate: counts(2, 3) })
    expect(registryActivity(tracker.getSnapshot(), sessionRefKey(ref("k")))).toBe(true)
    expect(tracker.getSnapshot().absent.has(sessionRefKey(ref("k")))).toBe(false)
    stop()
  })

  it("discards a presence answer below the base and keeps one newer than a later snapshot", async () => {
    ipcMocks.getLiveSessions.mockResolvedValue({
      seq: 10,
      working: 5,
      total: 5,
      sessions: [present(ref("row"))],
      anonymous: [],
    })
    const stale = deferred<Ipc.LivePresencePayload>()
    ipcMocks.getLiveSessionsFor.mockReturnValueOnce(stale.promise)
    const { tracker, stop } = await startTracker()
    tracker.setInterest({}, [ref("k")])
    await vi.waitFor(() => expect(ipcMocks.getLiveSessionsFor).toHaveBeenCalledTimes(1))

    // A resync arrives; the re-read snapshot is held. The presence answer
    // for the old base lands first, ahead of that snapshot in registry
    // order.
    const reread = deferred<Ipc.LiveSnapshotPayload>()
    ipcMocks.getLiveSessions.mockReturnValueOnce(reread.promise)
    emit({ seq: 15, kind: "resync" })
    stale.resolve({ seq: 16, present: [present(ref("k"))], absent: [] })
    await vi.waitFor(() =>
      expect(registryActivity(tracker.getSnapshot(), sessionRefKey(ref("k")))).toBe(true),
    )

    // The snapshot resolves at 15, truncated and without `k`: the newer
    // presence evidence for `k` survives it, and the omitted interests
    // that lack newer evidence are asked for again.
    tracker.setInterest({}, [ref("other")])
    ipcMocks.getLiveSessionsFor.mockResolvedValueOnce({ seq: 17, present: [], absent: [ref("other")] })
    reread.resolve({ seq: 15, working: 6, total: 6, sessions: [present(ref("row"))], anonymous: [] })
    await vi.waitFor(() => expect(ipcMocks.getLiveSessionsFor).toHaveBeenCalledTimes(2))
    expect(requestedKeys()[1]).toEqual([sessionRefKey(ref("other"))])
    expect(registryActivity(tracker.getSnapshot(), sessionRefKey(ref("k")))).toBe(true)
    expect(tracker.getSnapshot()).toMatchObject({ seq: 15, working: 6, total: 6 })
    await vi.waitFor(() =>
      expect(registryActivity(tracker.getSnapshot(), sessionRefKey(ref("other")))).toBe(false),
    )

    // An answer below the base is discarded whole.
    ipcMocks.getLiveSessionsFor.mockResolvedValueOnce({ seq: 9, present: [present(ref("ghost"))], absent: [ref("k")] })
    tracker.setInterest({}, [ref("ghost")])
    await vi.waitFor(() => expect(ipcMocks.getLiveSessionsFor).toHaveBeenCalledTimes(3))
    await Promise.resolve()
    expect(registryActivity(tracker.getSnapshot(), sessionRefKey(ref("ghost")))).toBeNull()
    expect(registryActivity(tracker.getSnapshot(), sessionRefKey(ref("k")))).toBe(true)
    stop()
  })

  it("re-asks for omitted interests after every accepted truncated snapshot, and never after a complete one", async () => {
    ipcMocks.getLiveSessions.mockResolvedValue(completeSnapshot(5, [present(ref("row"))]))
    const { tracker, stop } = await startTracker()
    const owner = {}
    tracker.setInterest(owner, [ref("row"), ref("k")])
    await Promise.resolve()
    // A complete base answers everything: `k` is absent without a read.
    expect(ipcMocks.getLiveSessionsFor).not.toHaveBeenCalled()
    expect(registryActivity(tracker.getSnapshot(), sessionRefKey(ref("k")))).toBe(false)

    // A lifecycle-only resync re-reads a base that is now truncated.
    ipcMocks.getLiveSessions.mockResolvedValue({
      seq: 20,
      working: 200,
      total: 200,
      sessions: [present(ref("row"))],
      anonymous: [],
    })
    ipcMocks.getLiveSessionsFor.mockResolvedValue({ seq: 21, present: [present(ref("k"))], absent: [] })
    emit({ seq: 19, kind: "resync" })
    await vi.waitFor(() => expect(ipcMocks.getLiveSessionsFor).toHaveBeenCalledTimes(1))
    expect(requestedKeys()[0]).toEqual([sessionRefKey(ref("k"))])
    await vi.waitFor(() =>
      expect(registryActivity(tracker.getSnapshot(), sessionRefKey(ref("k")))).toBe(true),
    )

    // Another resync at a later base asks again: the old answer is below
    // the new base and is dropped.
    ipcMocks.getLiveSessions.mockResolvedValue({
      seq: 30,
      working: 200,
      total: 200,
      sessions: [present(ref("row"))],
      anonymous: [],
    })
    ipcMocks.getLiveSessionsFor.mockResolvedValue({ seq: 31, present: [], absent: [ref("k")] })
    emit({ seq: 29, kind: "resync" })
    await vi.waitFor(() => expect(ipcMocks.getLiveSessionsFor).toHaveBeenCalledTimes(2))
    await vi.waitFor(() =>
      expect(registryActivity(tracker.getSnapshot(), sessionRefKey(ref("k")))).toBe(false),
    )
    stop()
  })

  it("retries a failed presence read through the snapshot retry and re-asks", async () => {
    vi.useFakeTimers()
    ipcMocks.getLiveSessions.mockResolvedValue({
      seq: 10,
      working: 300,
      total: 300,
      sessions: [present(ref("row"))],
      anonymous: [],
    })
    ipcMocks.getLiveSessionsFor
      .mockRejectedValueOnce(new Error("ipc failed"))
      .mockResolvedValueOnce({ seq: 12, present: [present(ref("k"))], absent: [] })
    const tracker = new LiveSessionsTracker()
    const stop = tracker.subscribe(() => undefined)
    await vi.advanceTimersByTimeAsync(0)
    expect(tracker.getSnapshot().ready).toBe(true)
    tracker.setInterest({}, [ref("k")])
    await vi.advanceTimersByTimeAsync(0)
    expect(ipcMocks.getLiveSessionsFor).toHaveBeenCalledTimes(1)
    expect(registryActivity(tracker.getSnapshot(), sessionRefKey(ref("k")))).toBeNull()
    expect(vi.getTimerCount()).toBe(1)

    await vi.advanceTimersByTimeAsync(SNAPSHOT_RETRY_MS)
    expect(ipcMocks.getLiveSessions).toHaveBeenCalledTimes(2)
    expect(ipcMocks.getLiveSessionsFor).toHaveBeenCalledTimes(2)
    expect(registryActivity(tracker.getSnapshot(), sessionRefKey(ref("k")))).toBe(true)
    stop()
  })

  it("prunes absence evidence with its interest and answers nothing for an owner that left", async () => {
    ipcMocks.getLiveSessions.mockResolvedValue({
      seq: 10,
      working: 300,
      total: 300,
      sessions: [present(ref("row"))],
      anonymous: [],
    })
    ipcMocks.getLiveSessionsFor.mockResolvedValueOnce({ seq: 11, present: [], absent: [ref("a"), ref("b")] })
    const { tracker, stop } = await startTracker()
    const listA = {}
    const listB = {}
    tracker.setInterest(listA, [ref("a")])
    tracker.setInterest(listB, [ref("b")])
    await vi.waitFor(() => expect(tracker.getSnapshot().absent.size).toBe(2))

    // The same set again asks nothing.
    tracker.setInterest(listA, [ref("a")])
    await Promise.resolve()
    expect(ipcMocks.getLiveSessionsFor).toHaveBeenCalledTimes(1)

    // One owner leaves: its absence evidence goes with it, and no read runs
    // because the remaining interest is already answered.
    tracker.clearInterest(listA)
    expect([...tracker.getSnapshot().absent]).toEqual([sessionRefKey(ref("b"))])
    expect(registryActivity(tracker.getSnapshot(), sessionRefKey(ref("a")))).toBeNull()
    await Promise.resolve()
    expect(ipcMocks.getLiveSessionsFor).toHaveBeenCalledTimes(1)

    // An answer for an interest cleared while the read was in flight is
    // ignored, present or absent.
    const held = deferred<Ipc.LivePresencePayload>()
    ipcMocks.getLiveSessionsFor.mockReturnValueOnce(held.promise)
    tracker.setInterest(listA, [ref("c"), ref("d")])
    await vi.waitFor(() => expect(ipcMocks.getLiveSessionsFor).toHaveBeenCalledTimes(2))
    tracker.clearInterest(listA)
    held.resolve({ seq: 12, present: [present(ref("c"))], absent: [ref("d")] })
    await Promise.resolve()
    await Promise.resolve()
    expect(registryActivity(tracker.getSnapshot(), sessionRefKey(ref("c")))).toBeNull()
    expect(registryActivity(tracker.getSnapshot(), sessionRefKey(ref("d")))).toBeNull()
    expect([...tracker.getSnapshot().absent]).toEqual([sessionRefKey(ref("b"))])
    stop()
  })

  it("stop cancels pending presence callbacks", async () => {
    ipcMocks.getLiveSessions.mockResolvedValue({
      seq: 10,
      working: 300,
      total: 300,
      sessions: [present(ref("row"))],
      anonymous: [],
    })
    const held = deferred<Ipc.LivePresencePayload>()
    ipcMocks.getLiveSessionsFor.mockReturnValueOnce(held.promise)
    const listener = vi.fn()
    const tracker = new LiveSessionsTracker()
    const stop = tracker.subscribe(listener)
    await vi.waitFor(() => expect(tracker.getSnapshot().ready).toBe(true))
    const owner = {}
    tracker.setInterest(owner, [ref("k")])
    await vi.waitFor(() => expect(ipcMocks.getLiveSessionsFor).toHaveBeenCalledTimes(1))

    stop()
    listener.mockClear()
    held.resolve({ seq: 11, present: [present(ref("k"))], absent: [] })
    await Promise.resolve()
    await Promise.resolve()
    expect(listener).not.toHaveBeenCalled()
    expect(tracker.getSnapshot().ready).toBe(false)
    expect(tracker.getSnapshot().sessions.size).toBe(0)

    // Interests belong to their owners and survive a stop; the next start
    // asks for them once its base is truncated.
    ipcMocks.getLiveSessionsFor.mockResolvedValueOnce({ seq: 21, present: [], absent: [ref("k")] })
    ipcMocks.getLiveSessions.mockResolvedValue({
      seq: 20,
      working: 300,
      total: 300,
      sessions: [present(ref("row"))],
      anonymous: [],
    })
    const stopAgain = tracker.subscribe(() => undefined)
    await vi.waitFor(() => expect(ipcMocks.getLiveSessionsFor).toHaveBeenCalledTimes(2))
    await vi.waitFor(() =>
      expect(registryActivity(tracker.getSnapshot(), sessionRefKey(ref("k")))).toBe(false),
    )
    tracker.clearInterest(owner)
    expect(tracker.getSnapshot().absent.size).toBe(0)
    stopAgain()
  })

})
