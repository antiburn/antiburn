import type * as IpcModule from "../../lib/ipc"
import type * as SubjectModule from "../../lib/sessionSubject"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { MainActivitySession, orderedActivityEntries } from "./MainActivitySession"
import {
  DEFAULT_SETTINGS,
  type ActivityEntryPayload,
  type SessionAnalysisPayload,
} from "../../lib/ipc"
import { sessionKey } from "../../lib/sessionSubject"

const mocks = vi.hoisted(() => ({
  getSettings: vi.fn(),
  setSettings: vi.fn(),
  listRecentSessions: vi.fn(),
  getMainWindowVisible: vi.fn(),
  getLiveUsage: vi.fn(),
  getSessionLimitAllocations: vi.fn(),
  loadSessionAnalysis: vi.fn(),
  stops: [] as ReturnType<typeof vi.fn>[],
  events: new Map<string, (...args: unknown[]) => void>(),
}))
vi.mock("../../lib/ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof IpcModule>()
  const subscribe = (name: string) => async (handler: (...args: unknown[]) => void) => {
    mocks.events.set(name, handler)
    const stop = vi.fn(() => mocks.events.delete(name))
    mocks.stops.push(stop)
    return stop
  }
  return {
    ...actual,
    ...mocks,
    onMainWindowVisibilityChanged: subscribe("visibility"),
    onSettingsChanged: subscribe("settings"),
    onSessionsInvalidated: subscribe("invalidated"),
    onScanEvent: subscribe("scan"),
    onSessionEntryChanged: subscribe("entry"),
    onLiveUsageChanged: subscribe("usage"),
  }
})
vi.mock("../../lib/sessionSubject", async (importOriginal) => ({
  ...(await importOriginal<typeof SubjectModule>()),
  loadSessionAnalysis: mocks.loadSessionAnalysis,
}))
const entry = (id: string, extra = {}): ActivityEntryPayload =>
  ({
    agent: "claude",
    surface: "cli",
    title: null,
    sessionId: id,
    repo: "example",
    timestamp: new Date().toISOString(),
    isActive: true,
    cost: null,
    models: [],
    modelRuns: [],
    hasForkParent: false,
    forkChildCount: 0,
    wslDistro: null,
    ...extra,
  }) as ActivityEntryPayload
const payload = (title: string) => ({ title }) as SessionAnalysisPayload
function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => {
    resolve = done
  })
  return { promise, resolve }
}
let sessions: MainActivitySession[]
function start(active = true) {
  const session = new MainActivitySession()
  sessions.push(session)
  const stop = (active ? session.subscribe : session.subscribeInactive)(() => {})
  return { session, stop }
}
async function ready(session: MainActivitySession) {
  await vi.waitFor(() => expect(session.getSnapshot().entries).not.toBeNull())
}
beforeEach(() => {
  vi.clearAllMocks()
  mocks.events.clear()
  mocks.stops.length = 0
  sessions = []
  mocks.getSettings.mockResolvedValue(DEFAULT_SETTINGS)
  mocks.setSettings.mockImplementation(async (settings) => settings)
  mocks.getMainWindowVisible.mockResolvedValue(true)
  mocks.listRecentSessions.mockResolvedValue([entry("one"), entry("two")])
  mocks.loadSessionAnalysis.mockResolvedValue(payload("Loaded"))
  mocks.getLiveUsage.mockResolvedValue(null)
  mocks.getSessionLimitAllocations.mockResolvedValue(null)
})
afterEach(() => sessions.forEach((session) => session.dispose()))

