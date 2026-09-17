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
  return { working, total, anonymous, sweep: [] }
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
    sweep: [],
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
    emit({
      seq: 5,
      kind: "activity",
      session: ref("a"),
      agent: "claude-code",
      at: 100,
      resumed: false,
    })
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
    emit({
      seq: 10,
      kind: "activity",
      session: ref("a"),
      agent: "claude-code",
      at: 100,
      resumed: false,
    })
    expect(tracker.getSnapshot().seq).toBe(10)

    // A resync forces a re-read, and the re-read resolves stale.
    let resolveSnapshot!: (value: Ipc.LiveSnapshotPayload) => void
    ipcMocks.getLiveSessions.mockReturnValueOnce(
      new Promise<Ipc.LiveSnapshotPayload>((resolve) => {
        resolveSnapshot = resolve
      }),
    )
    emit({ seq: 11, kind: "resync" })
    emit({
      seq: 12,
      kind: "activity",
      session: ref("b"),
      agent: "claude-code",
      at: 200,
      resumed: false,
    })
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

    emit({
      seq: 9,
      kind: "activity",
      session: ref("late"),
      agent: "claude-code",
      at: 100,
      resumed: false,
    })

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
      tracker.getSnapshot().sessions.has(JSON.stringify(["native", "codex", "resolved"])),
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
    emit({
      seq: 3,
      kind: "activity",
      session: ref("interim"),
      agent: "claude-code",
      at: 90,
      resumed: false,
    })
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

    emit({
      seq: 1,
      kind: "activity",
      session: ref("solo"),
      agent: "claude-code",
      at: 100,
      resumed: false,
    })

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
  it("uses a complete sequence-zero seed without a presence read or same-sequence delta", async () => {
    ipcMocks.getLiveSessions.mockResolvedValue(completeSnapshot(0, [present(ref("seeded"))]))
    const { tracker, stop } = await startTracker()
    tracker.setInterest({}, [ref("seeded"), ref("missing")])
    const accepted = tracker.getSnapshot()
    expect(accepted).toMatchObject({ seq: 0, complete: true, working: 1, total: 1 })
    expect(registryActivity(accepted, sessionRefKey(ref("seeded")))).toBe(true)
    expect(registryActivity(accepted, sessionRefKey(ref("missing")))).toBe(false)
    expect(ipcMocks.getLiveSessionsFor).not.toHaveBeenCalled()

    emit({
      seq: 0,
      kind: "activity",
      session: ref("missing"),
      agent: "claude-code",
      at: 200,
      resumed: false,
      aggregate: counts(2, 2),
    })
    expect(tracker.getSnapshot()).toBe(accepted)
    stop()
  })

  it.each(["before", "after"])(
    "queries omitted interests registered %s a truncated sequence-zero seed",
    async (registration) => {
      const seeded = manyRefs(130)
      ipcMocks.getLiveSessions.mockResolvedValue({
        seq: 0,
        working: 2,
        total: 130,
        sessions: seeded.slice(0, 128).map((session) => present(session, true)),
        anonymous: [],
      })
      const answer = deferred<Ipc.LivePresencePayload>()
      ipcMocks.getLiveSessionsFor.mockReturnValueOnce(answer.promise)
      const tracker = new LiveSessionsTracker()
      const owner = {}
      const interests = [ref("s-0"), ref("s-129"), ref("missing")]
      if (registration === "before") tracker.setInterest(owner, interests)
      const stop = tracker.subscribe(() => undefined)
      await vi.waitFor(() => expect(tracker.getSnapshot().ready).toBe(true))
      if (registration === "after") tracker.setInterest(owner, interests)

      expect(tracker.getSnapshot()).toMatchObject({
        seq: 0,
        complete: false,
        working: 2,
        total: 130,
        anonymous: 0,
      })
      expect(tracker.getSnapshot().sessions.size).toBe(128)
      expect(registryActivity(tracker.getSnapshot(), sessionRefKey(ref("s-129")))).toBeNull()
      expect(registryActivity(tracker.getSnapshot(), sessionRefKey(ref("missing")))).toBeNull()
      expect(requestedKeys()).toEqual([
        [sessionRefKey(ref("s-129")), sessionRefKey(ref("missing"))],
      ])

      const evidence = {
        seq: 0,
        present: [present(ref("s-129"))],
        absent: [ref("missing")],
      }
      answer.resolve(evidence)
      await vi.waitFor(() =>
        expect(registryActivity(tracker.getSnapshot(), sessionRefKey(ref("s-129")))).toBe(true),
      )
      expect(registryActivity(tracker.getSnapshot(), sessionRefKey(ref("missing")))).toBe(false)
      expect(registryActivity(tracker.getSnapshot(), sessionRefKey(ref("s-128")))).toBeNull()
      const accepted = tracker.getSnapshot()
      expect(accepted.sessions.size).toBe(129)
      expect(accepted).toMatchObject({ seq: 0, complete: false, working: 2, total: 130 })

      // Duplicate answers cannot change known evidence or publish another snapshot.
      tracker["applyPresence"](evidence)
      tracker["applyPresence"]({
        seq: 0,
        present: [present(ref("s-129"), true), present(ref("missing"))],
        absent: [ref("s-129")],
      })
      expect(tracker.getSnapshot()).toBe(accepted)

      tracker.setInterest({}, [...interests, ref("s-128")])
      expect(requestedKeys()).toEqual([
        [sessionRefKey(ref("s-129")), sessionRefKey(ref("missing"))],
        [sessionRefKey(ref("s-128"))],
      ])
      stop()
    },
  )

  it.each([
    { kind: "idle", answerSeq: 0 },
    { kind: "idle", answerSeq: 1 },
    { kind: "activity", answerSeq: 0 },
    { kind: "activity", answerSeq: 1 },
  ] as const)(
    "rejects presence at $answerSeq after a newer $kind delta from a sequence-zero base",
    async ({ kind, answerSeq }) => {
      ipcMocks.getLiveSessions.mockResolvedValue({
        seq: 0,
        working: 130,
        total: 130,
        sessions: manyRefs(128).map((session) => present(session)),
        anonymous: [],
      })
      const answer = deferred<Ipc.LivePresencePayload>()
      ipcMocks.getLiveSessionsFor.mockReturnValueOnce(answer.promise)
      const { tracker, stop } = await startTracker()
      const target = ref(kind === "idle" ? "s-129" : "missing")
      const key = sessionRefKey(target)
      tracker.setInterest({}, [target, ref("s-128")])
      expect(requestedKeys()).toEqual([[key, sessionRefKey(ref("s-128"))]])
      const aggregate = kind === "idle" ? counts(129, 129) : counts(131, 131)
      emit({
        seq: 1,
        kind,
        session: target,
        agent: "claude-code",
        at: 200,
        resumed: false,
        aggregate,
      })
      const afterDelta = tracker.getSnapshot()
      expect(registryActivity(afterDelta, key)).toBe(kind === "activity")

      const stale = {
        seq: answerSeq,
        present: [present(ref("s-128")), ...(kind === "idle" ? [present(target)] : [])],
        absent: kind === "activity" ? [target] : [],
      }
      answer.resolve(stale)
      await vi.waitFor(() =>
        expect(registryActivity(tracker.getSnapshot(), sessionRefKey(ref("s-128")))).toBe(true),
      )
      expect(registryActivity(tracker.getSnapshot(), key)).toBe(kind === "activity")
      expect(tracker.getSnapshot().sessions.get(key)).toEqual(afterDelta.sessions.get(key))
      expect(tracker.getSnapshot()).toMatchObject({ seq: 1, ...aggregate, complete: false })
      const accepted = tracker.getSnapshot()
      tracker["applyPresence"](stale)
      expect(tracker.getSnapshot()).toBe(accepted)
      stop()
    },
  )

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
    expect(requestedKeys()[1]).toEqual(
      manyRefs(100, "list").map((_, i) => sessionRefKey(ref(`list-${500 + i}`))),
    )
    expect(registryActivity(tracker.getSnapshot(), sessionRefKey(ref("list-0")))).toBe(true)
    expect(registryActivity(tracker.getSnapshot(), sessionRefKey(ref("list-1")))).toBe(false)
    expect(registryActivity(tracker.getSnapshot(), sessionRefKey(ref("list-500")))).toBeNull()

    second.resolve({
      seq: 12,
      present: manyRefs(100, "list").map((_, i) => present(ref(`list-${500 + i}`))),
      absent: [],
    })
    // The remembered change runs one more read, for the still-unknown key only.
    await vi.waitFor(() => expect(ipcMocks.getLiveSessionsFor).toHaveBeenCalledTimes(3))
    expect(requestedKeys()[2]).toEqual([sessionRefKey(ref("late"))])
    await vi.waitFor(() =>
      expect(registryActivity(tracker.getSnapshot(), sessionRefKey(ref("list-599")))).toBe(
        true,
      ),
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
    emit({
      seq: 12,
      kind: "idle",
      session: ref("k"),
      agent: "claude-code",
      at: 300,
      aggregate: counts(2, 2),
    })
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
    emit({
      seq: 13,
      kind: "quiet",
      session: ref("j"),
      agent: "claude-code",
      at: 131,
      aggregate: counts(1, 2),
    })
    expect(tracker.getSnapshot().sessions.get(sessionRefKey(ref("j")))?.quiet).toBe(true)
    // A delta at the sequence of the absence evidence for `k` is not newer.
    emit({
      seq: 12,
      kind: "activity",
      session: ref("k"),
      agent: "claude-code",
      at: 301,
      resumed: true,
    })
    expect(registryActivity(tracker.getSnapshot(), sessionRefKey(ref("k")))).toBe(false)
    emit({
      seq: 14,
      kind: "activity",
      session: ref("k"),
      agent: "claude-code",
      at: 302,
      resumed: true,
      aggregate: counts(2, 3),
    })
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
    ipcMocks.getLiveSessionsFor.mockResolvedValueOnce({
      seq: 17,
      present: [],
      absent: [ref("other")],
    })
    reread.resolve({
      seq: 15,
      working: 6,
      total: 6,
      sweep: [],
      sessions: [present(ref("row"))],
      anonymous: [],
    })
    await vi.waitFor(() => expect(ipcMocks.getLiveSessionsFor).toHaveBeenCalledTimes(2))
    expect(requestedKeys()[1]).toEqual([sessionRefKey(ref("other"))])
    expect(registryActivity(tracker.getSnapshot(), sessionRefKey(ref("k")))).toBe(true)
    expect(tracker.getSnapshot()).toMatchObject({ seq: 15, working: 6, total: 6 })
    await vi.waitFor(() =>
      expect(registryActivity(tracker.getSnapshot(), sessionRefKey(ref("other")))).toBe(false),
    )

    // An answer below the base is discarded whole.
    ipcMocks.getLiveSessionsFor.mockResolvedValueOnce({
      seq: 9,
      present: [present(ref("ghost"))],
      absent: [ref("k")],
    })
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
    ipcMocks.getLiveSessionsFor.mockResolvedValue({
      seq: 21,
      present: [present(ref("k"))],
      absent: [],
    })
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
    ipcMocks.getLiveSessionsFor.mockResolvedValueOnce({
      seq: 11,
      present: [],
      absent: [ref("a"), ref("b")],
    })
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
    ipcMocks.getLiveSessionsFor.mockResolvedValueOnce({
      seq: 21,
      present: [],
      absent: [ref("k")],
    })
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

describe("reviewed tracker recovery schedules", () => {
  it.each([0, 10])("fences a retired interest's presence answer at base %i", async (base) => {
    for (const idleBeforeClear of [false, true]) {
      ipcMocks.getLiveSessions.mockResolvedValue({
        seq: base,
        working: 130,
        total: 130,
        sessions: manyRefs(128).map((session) => present(session)),
        anonymous: [],
      })
      const old = deferred<Ipc.LivePresencePayload>()
      const current = deferred<Ipc.LivePresencePayload>()
      ipcMocks.getLiveSessionsFor
        .mockReturnValueOnce(old.promise)
        .mockReturnValueOnce(current.promise)
      const { tracker, stop } = await startTracker()
      const owner = {}
      const key = sessionRefKey(ref("omitted"))
      const idle = () =>
        emit({
          seq: base + 1,
          kind: "idle",
          session: ref("omitted"),
          agent: "claude-code",
          at: 101,
          aggregate: counts(129, 129),
        })
      tracker.setInterest(owner, [ref("omitted")])
      if (idleBeforeClear) idle()
      tracker.clearInterest(owner)
      if (!idleBeforeClear) idle()
      tracker.setInterest(owner, [ref("omitted")])
      const before = ipcMocks.getLiveSessionsFor.mock.calls.length
      old.resolve({ seq: base, present: [present(ref("omitted"))], absent: [] })
      await Promise.resolve()
      await Promise.resolve()
      expect(registryActivity(tracker.getSnapshot(), key)).not.toBe(true)
      expect(ipcMocks.getLiveSessionsFor).toHaveBeenCalledTimes(before + 1)
      expect(tracker.getSnapshot().working).toBe(129)
      current.resolve({ seq: base + 1, present: [], absent: [ref("omitted")] })
      await vi.waitFor(() => expect(registryActivity(tracker.getSnapshot(), key)).toBe(false))
      stop()
    }
  })

  it("retries listener attachment before snapshot and recovers quiet and idle", async () => {
    vi.useFakeTimers()
    ipcMocks.onSessionLifecycleEvent.mockRejectedValueOnce(new Error("listen failed"))
    ipcMocks.getLiveSessions.mockResolvedValue(completeSnapshot(0, [liveSession("a", 100)]))
    const tracker = new LiveSessionsTracker()
    const stop = tracker.subscribe(() => undefined)
    await vi.advanceTimersByTimeAsync(0)
    expect(tracker.getSnapshot().ready).toBe(false)
    expect(ipcMocks.getLiveSessions).not.toHaveBeenCalled()
    expect(vi.getTimerCount()).toBe(1)
    await vi.advanceTimersByTimeAsync(SNAPSHOT_RETRY_MS - 1)
    expect(ipcMocks.onSessionLifecycleEvent).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(1)
    expect(ipcMocks.onSessionLifecycleEvent).toHaveBeenCalledTimes(2)
    expect(ipcMocks.getLiveSessions).toHaveBeenCalledTimes(1)
    expect(tracker.getSnapshot().ready).toBe(true)
    emit({
      seq: 1,
      kind: "quiet",
      session: ref("a"),
      agent: "claude-code",
      at: 130,
      aggregate: counts(0, 1),
    })
    expect(tracker.getSnapshot().sessions.get(sessionRefKey(ref("a")))?.quiet).toBe(true)
    expect(tracker.getSnapshot().working).toBe(0)
    emit({
      seq: 2,
      kind: "idle",
      session: ref("a"),
      agent: "claude-code",
      at: 280,
      aggregate: counts(0, 0),
    })
    expect(tracker.getSnapshot().sessions.size).toBe(0)
    expect(tracker.getSnapshot().total).toBe(0)
    expect(vi.getTimerCount()).toBe(0)
    stop()
  })

  it("cancels listener retries when the last consumer stops", async () => {
    vi.useFakeTimers()
    ipcMocks.onSessionLifecycleEvent.mockRejectedValueOnce(new Error("listen failed"))
    const tracker = new LiveSessionsTracker()
    const listener = vi.fn()
    const stop = tracker.subscribe(listener)
    await vi.advanceTimersByTimeAsync(0)
    expect(vi.getTimerCount()).toBe(1)
    stop()
    listener.mockClear()
    await vi.advanceTimersByTimeAsync(SNAPSHOT_RETRY_MS * 2)
    expect(ipcMocks.onSessionLifecycleEvent).toHaveBeenCalledTimes(1)
    expect(ipcMocks.getLiveSessions).not.toHaveBeenCalled()
    expect(listener).not.toHaveBeenCalled()
    expect(vi.getTimerCount()).toBe(0)
  })
})

describe("omitted quiet ordering", () => {
  it.each(["absent", "newer", "idle", "resync"])(
    "orders a quiet transition against %s evidence",
    async (answerKind) => {
      ipcMocks.getLiveSessions.mockResolvedValue({
        seq: 0,
        working: 130,
        total: 130,
        sessions: manyRefs(128).map((session) => present(session)),
        anonymous: [],
      })
      const answer = deferred<Ipc.LivePresencePayload>()
      ipcMocks.getLiveSessionsFor.mockReturnValueOnce(answer.promise)
      const { tracker, stop } = await startTracker()
      const target = ref("omitted"),
        key = sessionRefKey(target),
        first = {},
        second = {}
      tracker.setInterest(first, [target])
      tracker.setInterest(second, [target])
      emit({ seq: 1, kind: "quiet", session: target, agent: "claude-code", at: 131 })
      tracker.clearInterest(first)
      if (answerKind === "idle")
        emit({ seq: 2, kind: "idle", session: target, agent: "claude-code", at: 280 })
      if (answerKind === "resync") {
        ipcMocks.getLiveSessions.mockResolvedValueOnce(completeSnapshot(2, [present(target)]))
        emit({ seq: 2, kind: "resync" })
        await vi.waitFor(() => expect(tracker.getSnapshot().complete).toBe(true))
      }
      if (answerKind === "absent") {
        ipcMocks.getLiveSessionsFor.mockResolvedValueOnce({
          seq: 1,
          present: [present(target, true)],
          absent: [],
        })
        answer.resolve({ seq: 0, present: [], absent: [target] })
        await vi.waitFor(() =>
          expect(tracker.getSnapshot().sessions.get(key)?.quiet).toBe(true),
        )
        expect(ipcMocks.getLiveSessionsFor).toHaveBeenCalledTimes(2)
      } else {
        answer.resolve({
          seq: answerKind === "newer" ? 2 : 0,
          present: [present(target)],
          absent: [],
        })
        await Promise.resolve()
        await Promise.resolve()
        if (answerKind === "idle")
          expect(registryActivity(tracker.getSnapshot(), key)).toBe(false)
        else expect(tracker.getSnapshot().sessions.get(key)?.quiet).toBe(false)
        expect(ipcMocks.getLiveSessionsFor).toHaveBeenCalledTimes(1)
      }
      stop()
    },
  )

  it.each([0, 10])("keeps quiet evidence and timestamp provenance at base %i", async (base) => {
    ipcMocks.getLiveSessions.mockResolvedValue({
      seq: base,
      working: 130,
      total: 130,
      sessions: manyRefs(128).map((session) => present(session)),
      anonymous: [],
    })
    const answer = deferred<Ipc.LivePresencePayload>()
    ipcMocks.getLiveSessionsFor.mockReturnValueOnce(answer.promise)
    const { tracker, stop } = await startTracker()
    const target = ref("omitted")
    const key = sessionRefKey(target)
    tracker.setInterest({}, [target])
    emit({
      seq: base + 2,
      kind: "quiet",
      session: target,
      agent: "claude-code",
      at: 131,
      aggregate: counts(129, 130),
    })
    emit({
      seq: base + 1,
      kind: "activity",
      session: target,
      agent: "claude-code",
      at: 99,
      resumed: false,
    })
    answer.resolve({ seq: base, present: [present(target)], absent: [] })
    await vi.waitFor(() =>
      expect(tracker.getSnapshot().sessions.get(key)).toEqual({
        agent: "claude-code",
        lastActivityAt: 100,
        quiet: true,
      }),
    )
    expect(tracker.getSnapshot()).toMatchObject({ seq: base + 2, working: 129, total: 130 })
    expect(ipcMocks.getLiveSessionsFor).toHaveBeenCalledTimes(1)
    emit({
      seq: base + 3,
      kind: "activity",
      session: target,
      agent: "claude-code",
      at: 150,
      resumed: true,
    })
    expect(tracker.getSnapshot().sessions.get(key)).toEqual({
      agent: "claude-code",
      lastActivityAt: 150,
      quiet: false,
    })
    stop()
  })

  it.each(["clear", "stop"])("discards unresolved quiet evidence on %s", async (action) => {
    ipcMocks.getLiveSessions.mockResolvedValue({
      seq: 0,
      working: 130,
      total: 130,
      sessions: manyRefs(128).map((session) => present(session)),
      anonymous: [],
    })
    const answer = deferred<Ipc.LivePresencePayload>()
    ipcMocks.getLiveSessionsFor.mockReturnValueOnce(answer.promise)
    const { tracker, stop } = await startTracker()
    const owner = {},
      target = ref("omitted"),
      key = sessionRefKey(target)
    tracker.setInterest(owner, [target])
    emit({ seq: 1, kind: "quiet", session: target, agent: "claude-code", at: 131 })
    if (action === "clear") tracker.clearInterest(owner)
    else stop()
    const before = tracker.getSnapshot()
    answer.resolve({ seq: 0, present: [present(target)], absent: [] })
    await Promise.resolve()
    await Promise.resolve()
    expect(tracker.getSnapshot()).toBe(before)
    expect(tracker.getSnapshot().sessions.has(key)).toBe(false)
    ipcMocks.getLiveSessions.mockResolvedValue({
      seq: 2,
      working: 130,
      total: 130,
      sessions: manyRefs(128).map((session) => present(session)),
      anonymous: [],
    })
    ipcMocks.getLiveSessionsFor.mockResolvedValueOnce({
      seq: 2,
      present: [present(target)],
      absent: [],
    })
    const stopAgain = action === "stop" ? tracker.subscribe(() => undefined) : stop
    if (action === "clear") tracker.setInterest(owner, [target])
    await vi.waitFor(() => expect(tracker.getSnapshot().sessions.get(key)?.quiet).toBe(false))
    stopAgain()
  })
})
