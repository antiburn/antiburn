import type * as IpcModule from "../../lib/ipc"
import type * as SubjectModule from "../../lib/sessionSubject"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { MainActivitySession, orderedActivityEntries } from "./MainActivitySession"
import {
  DEFAULT_SETTINGS,
  type ActivityEntryPayload,
  type SessionAnalysisPayload,
  type SessionUpdatedPayload,
} from "../../lib/ipc"
import { sessionKey } from "../../lib/sessionSubject"
import { liveSessions } from "../../lib/sessionLifecycle"
import { toActivityEntry } from "../../lib/activityEntries"
import { MainWindowNavigationSession } from "./MainWindowNavigationSession"
import { parseSessionFilters, serializeSessionFilters } from "../../lib/sessionFilters"

const mocks = vi.hoisted(() => ({
  getSettings: vi.fn(),
  setSettings: vi.fn(),
  listRecentSessions: vi.fn(),
  getMainWindowVisible: vi.fn(),
  existingMainWindowSessionTargets: vi.fn(),
  getLiveUsage: vi.fn(),
  getSessionLimitAllocations: vi.fn(),
  getSessionQuota: vi.fn(),
  loadSessionAnalysis: vi.fn(),
  noteInteraction: vi.fn(),
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
    onSessionIndexChanged: subscribe("index"),
    onSessionUpdated: subscribe("update"),
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
    totalTokens: 0,
    hasForkParent: false,
    forkChildCount: 0,
    wslDistro: null,
    ...extra,
  }) as ActivityEntryPayload
const payload = (title: string) => ({ title }) as SessionAnalysisPayload
const update = (
  changed: ActivityEntryPayload,
  facets: Partial<SessionUpdatedPayload["facets"]> = { metadata: true },
): SessionUpdatedPayload => ({
  seq: 1,
  session: {
    environmentKey: changed.wslDistro ? `wsl:${changed.wslDistro}` : "native",
    agent: changed.agent,
    sessionId: changed.sessionId,
  },
  facets: {
    metadata: false,
    title: false,
    analysis: false,
    usage: false,
    checks: false,
    limits: false,
    ...facets,
  },
  entry: changed,
})
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
function startList() {
  const session = new MainActivitySession()
  sessions.push(session)
  const stop = session.subscribeList(() => {})
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
  mocks.existingMainWindowSessionTargets.mockImplementation(async (targets) => targets)
  const now = Date.now()
  mocks.listRecentSessions.mockResolvedValue([
    entry("one", { timestamp: new Date(now).toISOString() }),
    entry("two", { timestamp: new Date(now - 1).toISOString() }),
  ])
  mocks.loadSessionAnalysis.mockResolvedValue(payload("Loaded"))
  mocks.getLiveUsage.mockResolvedValue(null)
  mocks.getSessionLimitAllocations.mockResolvedValue(null)
  mocks.getSessionQuota.mockResolvedValue({ entries: [], generatedAt: "g" })
})
afterEach(() => sessions.forEach((session) => session.dispose()))