describe("MainActivitySession", () => {
  it("leaves older sessions unselected", async () => {
    const yesterday = new Date()
    yesterday.setDate(yesterday.getDate() - 1)
    mocks.listRecentSessions.mockResolvedValue([
      entry("old", { isActive: false, timestamp: yesterday.toISOString() }),
    ])
    const { session } = start()
    await ready(session)
    expect(session.getSnapshot().subject).toBeNull()
    expect(mocks.loadSessionAnalysis).not.toHaveBeenCalled()
    expect(mocks.listRecentSessions).toHaveBeenCalledWith(DEFAULT_SETTINGS.activityWindowDays)
  })

  it("opens the newest active session before today's inactive sessions", async () => {
    mocks.listRecentSessions.mockResolvedValue([
      entry("today", { isActive: false }),
      entry("older-active", { timestamp: "2020-01-01T00:00:00Z" }),
      entry("newer-active", { timestamp: "2020-01-02T00:00:00Z" }),
    ])
    const { session } = start()
    await ready(session)
    expect(session.getSnapshot().subject?.sessionId).toBe("newer-active")
    expect(mocks.loadSessionAnalysis).toHaveBeenCalledWith(
      expect.objectContaining({ sessionId: "newer-active" }),
    )
  })

  it("opens today's newest session and preserves later user selection on refresh", async () => {
    const today = new Date()
    today.setHours(0, 0, 0, 0)
    mocks.listRecentSessions.mockResolvedValue([
      entry("earlier", { isActive: false, timestamp: today.toISOString() }),
      entry("latest", { isActive: false }),
    ])
    const { session } = start()
    await ready(session)
    expect(session.getSnapshot().subject?.sessionId).toBe("latest")
    session.selectEntry(session.getSnapshot().entries![0]!)
    session.refreshList()
    await vi.waitFor(() => expect(mocks.listRecentSessions).toHaveBeenCalledTimes(2))
    expect(session.getSnapshot().subject?.sessionId).toBe("earlier")
    session.clearSelection()
    session.refreshList()
    await vi.waitFor(() => expect(mocks.listRecentSessions).toHaveBeenCalledTimes(3))
    expect(session.getSnapshot().subject).toBeNull()
  })

  it("selects a session discovered after an empty initial list", async () => {
    mocks.listRecentSessions.mockResolvedValueOnce([])
    const { session } = start()
    await ready(session)
    expect(session.getSnapshot().subject).toBeNull()
    session.refreshList()
    await vi.waitFor(() => expect(session.getSnapshot().subject).not.toBeNull())
  })

  it("keeps hidden and inactive sections idle, then reconciles on return", async () => {
    mocks.getMainWindowVisible.mockResolvedValue(false)
    const { session } = start()
    await vi.waitFor(() => expect(mocks.getMainWindowVisible).toHaveBeenCalled())
    expect(mocks.listRecentSessions).not.toHaveBeenCalled()
    mocks.events.get("visibility")!(true)
    await ready(session)
    mocks.events.get("visibility")!(false)
    const count = mocks.listRecentSessions.mock.calls.length
    mocks.events.get("scan")!({}, "finished")
    mocks.events.get("entry")!(entry("three"))
    expect(mocks.listRecentSessions).toHaveBeenCalledTimes(count)
    expect(session.getSnapshot().active).toBe(false)
    mocks.events.get("visibility")!(true)
    await vi.waitFor(() => expect(mocks.listRecentSessions).toHaveBeenCalledTimes(count + 1))
  })

  it("does not load an unselected section even when the window is visible", async () => {
    const { session } = start(false)
    await vi.waitFor(() => expect(mocks.getMainWindowVisible).toHaveBeenCalled())
    expect(session.getSnapshot().active).toBe(false)
    expect(mocks.listRecentSessions).not.toHaveBeenCalled()
    session.subscribe(() => {})
    await ready(session)
  })

  it("preserves a visibility event over a stale initial snapshot", async () => {
    const pending = deferred<boolean>()
    mocks.getMainWindowVisible.mockReturnValue(pending.promise)
    const { session } = start()
    await vi.waitFor(() => expect(mocks.getMainWindowVisible).toHaveBeenCalled())
    mocks.events.get("visibility")!(true)
    pending.resolve(false)
    await ready(session)
    expect(session.getSnapshot().active).toBe(true)
  })

  it("ignores stale analysis after rapid selection and distinguishes environments", async () => {
    const first = deferred<SessionAnalysisPayload>()
    mocks.loadSessionAnalysis
      .mockReturnValueOnce(first.promise)
      .mockResolvedValue(payload("Second"))
    const { session } = start()
    await ready(session)
    const initial = session.getSnapshot().entries![0]!
    session.selectEntry(initial)
    session.selectEntry({ ...initial, wslDistro: "Ubuntu" })
    first.resolve(payload("Stale"))
    await vi.waitFor(() =>
      expect(session.getSnapshot().analysis?.payload?.title).toBe("Second"),
    )
    expect(session.getSnapshot().analysis?.key).toBe(
      sessionKey({ agent: initial.agent, sessionId: initial.sessionId!, wslDistro: "Ubuntu" }),
    )
  })

  it("keeps related-session history independent of list selection", async () => {
    const { session } = start()
    await ready(session)
    session.selectEntry(session.getSnapshot().entries![0]!)
    session.openRelated({
      agent: "claude",
      sessionId: "child",
      subagent: { parentSessionId: "one", subagentId: "child" },
    })
    expect(session.getSnapshot().history).toHaveLength(1)
    session.goBack()
    expect(session.getSnapshot().subject?.sessionId).toBe("one")
    expect(session.getSnapshot().history).toHaveLength(0)
    session.selectEntry(session.getSnapshot().entries![1]!)
    expect(session.getSnapshot().history).toHaveLength(0)
  })

  it("keeps selection outside a changed range but clears confirmed removal", async () => {
    const { session } = start()
    await ready(session)
    session.selectEntry(session.getSnapshot().entries![0]!)
    mocks.listRecentSessions.mockResolvedValue([])
    mocks.events.get("settings")!({ ...DEFAULT_SETTINGS, activityWindowDays: 1 })
    await vi.waitFor(() => expect(session.getSnapshot().entries).toHaveLength(0))
    expect(session.getSnapshot().subject?.sessionId).toBe("one")
    mocks.events.get("invalidated")!()
    await vi.waitFor(() => expect(session.getSnapshot().subject).toBeNull())
  })

  it("coalesces refresh requests and never publishes a hidden response", async () => {
    const { session } = start()
    await ready(session)
    const pending = deferred<ActivityEntryPayload[]>()
    mocks.listRecentSessions.mockReturnValueOnce(pending.promise)
    session.refreshList()
    session.refreshList()
    session.refreshList()
    expect(mocks.listRecentSessions).toHaveBeenCalledTimes(2)
    mocks.events.get("visibility")!(false)
    pending.resolve([entry("stale")])
    await Promise.resolve()
    await Promise.resolve()
    expect(session.getSnapshot().entries![0]!.sessionId).toBe("one")
    expect(mocks.listRecentSessions).toHaveBeenCalledTimes(2)
  })

  it("exposes a list failure with an explicit retry", async () => {
    mocks.listRecentSessions.mockRejectedValueOnce(new Error("Unavailable"))
    const { session } = start()
    await vi.waitFor(() => expect(session.getSnapshot().listError).toBe(true))
    expect(session.getSnapshot().entries).toBeNull()
    session.refreshList()
    await ready(session)
    expect(session.getSnapshot().listError).toBe(false)
  })

  it("keeps successful analysis visible while refreshing and after refresh failure", async () => {
    const { session } = start()
    await ready(session)
    session.selectEntry(session.getSnapshot().entries![0]!)
    await vi.waitFor(() =>
      expect(session.getSnapshot().analysis?.payload?.title).toBe("Loaded"),
    )
    mocks.loadSessionAnalysis.mockRejectedValueOnce(new Error("Unavailable"))
    session.refreshAnalysis()
    expect(session.getSnapshot().analysis?.payload?.title).toBe("Loaded")
    await vi.waitFor(() => expect(session.getSnapshot().analysis?.error).toBe(true))
    expect(session.getSnapshot().analysis?.payload?.title).toBe("Loaded")
  })

  it("preserves unrelated settings when changing the badge metric", async () => {
    const { session } = start()
    await ready(session)
    mocks.getSettings.mockResolvedValue({ ...DEFAULT_SETTINGS, activityWindowDays: 2 })
    await session.setBadgeMetric("weeklyPercent")
    expect(mocks.setSettings).toHaveBeenCalledWith(
      expect.objectContaining({ activityWindowDays: 2, sessionBadgeMetric: "weeklyPercent" }),
    )
  })

  it("orders navigation like the list and excludes non-opening rows", async () => {
    const { session } = start()
    await ready(session)
    expect(orderedActivityEntries(session.getSnapshot()).map((item) => item.sessionId)).toEqual(
      ["one", "two"],
    )
  })

  it("releases listeners and ignores late results after disposal", async () => {
    const pending = deferred<ActivityEntryPayload[]>()
    mocks.listRecentSessions.mockReturnValue(pending.promise)
    const { session, stop } = start()
    await vi.waitFor(() => expect(mocks.listRecentSessions).toHaveBeenCalled())
    stop()
    expect(mocks.stops.every((unlisten) => unlisten.mock.calls.length === 1)).toBe(true)
    pending.resolve([entry("late")])
    await Promise.resolve()
    expect(session.getSnapshot().entries).toBeNull()
  })
})

