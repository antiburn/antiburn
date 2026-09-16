import { beforeEach, afterEach, expect, it, vi } from "vitest"
import type { LiveSnapshotPayload, SessionLifecycleEventPayload } from "./sessionIpc"
import { LiveSessionsTracker } from "./sessionLifecycle"
import { liveModels, liveProviders } from "./sessionLiveness"

const ipc = vi.hoisted(() => ({ snapshot: vi.fn(), presence: vi.fn(), listen: vi.fn() }))
vi.mock("./ipc", () => ({
  getLiveSessions: ipc.snapshot,
  getLiveSessionsFor: ipc.presence,
  onSessionLifecycleEvent: ipc.listen,
  LIVE_PRESENCE_REQUEST_LIMIT: 500,
}))
let receive: (event: SessionLifecycleEventPayload) => void
let stop: () => void
let tracker: LiveSessionsTracker
const scope = (model = "sonnet") => [
  {
    agent: "claude-code",
    working: 1,
    anonymous: 0,
    modelPendingWorking: 0,
    modelFailedWorking: 0,
    modelNoneWorking: 0,
    models: [{ model, working: 1 }],
  },
]
const snapshot = (seq = 0): LiveSnapshotPayload => ({
  seq,
  working: 129,
  total: 129,
  sessions: [],
  anonymous: [],
  sweep: scope(),
})
beforeEach(() => {
  ipc.snapshot.mockReset().mockResolvedValue(snapshot())
  ipc.presence.mockReset()
  ipc.listen.mockImplementation(async (handler) => {
    receive = handler
    return () => undefined
  })
  tracker = new LiveSessionsTracker()
  stop = tracker.subscribe(() => undefined)
})
afterEach(() => stop())

it("reports a cold omitted agent and model at sequence zero without list interests", async () => {
  await vi.waitFor(() => expect(tracker.getSnapshot().ready).toBe(true))
  expect(tracker.getSnapshot().sessions.size).toBe(0)
  expect(liveProviders(tracker.getSnapshot())).toEqual(["anthropic"])
  expect(liveModels(tracker.getSnapshot())).toEqual({ anthropic: ["sonnet"] })
  expect(ipc.presence).not.toHaveBeenCalled()
})
it("applies sequenced metadata without changing activity timestamps or accepting older counts", async () => {
  await vi.waitFor(() => expect(tracker.getSnapshot().ready).toBe(true))
  receive({
    seq: 1,
    kind: "activity",
    session: { agent: "claude-code", environmentKey: "native", sessionId: "one" },
    agent: "claude-code",
    at: 123,
    resumed: false,
  })
  const sessions = tracker.getSnapshot().sessions
  receive({
    seq: 3,
    kind: "sweep_changed",
    aggregate: { working: 129, total: 129, anonymous: 0, sweep: scope("opus") },
  })
  receive({
    seq: 2,
    kind: "sweep_changed",
    aggregate: { working: 129, total: 129, anonymous: 0, sweep: scope("old") },
  })
  expect(tracker.getSnapshot().sessions).toBe(sessions)
  expect([...sessions.values()][0]?.lastActivityAt).toBe(123)
  expect(liveModels(tracker.getSnapshot())).toEqual({ anthropic: ["opus"] })
})
it("clears positive scoped evidence immediately on failed resync", async () => {
  await vi.waitFor(() => expect(tracker.getSnapshot().ready).toBe(true))
  ipc.snapshot.mockRejectedValue(new Error("synthetic failure"))
  receive({ seq: 10, kind: "resync" })
  expect(liveModels(tracker.getSnapshot())).toEqual({})
  await vi.waitFor(() => expect(ipc.snapshot).toHaveBeenCalledTimes(2))
  receive({
    seq: 9,
    kind: "sweep_changed",
    aggregate: { working: 129, total: 129, anonymous: 0, sweep: scope("stale") },
  })
  expect(liveModels(tracker.getSnapshot())).toEqual({})
})
it("never restores stale scoped evidence from a pre-resync snapshot", async () => {
  await vi.waitFor(() => expect(tracker.getSnapshot().ready).toBe(true))
  receive({ seq: 10, kind: "resync" })
  await vi.waitFor(() => expect(ipc.snapshot).toHaveBeenCalledTimes(2))
  expect(liveModels(tracker.getSnapshot())).toEqual({})
  receive({
    seq: 11,
    kind: "sweep_changed",
    aggregate: { working: 129, total: 129, anonymous: 0, sweep: scope("new") },
  })
  expect(liveModels(tracker.getSnapshot())).toEqual({ anthropic: ["new"] })
})
it("disposal rejects a delayed scoped snapshot", async () => {
  await vi.waitFor(() => expect(tracker.getSnapshot().ready).toBe(true))
  let resolve!: (value: LiveSnapshotPayload) => void
  ipc.snapshot.mockImplementation(
    () =>
      new Promise<LiveSnapshotPayload>((done) => {
        resolve = done
      }),
  )
  receive({ seq: 10, kind: "resync" })
  stop()
  resolve(snapshot(11))
  await Promise.resolve()
  expect(tracker.getSnapshot().ready).toBe(false)
  expect(liveModels(tracker.getSnapshot())).toEqual({})
})