describe("MainActivitySession", () => {
  it("reopens the same detail only on a deliberate reveal without reloading analysis", async () => {
    const { session } = start()
    await ready(session)
    const before = session.getSnapshot()
    const reads = mocks.loadSessionAnalysis.mock.calls.length
    session.revealDetail()
    session.revealDetail()
    expect(session.getSnapshot().detailRevealRevision).toBe(before.detailRevealRevision + 2)
    expect(session.getSnapshot().subject).toBe(before.subject)
    expect(mocks.loadSessionAnalysis).toHaveBeenCalledTimes(reads)
    session.clearSelection()
    session.revealDetail()
    expect(session.getSnapshot().detailRevealRevision).toBe(before.detailRevealRevision + 2)
  })

  it("loads the shared list for a visible window without starting detail work", async () => {
    const { session } = startList()

    await ready(session)

    expect(session.getSnapshot().active).toBe(false)
    expect(session.getSnapshot().entries?.map((item) => item.sessionId)).toEqual(["one", "two"])
    expect(mocks.loadSessionAnalysis).not.toHaveBeenCalled()
    expect(mocks.getLiveUsage).not.toHaveBeenCalled()
    expect(mocks.getSessionQuota).not.toHaveBeenCalled()
    expect(mocks.noteInteraction).not.toHaveBeenCalled()
  })

  it("keeps one list request when the detail pane is added while it is pending", async () => {
    const pending = deferred<ActivityEntryPayload[]>()
    mocks.listRecentSessions.mockReturnValueOnce(pending.promise)
    const { session } = startList()
    await vi.waitFor(() => expect(mocks.listRecentSessions).toHaveBeenCalledOnce())

    session.subscribe(() => {})
    expect(mocks.listRecentSessions).toHaveBeenCalledOnce()
    pending.resolve([entry("shared")])

    await vi.waitFor(() => expect(session.getSnapshot().entries?.[0]?.sessionId).toBe("shared"))
    expect(mocks.loadSessionAnalysis).toHaveBeenCalledOnce()
  })

  it("loads default detail data once when the detail pane joins a loaded list", async () => {
    const { session } = startList()
    await ready(session)
    mocks.loadSessionAnalysis.mockClear()
    mocks.getSessionQuota.mockClear()

    session.subscribe(() => {})

    await vi.waitFor(() => expect(mocks.loadSessionAnalysis).toHaveBeenCalledOnce())
    await vi.waitFor(() => expect(mocks.getSessionQuota).toHaveBeenCalledOnce())
  })

  it("pauses and rejects a pending list while hidden, then reloads on resume", async () => {
    const pending = deferred<ActivityEntryPayload[]>()
    mocks.listRecentSessions.mockReturnValueOnce(pending.promise)
    const { session } = startList()
    await vi.waitFor(() => expect(mocks.listRecentSessions).toHaveBeenCalledOnce())

    mocks.events.get("visibility")!(false)
    pending.resolve([entry("stale")])
    await Promise.resolve()
    expect(session.getSnapshot().entries).toBeNull()

    mocks.listRecentSessions.mockResolvedValueOnce([entry("fresh")])
    mocks.events.get("visibility")!(true)
    await vi.waitFor(() => expect(session.getSnapshot().entries?.[0]?.sessionId).toBe("fresh"))
    expect(mocks.listRecentSessions).toHaveBeenCalledTimes(2)
    expect(mocks.loadSessionAnalysis).not.toHaveBeenCalled()
  })

  it("rejects a pending list after its last subscriber is disposed", async () => {
    const pending = deferred<ActivityEntryPayload[]>()
    mocks.listRecentSessions.mockReturnValueOnce(pending.promise)
    const { session, stop } = startList()
    await vi.waitFor(() => expect(mocks.listRecentSessions).toHaveBeenCalledOnce())

    stop()
    pending.resolve([entry("disposed")])
    await Promise.resolve()
    expect(session.getSnapshot().entries).toBeNull()
  })

  it("patches a list row without analyzing it when detail is not active", async () => {
    const { session } = startList()
    await ready(session)
    mocks.events.get("update")!(update(entry("one", { title: "Updated" })))

    await vi.waitFor(() => expect(session.getSnapshot().entries?.[0]?.title).toBe("Updated"))
    expect(mocks.loadSessionAnalysis).not.toHaveBeenCalled()
  })

  it("keeps a pending list response across a filter-only settings update", async () => {
    const pending = deferred<ActivityEntryPayload[]>()
    mocks.listRecentSessions.mockReturnValueOnce(pending.promise)
    const { session } = startList()
    await vi.waitFor(() => expect(mocks.listRecentSessions).toHaveBeenCalledOnce())

    mocks.events.get("settings")!({ ...DEFAULT_SETTINGS, sessionFilter: "failing" })
    pending.resolve([entry("after-filter")])

    await vi.waitFor(() =>
      expect(session.getSnapshot().entries?.[0]?.sessionId).toBe("after-filter"),
    )
    expect(mocks.listRecentSessions).toHaveBeenCalledOnce()
  })

  it("keeps list refreshes alive when the detail subscriber leaves", async () => {
    const { session } = startList()
    await ready(session)
    const detailStop = session.subscribe(() => {})
    await vi.waitFor(() => expect(mocks.loadSessionAnalysis).toHaveBeenCalledOnce())
    const usageCalls = mocks.getLiveUsage.mock.calls.length
    detailStop()
    mocks.listRecentSessions.mockResolvedValueOnce([entry("refreshed")])
    mocks.events.get("index")!({ seq: 2, cause: "invalidated" })

    await vi.waitFor(() =>
      expect(session.getSnapshot().entries?.[0]?.sessionId).toBe("refreshed"),
    )
    expect(mocks.getLiveUsage).toHaveBeenCalledTimes(usageCalls)
  })

  it("records automatic default-session exposure without sending identity", async () => {
    const { session } = start()
    await ready(session)
    await vi.waitFor(() =>
      expect(mocks.noteInteraction).toHaveBeenCalledWith({
        kind: "surfaceStateObserved",
        surface: "session_detail",
        state: "ready",
        origin: "automatic",
      }),
    )
    expect(mocks.noteInteraction).toHaveBeenCalledWith({
      kind: "surfaceViewed",
      surface: "session_detail",
      origin: "automatic",
    })
    expect(
      mocks.noteInteraction.mock.calls.some(([interaction]) =>
        Object.keys(interaction as object).some((key) =>
          ["identity", "agent", "sessionId", "wslDistro"].includes(key),
        ),
      ),
    ).toBe(false)
  })

  it("records a restored external session as user exposure only after activation", async () => {
    const { session } = start(false)
    session.restoreNavigation(
      parseSessionFilters("all"),
      { agent: "codex", sessionId: "linked", wslDistro: null },
      "user",
    )
    expect(mocks.noteInteraction).not.toHaveBeenCalledWith(
      expect.objectContaining({ surface: "session_detail" }),
    )

    session.subscribe(() => {})
    await vi.waitFor(() =>
      expect(mocks.noteInteraction).toHaveBeenCalledWith({
        kind: "surfaceViewed",
        surface: "session_detail",
        origin: "user",
      }),
    )
  })

  it("records list and related-session navigation as user exposure", async () => {
    const { session } = start()
    await ready(session)
    await vi.waitFor(() => expect(session.getSnapshot().analysis).not.toBeNull())
    session.clearSelection()
    mocks.noteInteraction.mockClear()

    session.selectEntry(session.getSnapshot().entries![1]!)
    session.openRelated({ agent: "claude", sessionId: "related", wslDistro: null })

    expect(
      mocks.noteInteraction.mock.calls.filter(
        ([interaction]) =>
          interaction.kind === "surfaceViewed" &&
          interaction.surface === "session_detail" &&
          interaction.origin === "user",
      ),
    ).toHaveLength(2)
  })

  it.each([
    ["empty", null],
    ["error", new Error("Unavailable")],
  ] as const)("records the %s session-detail state", async (state, result) => {
    if (result instanceof Error) mocks.loadSessionAnalysis.mockRejectedValue(result)
    else mocks.loadSessionAnalysis.mockResolvedValue(result)
    const { session } = start()
    await ready(session)

    await vi.waitFor(() =>
      expect(mocks.noteInteraction).toHaveBeenCalledWith({
        kind: "surfaceStateObserved",
        surface: "session_detail",
        state,
        origin: "automatic",
      }),
    )
  })

  it("starts a new exposure after selection is cleared", async () => {
    const { session } = start()
    await ready(session)
    await vi.waitFor(() => expect(session.getSnapshot().analysis).not.toBeNull())
    mocks.noteInteraction.mockClear()
    const selected = session.getSnapshot().entries![0]!

    session.clearSelection()
    session.selectEntry(selected)

    expect(mocks.noteInteraction).toHaveBeenCalledWith({
      kind: "surfaceViewed",
      surface: "session_detail",
      origin: "user",
    })
  })

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
    mocks.events.get("index")!({ seq: 3, cause: "scan_pass" })
    mocks.events.get("update")!(update(entry("three")))
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

  it("reports deliberate session and filter navigation to the window history", async () => {
    const { session } = start()
    await ready(session)
    const onNavigation = vi.fn()
    session.onNavigation = onNavigation
    session.clearSelection()

    session.selectEntry(session.getSnapshot().entries![0]!)
    session.openRelated({ agent: "claude", sessionId: "related", wslDistro: null })
    session.goBack()
    session.setSpendFilter("notable")

    expect(onNavigation.mock.calls).toEqual([["user"], ["user"], ["user"], ["user"]])
  })

  it("reports explicit filter navigation but suppresses history restoration analytics", async () => {
    const { session } = start()
    await ready(session)
    const navigation = new MainWindowNavigationSession(session)
    navigation.navigate({
      section: "activity",
      filters: parseSessionFilters("all"),
      subject: session.getSnapshot().subject,
    })
    mocks.noteInteraction.mockClear()

    navigation.navigate({
      section: "activity",
      filters: parseSessionFilters("notable"),
      subject: session.getSnapshot().subject,
    })

    expect(mocks.noteInteraction).toHaveBeenCalledWith({
      kind: "sessionFiltersChanged",
      action: "spend_notable",
    })
    mocks.noteInteraction.mockClear()

    navigation.back()

    expect(mocks.noteInteraction).not.toHaveBeenCalledWith(
      expect.objectContaining({ kind: "sessionFiltersChanged" }),
    )
  })

  it("reports automatic selection for replacement and suppresses restored navigation", async () => {
    const session = new MainActivitySession()
    sessions.push(session)
    const onNavigation = vi.fn()
    session.onNavigation = onNavigation
    session.subscribe(() => undefined)
    await ready(session)

    expect(onNavigation).toHaveBeenCalledExactlyOnceWith("automatic")
    onNavigation.mockClear()
    session.restoreNavigation(
      parseSessionFilters("notable"),
      { agent: "codex", sessionId: "restored", wslDistro: null },
      "user",
    )

    expect(onNavigation).not.toHaveBeenCalled()
    expect(session.getSnapshot()).toEqual(
      expect.objectContaining({
        filters: parseSessionFilters("notable"),
        subject: { agent: "codex", sessionId: "restored", wslDistro: null },
      }),
    )
  })

  it("reports a persisted filter as an automatic navigation replacement", async () => {
    mocks.getSettings.mockResolvedValue({ ...DEFAULT_SETTINGS, sessionFilter: "failing" })
    const session = new MainActivitySession()
    sessions.push(session)
    const onNavigation = vi.fn()
    session.onNavigation = onNavigation
    session.subscribe(() => undefined)
    await ready(session)

    expect(onNavigation).toHaveBeenCalledWith("automatic")
    expect(session.getSnapshot().filters).toEqual(parseSessionFilters("failing"))
  })

  it("keeps selection outside a changed range but clears confirmed removal", async () => {
    const { session } = start()
    await ready(session)
    session.selectEntry(session.getSnapshot().entries![0]!)
    mocks.listRecentSessions.mockResolvedValue([])
    mocks.events.get("settings")!({ ...DEFAULT_SETTINGS, activityWindowDays: 1 })
    await vi.waitFor(() => expect(session.getSnapshot().entries).toHaveLength(0))
    expect(session.getSnapshot().subject?.sessionId).toBe("one")
    new MainWindowNavigationSession(session)
    const onDeleted = vi.fn()
    session.onDeleted = onDeleted
    mocks.existingMainWindowSessionTargets.mockResolvedValue([])
    mocks.events.get("index")!({ seq: 4, cause: "removed", removal: "deleted" })
    await vi.waitFor(() => expect(session.getSnapshot().subject).toBeNull())
    expect(onDeleted).toHaveBeenCalledWith(expect.objectContaining({ sessionId: "one" }))
  })

  it("prunes all deleted history destinations after a complete invalidation", async () => {
    const { session } = start()
    await ready(session)
    session.clearSelection()
    const navigation = new MainWindowNavigationSession(session)
    session.selectEntry(session.getSnapshot().entries![0]!)
    session.selectEntry(session.getSnapshot().entries![1]!)
    mocks.existingMainWindowSessionTargets.mockResolvedValue([])
    mocks.listRecentSessions.mockResolvedValue([])

    mocks.events.get("index")!({ seq: 4, cause: "removed", removal: "deleted" })

    await vi.waitFor(() => expect(session.getSnapshot().subject).toBeNull())
    expect(navigation.getSnapshot()).toEqual(
      expect.objectContaining({ selected: "overview", canBack: false, canForward: false }),
    )
  })

  it("does not infer deletion from absence in a capped activity response", async () => {
    const { session } = start()
    await ready(session)
    session.selectEntry(session.getSnapshot().entries![0]!)
    new MainWindowNavigationSession(session)
    mocks.listRecentSessions.mockResolvedValue(
      Array.from({ length: 500 }, (_, index) => entry(`capped-${index}`)),
    )

    mocks.events.get("index")!({ seq: 4, cause: "removed", removal: "deleted" })

    await vi.waitFor(() => expect(session.getSnapshot().entries).toHaveLength(500))
    expect(session.getSnapshot().subject?.sessionId).toBe("one")
    expect(mocks.existingMainWindowSessionTargets).toHaveBeenCalledWith([
      { agent: "claude", sessionId: "one", wslDistro: null },
    ])
  })

  it("retains an older history subject when the authoritative index still has it", async () => {
    const { session } = start()
    await ready(session)
    session.selectEntry(session.getSnapshot().entries![0]!)
    new MainWindowNavigationSession(session)
    mocks.listRecentSessions.mockResolvedValue([])
    mocks.events.get("settings")!({ ...DEFAULT_SETTINGS, activityWindowDays: 1 })
    await vi.waitFor(() => expect(session.getSnapshot().entries).toHaveLength(0))

    mocks.events.get("index")!({ seq: 4, cause: "removed", removal: "deleted" })

    await vi.waitFor(() =>
      expect(mocks.existingMainWindowSessionTargets).toHaveBeenCalledWith([
        { agent: "claude", sessionId: "one", wslDistro: null },
      ]),
    )
    expect(session.getSnapshot().subject?.sessionId).toBe("one")
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

  it.each(["filter-first", "metric-first"])(
    "preserves both preferences when writes overlap (%s)",
    async (order) => {
      const { session } = start()
      await ready(session)
      let stored = { ...DEFAULT_SETTINGS }
      const gate = deferred<void>()
      let first = true
      mocks.getSettings.mockImplementation(async () => ({ ...stored }))
      mocks.setSettings.mockImplementation(async (settings) => {
        if (first) {
          first = false
          await gate.promise
        }
        stored = settings
        mocks.events.get("settings")!(stored)
        return stored
      })
      let metricWrite: Promise<void>
      if (order === "filter-first") {
        session.setResultFilter("failing")
        await vi.waitFor(() => expect(mocks.setSettings).toHaveBeenCalledTimes(1))
        metricWrite = session.setBadgeMetric("weeklyPercent")
      } else {
        metricWrite = session.setBadgeMetric("weeklyPercent")
        await vi.waitFor(() => expect(mocks.setSettings).toHaveBeenCalledTimes(1))
        session.setResultFilter("failing")
      }
      gate.resolve()
      await metricWrite
      await vi.waitFor(() => expect(mocks.setSettings).toHaveBeenCalledTimes(2))
      await vi.waitFor(() => {
        expect(parseSessionFilters(stored.sessionFilter).result).toBe("failing")
        expect(stored.sessionBadgeMetric).toBe("weeklyPercent")
        expect(session.getSnapshot().filters.result).toBe("failing")
        expect(session.getSnapshot().settings.sessionBadgeMetric).toBe("weeklyPercent")
      })
    },
  )

  it("selects a filter optimistically, persists it, and reports the change", async () => {
    const { session } = start()
    await ready(session)

    session.setSpendFilter("notable")

    const filters = { agents: [], result: "all" as const, spend: "notable" as const }
    expect(session.getSnapshot().filters).toEqual(filters)
    expect(session.getSnapshot().settings.sessionFilter).toBe(serializeSessionFilters(filters))
    await vi.waitFor(() => expect(mocks.setSettings).toHaveBeenCalledTimes(1))
    expect(mocks.setSettings).toHaveBeenCalledWith(
      expect.objectContaining({ sessionFilter: serializeSessionFilters(filters) }),
    )
    expect(mocks.noteInteraction).toHaveBeenCalledWith({
      kind: "sessionFiltersChanged",
      action: "spend_notable",
    })
  })

  it.each(["response-first", "event-first"])(
    "keeps optimistic filter history stable when settings arrive %s",
    async (order) => {
      const { session } = start()
      await ready(session)
      const navigation = new MainWindowNavigationSession(session)
      navigation.select("activity")
      const pending = deferred<typeof DEFAULT_SETTINGS>()
      mocks.setSettings.mockReturnValueOnce(pending.promise)
      session.setSpendFilter("notable")
      const snapshot = navigation.getSnapshot()
      const saved = { ...session.getSnapshot().settings }
      expect(session.getSnapshot().filters).toEqual(parseSessionFilters("notable"))
      if (order === "event-first") mocks.events.get("settings")!(saved)
      pending.resolve(saved)
      await pending.promise
      if (order === "response-first") mocks.events.get("settings")!(saved)
      expect(navigation.getSnapshot()).toBe(snapshot)
      expect(session.getSnapshot().filters).toEqual(parseSessionFilters("notable"))
      navigation.back()
      expect(navigation.getSnapshot().destination.filters).toEqual(parseSessionFilters("all"))
      navigation.forward()
      expect(navigation.getSnapshot().destination.filters).toEqual(
        parseSessionFilters("notable"),
      )
    },
  )

  it("ignores a rejected filter write after a newer filter succeeds", async () => {
    const { session } = start()
    await ready(session)
    let reject!: (error: Error) => void
    const pending = new Promise<typeof DEFAULT_SETTINGS>((_, fail) => {
      reject = fail
    })
    mocks.setSettings.mockReturnValueOnce(pending)
    session.setSpendFilter("notable")
    session.setResultFilter("failing")
    await Promise.resolve()
    reject(new Error("stale save failed"))
    await pending.catch(() => undefined)
    await Promise.resolve()
    expect(session.getSnapshot().filters).toEqual({
      agents: [],
      result: "failing",
      spend: "notable",
    })
    expect(session.getSnapshot().settingsError).toBe(false)
  })

  it("does nothing when the requested filter already matches", async () => {
    const { session } = start()
    await ready(session)
    mocks.noteInteraction.mockClear()

    session.clearFilters()
    session.resetAgents()
    session.setResultFilter("all")
    session.setSpendFilter("all")

    expect(mocks.setSettings).not.toHaveBeenCalled()
    expect(mocks.noteInteraction).not.toHaveBeenCalled()
  })

  it("never reports a selection while restoring the persisted filter on load", async () => {
    mocks.getSettings.mockResolvedValue({ ...DEFAULT_SETTINGS, sessionFilter: "failing" })
    const { session } = start()
    await ready(session)

    expect(session.getSnapshot().filters).toEqual({
      agents: [],
      result: "failing",
      spend: "all",
    })
    session.setResultFilter("failing")
    expect(mocks.setSettings).not.toHaveBeenCalled()
    expect(mocks.noteInteraction).not.toHaveBeenCalledWith(
      expect.objectContaining({ kind: "sessionFiltersChanged" }),
    )
  })

  it("reports an agent filter's slug only when the harness is recognized", async () => {
    const { session } = start()
    await ready(session)

    session.toggleAgent("codex")
    expect(mocks.noteInteraction).toHaveBeenCalledWith({
      kind: "sessionFiltersChanged",
      action: "agent_added",
      agent: "codex",
    })

    mocks.noteInteraction.mockClear()
    session.toggleAgent("some-future-harness")
    expect(mocks.noteInteraction).toHaveBeenCalledWith({
      kind: "sessionFiltersChanged",
      action: "agent_added",
    })
  })

  it("falls back to all when persisted settings carry an unrecognized filter id", async () => {
    mocks.getSettings.mockResolvedValue({ ...DEFAULT_SETTINGS, sessionFilter: "bogus" })
    const { session } = start()
    await ready(session)
    expect(session.getSnapshot().filters).toEqual(parseSessionFilters("all"))
  })

  it("restores versioned filters in canonical order without writing or reporting", async () => {
    mocks.getSettings.mockResolvedValue({
      ...DEFAULT_SETTINGS,
      sessionFilter:
        'v1:{"agents":["codex","claude-code","codex"],"result":"passing","spend":"material"}',
    })
    const { session } = start()
    await ready(session)
    expect(session.getSnapshot().filters).toEqual({
      agents: ["claude-code", "codex"],
      result: "passing",
      spend: "material",
    })
    session.setSpendFilter("material")
    expect(mocks.setSettings).not.toHaveBeenCalled()
    expect(mocks.noteInteraction).not.toHaveBeenCalledWith(
      expect.objectContaining({ kind: "sessionFiltersChanged" }),
    )
  })

  it("keeps rapid changes optimistic and saves them in gesture order", async () => {
    const { session } = start()
    await ready(session)
    const first = deferred<IpcModule.AppSettings>()
    mocks.setSettings.mockImplementationOnce(() => first.promise)
    session.toggleAgent("codex")
    session.toggleAgent("claude-code")
    session.setResultFilter("failing")
    const latest = {
      agents: ["claude-code", "codex"],
      result: "failing" as const,
      spend: "all" as const,
    }
    expect(session.getSnapshot().filters).toEqual(latest)
    await vi.waitFor(() => expect(mocks.setSettings).toHaveBeenCalledTimes(1))
    const firstSaved = {
      ...DEFAULT_SETTINGS,
      sessionFilter: serializeSessionFilters({
        agents: ["codex"],
        result: "all",
        spend: "all",
      }),
    }
    mocks.events.get("settings")!(firstSaved)
    expect(session.getSnapshot().filters).toEqual(latest)
    first.resolve(firstSaved)
    await vi.waitFor(() => expect(mocks.setSettings).toHaveBeenCalledTimes(3))
    expect(
      mocks.setSettings.mock.calls.map(([settings]) =>
        parseSessionFilters(settings.sessionFilter),
      ),
    ).toEqual([
      { agents: ["codex"], result: "all", spend: "all" },
      { agents: ["claude-code", "codex"], result: "all", spend: "all" },
      latest,
    ])
    expect(session.getSnapshot().filters).toEqual(latest)
  })

  it("reports one gesture per actual facet reset and retains the other facets", async () => {
    mocks.getSettings.mockResolvedValue({
      ...DEFAULT_SETTINGS,
      sessionFilter: serializeSessionFilters({
        agents: ["codex", "claude-code"],
        result: "failing",
        spend: "material",
      }),
    })
    const { session } = start()
    await ready(session)
    mocks.noteInteraction.mockClear()
    session.toggleAgent("codex")
    session.resetAgents()
    expect(session.getSnapshot().filters).toEqual({
      agents: [],
      result: "failing",
      spend: "material",
    })
    session.resetAgents()
    session.setResultFilter("all")
    session.setSpendFilter("all")
    session.clearFilters()
    expect(mocks.noteInteraction.mock.calls.map(([interaction]) => interaction)).toEqual([
      { kind: "sessionFiltersChanged", action: "agent_removed", agent: "codex" },
      { kind: "sessionFiltersChanged", action: "agents_all" },
      { kind: "sessionFiltersChanged", action: "result_all" },
      { kind: "sessionFiltersChanged", action: "spend_all" },
    ])
    session.toggleAgent("cursor")
    session.setResultFilter("passing")
    mocks.noteInteraction.mockClear()
    session.clearFilters()
    expect(mocks.noteInteraction).toHaveBeenCalledExactlyOnceWith({
      kind: "sessionFiltersChanged",
      action: "cleared_all",
    })
    await vi.waitFor(() => expect(mocks.setSettings).toHaveBeenCalledTimes(7))
  })

  it("reports a saved-settings notification failure and recovers on the next save", async () => {
    const { session } = start()
    await ready(session)
    let failOnce = true
    const unsubscribe = session.subscribeInactive(() => {
      if (session.getSnapshot().settings.sessionBadgeMetric === "weeklyPercent" && failOnce) {
        failOnce = false
        throw new Error("Listener failed")
      }
    })
    await session.setBadgeMetric("weeklyPercent")
    expect(session.getSnapshot().settingsError).toBe(true)
    await session.setBadgeMetric("cost")
    expect(session.getSnapshot().settingsError).toBe(false)
    unsubscribe()
  })

  it("retains an optimistic filter on save failure and recovers on the next change", async () => {
    const { session } = start()
    await ready(session)
    mocks.setSettings.mockRejectedValueOnce(new Error("Unavailable"))
    session.setResultFilter("passing")
    await vi.waitFor(() => expect(session.getSnapshot().settingsError).toBe(true))
    expect(session.getSnapshot().filters.result).toBe("passing")
    session.setSpendFilter("material")
    await vi.waitFor(() => expect(session.getSnapshot().settingsError).toBe(false))
    expect(session.getSnapshot().filters).toEqual({
      agents: [],
      result: "passing",
      spend: "material",
    })
  })

  it("orders navigation like the list and excludes non-opening rows", async () => {
    mocks.listRecentSessions.mockResolvedValue([
      entry("one", { timestamp: "2026-01-01T00:00:02.000Z" }),
      entry("two", { timestamp: "2026-01-01T00:00:01.000Z" }),
    ])
    const { session } = start()
    await ready(session)
    expect(orderedActivityEntries(session.getSnapshot()).map((item) => item.sessionId)).toEqual(
      ["one", "two"],
    )
  })

  it("loads quota contributions alongside the analysis for the open subject", async () => {
    mocks.getSessionQuota.mockResolvedValue({
      entries: [
        {
          provider: "anthropic",
          displayName: "Claude",
          accountKey: "acct-1",
          lane: "weekly",
          laneLabel: "Weekly",
          period: {
            periodId: 1,
            startsAtEpoch: 1,
            resetsAtEpoch: 2,
            startSource: "reported",
            resetSource: "reported",
          },
          usd: 1.5,
          percent: 5,
          confidence: "learned",
        },
      ],
      generatedAt: "q1",
    })
    const { session } = start()
    await ready(session)
    await vi.waitFor(() => expect(session.getSnapshot().sessionQuota?.generatedAt).toBe("q1"))
    expect(mocks.getSessionQuota).toHaveBeenCalledWith(
      expect.objectContaining({ agent: "claude", sessionId: "one", wslDistro: null }),
    )
  })

  it("keeps the last quota payload and leaves the analysis untouched when a later quota load fails", async () => {
    const { session } = start()
    await ready(session)
    await vi.waitFor(() => expect(session.getSnapshot().sessionQuota?.generatedAt).toBe("g"))

    mocks.getSessionQuota.mockRejectedValueOnce(new Error("no"))
    mocks.events.get("update")!(update(entry("one")))
    await vi.waitFor(() => expect(mocks.getSessionQuota).toHaveBeenCalledTimes(2))

    expect(session.getSnapshot().sessionQuota?.generatedAt).toBe("g")
    expect(session.getSnapshot().analysis?.payload?.title).toBe("Loaded")
    expect(session.getSnapshot().analysis?.error).toBe(false)
  })

  it("clears quota when the selection changes and reloads for the new subject", async () => {
    mocks.getSessionQuota.mockResolvedValueOnce({
      entries: [],
      generatedAt: "q-one",
    })
    const { session } = start()
    await ready(session)
    await vi.waitFor(() =>
      expect(session.getSnapshot().sessionQuota?.generatedAt).toBe("q-one"),
    )
    mocks.getSessionQuota.mockResolvedValueOnce({ entries: [], generatedAt: "q-two" })
    session.selectEntry(session.getSnapshot().entries![1]!)
    await vi.waitFor(() =>
      expect(session.getSnapshot().sessionQuota?.generatedAt).toBe("q-two"),
    )
    expect(mocks.getSessionQuota).toHaveBeenLastCalledWith(
      expect.objectContaining({ sessionId: "two" }),
    )
  })

  it("refreshes quota when the open subject's entry changes and on a live-usage push", async () => {
    mocks.listRecentSessions.mockResolvedValue([entry("one")])
    const { session } = start()
    await ready(session)
    expect(session.getSnapshot().subject?.sessionId).toBe("one")
    await vi.waitFor(() => expect(mocks.getSessionQuota).toHaveBeenCalledTimes(1))
    mocks.events.get("update")!(update(entry("one")))
    await vi.waitFor(() => expect(mocks.getSessionQuota).toHaveBeenCalledTimes(2))
    mocks.events.get("usage")!(null)
    await vi.waitFor(() => expect(mocks.getSessionQuota).toHaveBeenCalledTimes(3))
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

describe("MainActivitySession incremental cost classification", () => {
  function cost(totalUsd: number): NonNullable<ActivityEntryPayload["cost"]> {
    return {
      totalUsd,
      inputUsd: totalUsd / 4,
      outputUsd: totalUsd / 4,
      cacheReadUsd: totalUsd / 4,
      cacheWriteUsd: totalUsd / 4,
    }
  }

  it.each([
    {
      name: "clears changed and peer flags when the median rises",
      costs: [1, 1, 1, 1, 10, 10, 10, 20],
      nextCost: 20,
      beforeFlags: [false, false, false, false, false, false, false, true],
      afterFlags: [false, false, false, false, false, false, false, false],
    },
    {
      name: "flags a peer again when the median falls",
      costs: [20, 1, 1, 1, 10, 10, 10, 20],
      nextCost: 1,
      beforeFlags: [false, false, false, false, false, false, false, false],
      afterFlags: [false, false, false, false, false, false, false, true],
    },
    {
      name: "classifies the cohort when priced rows grow from seven to eight",
      costs: [null, 1, 1, 1, 1, 1, 1, 20],
      nextCost: 20,
      beforeFlags: [null, false, false, false, false, false, false, false],
      afterFlags: [true, false, false, false, false, false, false, true],
    },
    {
      name: "clears peer flags when eight priced rows become seven",
      costs: [20, 1, 1, 1, 1, 1, 1, 20],
      nextCost: null,
      beforeFlags: [true, false, false, false, false, false, false, true],
      afterFlags: [null, false, false, false, false, false, false, false],
    },
  ])(
    "$name without reloading the list",
    async ({ costs, nextCost, beforeFlags, afterFlags }) => {
      const rows = costs.map((usd, index) =>
        entry(`row-${index}`, {
          title: `Session ${index}`,
          timestamp: `2026-01-01T00:00:0${index}.000Z`,
          repo: "widgets",
          wslDistro: "Ubuntu",
          hasForkParent: true,
          forkChildCount: 2,
          models: ["claude-fable-5"],
          modelRuns: [{ model: "claude-fable-5", thinkingMode: "low" }],
          cost: usd === null ? null : cost(usd),
        }),
      )
      rows.push(entry("unpriced"))
      mocks.listRecentSessions.mockResolvedValue(rows)
      mocks.getSettings.mockResolvedValue({ ...DEFAULT_SETTINGS, sessionFilter: "notable" })
      const { session } = start()
      await ready(session)
      await vi.waitFor(() => expect(session.getSnapshot().analysis).not.toBeNull())
      const before = session.getSnapshot()
      expect(before.entries!.map((row) => row.cost?.isHighCost ?? null)).toEqual([
        ...beforeFlags,
        null,
      ])
      const analysisCalls = mocks.loadSessionAnalysis.mock.calls.length
      const usageCalls = mocks.getLiveUsage.mock.calls.length
      const changed = {
        ...rows[0]!,
        title: "Updated title",
        cost: nextCost === null ? null : cost(nextCost),
      }

      mocks.events.get("update")!(update(changed, { title: true }))

      const after = session.getSnapshot()
      expect(after.entries!.map((row) => row.cost?.isHighCost ?? null)).toEqual([
        ...afterFlags,
        null,
      ])
      expect(after.entries!.map((row) => row.sessionId)).toEqual(
        rows.map((row) => row.sessionId),
      )
      const expectedChanged = toActivityEntry(changed)
      expect(after.entries![0]).toEqual({
        ...expectedChanged,
        cost: expectedChanged.cost
          ? { ...expectedChanged.cost, isHighCost: afterFlags[0] }
          : null,
      })
      for (let index = 1; index < before.entries!.length; index += 1) {
        const previous = before.entries![index]!
        if (previous.cost && beforeFlags[index] !== afterFlags[index]) {
          expect(after.entries![index]).toEqual({
            ...previous,
            cost: { ...previous.cost, isHighCost: afterFlags[index] },
          })
          expect(after.entries![index]!.cost!.breakdownRows).toBe(previous.cost.breakdownRows)
          expect(after.entries![index]!.cost!.models).toBe(previous.cost.models)
          expect(after.entries![index]!.modelRuns).toBe(previous.modelRuns)
        } else expect(after.entries![index]).toBe(previous)
      }
      expect(after.subject).toBe(before.subject)
      expect(after.history).toBe(before.history)
      expect(after.analysis).toBe(before.analysis)
      expect(after.filters).toBe(before.filters)
      expect(after.settings).toBe(before.settings)
      expect(mocks.listRecentSessions).toHaveBeenCalledTimes(1)
      expect(mocks.loadSessionAnalysis).toHaveBeenCalledTimes(analysisCalls)
      expect(mocks.getLiveUsage).toHaveBeenCalledTimes(usageCalls)
    },
  )
})

describe("MainActivitySession event ordering", () => {
  it("keeps a newer entry event when an older list response arrives", async () => {
    const { session } = start()
    await ready(session)
    const pending = deferred<ActivityEntryPayload[]>()
    mocks.listRecentSessions.mockReturnValueOnce(pending.promise)
    session.refreshList()
    mocks.events.get("update")!(update(entry("one", { title: "New title" }), { title: true }))
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
    mocks.events.get("index")!({ seq: 9, cause: "invalidated" })
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

it.each(["hidden", "inactive"])(
  "suspends Sessions lifecycle overlays while %s and reconciles on resume",
  async (mode) => {
    let changed: (() => void) | null = null
    const subscribe = vi.spyOn(liveSessions, "subscribe").mockImplementation((listener) => {
      changed = listener
      return () => {
        changed = null
      }
    })
    const snapshot = vi.spyOn(liveSessions, "getSnapshot").mockReturnValue({
      seq: 1,
      ready: true,
      complete: true,
      sessions: new Map(),
      absent: new Set(),
      keylessAgents: new Set(),
      working: 0,
      total: 0,
      anonymous: 0,
      sweep: [],
    })
    const interests = new Set<object>()
    const setInterest = vi.spyOn(liveSessions, "setInterest").mockImplementation((owner) => {
      interests.add(owner)
    })
    const clearInterest = vi
      .spyOn(liveSessions, "clearInterest")
      .mockImplementation((owner) => {
        interests.delete(owner)
      })
    try {
      const { session, stop } = start()
      const listener = vi.fn()
      session.subscribeInactive(listener)
      await ready(session)
      expect(interests.has(session)).toBe(true)
      if (mode === "hidden") mocks.events.get("visibility")!(false)
      else stop()
      expect(session.getSnapshot().active).toBe(false)
      const before = session.getSnapshot()
      listener.mockClear()
      snapshot.mockReturnValue({
        ...liveSessions.getSnapshot(),
        seq: 2,
        working: 1,
        total: 1,
        sessions: new Map([
          ['["native","claude","one"]', { agent: "claude", lastActivityAt: 100, quiet: false }],
        ]),
      })
      const publish = changed as (() => void) | null
      publish?.()
      expect(session.getSnapshot()).toBe(before)
      expect(listener).not.toHaveBeenCalled()
      expect(interests.has(session)).toBe(false)
      if (mode === "hidden") mocks.events.get("visibility")!(true)
      else session.subscribe(() => undefined)
      expect(interests.has(session)).toBe(true)
      expect(
        session.getSnapshot().entries?.find((row) => row.sessionId === "one")?.isActive,
      ).toBe(true)
      session.dispose()
    } finally {
      subscribe.mockRestore()
      snapshot.mockRestore()
      setInterest.mockRestore()
      clearInterest.mockRestore()
    }
  },
)