describe("MainActivitySession event ordering", () => {
  it("keeps a newer entry event when an older list response arrives", async () => {
    const { session } = start()
    await ready(session)
    const pending = deferred<ActivityEntryPayload[]>()
    mocks.listRecentSessions.mockReturnValueOnce(pending.promise)
    session.refreshList()
    mocks.events.get("entry")!(entry("one", { title: "New title" }))
    pending.resolve([entry("one", { title: "Old title" })])
    await Promise.resolve()
    await Promise.resolve()
    expect(session.getSnapshot().entries![0]!.title).toBe("New title")
  })

  it("discards analysis superseded by invalidation before publishing its replacement", async () => {
    const { session } = start()
    await ready(session)
    session.clearSelection()
    mocks.loadSessionAnalysis.mockClear()
    const first = deferred<SessionAnalysisPayload>()
    const second = deferred<SessionAnalysisPayload>()
    mocks.loadSessionAnalysis
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise)
    session.selectEntry(session.getSnapshot().entries![0]!)
    mocks.events.get("invalidated")!()
    first.resolve(payload("Old analysis"))
    await vi.waitFor(() => expect(mocks.loadSessionAnalysis).toHaveBeenCalledTimes(2))
    expect(session.getSnapshot().analysis).toBeNull()
    second.resolve(payload("New analysis"))
    await vi.waitFor(() =>
      expect(session.getSnapshot().analysis?.payload?.title).toBe("New analysis"),
    )
  })

  it("starts a newly selected session without waiting for the previous request", async () => {
    const { session } = start()
    await ready(session)
    session.clearSelection()
    const first = deferred<SessionAnalysisPayload>()
    mocks.loadSessionAnalysis
      .mockReturnValueOnce(first.promise)
      .mockResolvedValue(payload("Second"))
    session.selectEntry(session.getSnapshot().entries![0]!)
    session.selectEntry(session.getSnapshot().entries![1]!)
    await vi.waitFor(() =>
      expect(session.getSnapshot().analysis?.payload?.title).toBe("Second"),
    )
    first.resolve(payload("Old"))
  })
})
