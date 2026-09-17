import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type * as Ipc from "../../lib/ipc"
import type * as InsightsIpc from "../../lib/insightsIpc"
import type * as OverlayWindow from "../../lib/overlayWindow"
import {
  EMPTY_PROVIDER_USAGE,
  type ActivityEntryPayload,
  type SessionIndexChangedPayload,
  type SessionLifecycleEventPayload,
  type SessionLimitAllocationSummaryPayload,
  type SessionUpdatedPayload,
  type UpdateFacetsPayload,
} from "../../lib/ipc"
import { PopoverSession } from "./PopoverSession"
import { liveSessions } from "../../lib/sessionLifecycle"

const getSessionLimitAllocations = vi.hoisted(() => vi.fn())
const getProviderUsage = vi.hoisted(() => vi.fn())
const listRecentSessions = vi.hoisted(() => vi.fn())
const getLiveSessions = vi.hoisted(() => vi.fn())
const getLiveSessionsFor = vi.hoisted(() => vi.fn())
const onSessionUpdated = vi.hoisted(() => vi.fn())
const onSessionIndexChanged = vi.hoisted(() => vi.fn())
const onSessionLifecycleEvent = vi.hoisted(() => vi.fn())
const onChecksReportChanged = vi.hoisted(() => vi.fn())
const getChecksReport = vi.hoisted(() => vi.fn())
const onPopoverShown = vi.hoisted(() => vi.fn())
const onPopoverHidden = vi.hoisted(() => vi.fn())
const noteInteraction = vi.hoisted(() => vi.fn())
const isCurrentWindowVisible = vi.hoisted(() => vi.fn())

// The list and event-subscription commands are overridden.
// Other wrappers keep their no-shell fallback outside Tauri.
vi.mock("../../lib/ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof Ipc>()
  return {
    ...actual,
    getSessionLimitAllocations,
    getProviderUsage,
    listRecentSessions,
    getLiveSessions,
    getLiveSessionsFor,
    onSessionUpdated,
    onSessionIndexChanged,
    onSessionLifecycleEvent,
    onPopoverShown,
    onPopoverHidden,
    noteInteraction,
  }
})

vi.mock("../../lib/overlayWindow", async (importOriginal) => {
  const actual = await importOriginal<typeof OverlayWindow>()
  return { ...actual, isCurrentWindowVisible }
})

vi.mock("../../lib/insightsIpc", async (importOriginal) => {
  const actual = await importOriginal<typeof InsightsIpc>()
  return { ...actual, getChecksReport, onChecksReportChanged }
})

type UpdatedHandler = (update: SessionUpdatedPayload) => void
type IndexChangedHandler = (change: SessionIndexChangedPayload) => void
type LifecycleHandler = (event: SessionLifecycleEventPayload) => void

let sessionUpdatedHandler: UpdatedHandler | null = null
let indexChangedHandler: IndexChangedHandler | null = null
// Both the session and the shared live tracker subscribe to lifecycle
// events, so every registered handler receives each emitted event.
const lifecycleHandlers = new Set<LifecycleHandler>()
let popoverShownHandler: (() => void) | null = null
let popoverHiddenHandler: (() => void) | null = null
let updateSeq = 0

function facets(overrides: Partial<UpdateFacetsPayload> = {}): UpdateFacetsPayload {
  return {
    metadata: false,
    title: false,
    analysis: false,
    usage: false,
    checks: false,
    limits: false,
    ...overrides,
  }
}

function emitUpdated(
  entry: ActivityEntryPayload,
  facetOverrides: Partial<UpdateFacetsPayload> = { metadata: true },
): void {
  updateSeq += 1
  sessionUpdatedHandler?.({
    seq: updateSeq,
    session: {
      environmentKey: entry.wslDistro ? `wsl:${entry.wslDistro}` : "native",
      agent: entry.agent,
      sessionId: entry.sessionId,
    },
    facets: facets(facetOverrides),
    entry,
  })
}

function emitLifecycleActivity(at = Date.now() / 1000): void {
  updateSeq += 1
  const event: SessionLifecycleEventPayload = {
    seq: updateSeq,
    kind: "activity",
    session: { environmentKey: "native", agent: "claude-code", sessionId: "session-1" },
    agent: "claude-code",
    at,
    resumed: false,
    aggregate: {
      working: 1,
      total: 1,
      anonymous: 0,
      sweep: [
        {
          agent: "claude-code",
          working: 1,
          anonymous: 0,
          modelPendingWorking: 1,
          modelFailedWorking: 0,
          modelNoneWorking: 0,
          models: [],
        },
      ],
    },
  }
  for (const handler of lifecycleHandlers) handler(event)
}

function changedEntry(): ActivityEntryPayload {
  return activityEntry({ timestamp: "2027-01-15T08:00:00Z", isActive: true })
}

function activityEntry(overrides: Partial<ActivityEntryPayload> = {}): ActivityEntryPayload {
  return {
    agent: "claude-code",
    sessionId: "session-1",
    repo: "repo",
    timestamp: "2024-01-01T00:00:00.000Z",
    isActive: false,
    surface: "cli",
    wslDistro: null,
    title: null,
    hasForkParent: false,
    forkChildCount: 0,
    cost: null,
    models: [],
    modelRuns: [],
    ...overrides,
  }
}

beforeEach(() => {
  sessionUpdatedHandler = null
  indexChangedHandler = null
  lifecycleHandlers.clear()
  popoverShownHandler = null
  popoverHiddenHandler = null
  updateSeq = 0
  listRecentSessions.mockReset()
  listRecentSessions.mockResolvedValue([])
  getLiveSessions.mockReset()
  getLiveSessions.mockResolvedValue({
    seq: 0,
    working: 0,
    total: 0,
    sessions: [],
    anonymous: [],
  })
  getLiveSessionsFor.mockReset()
  getLiveSessionsFor.mockResolvedValue(null)
  onSessionUpdated.mockReset()
  onSessionUpdated.mockImplementation(async (handler: UpdatedHandler) => {
    sessionUpdatedHandler = handler
    return () => {
      sessionUpdatedHandler = null
    }
  })
  onSessionIndexChanged.mockReset()
  onSessionIndexChanged.mockImplementation(async (handler: IndexChangedHandler) => {
    indexChangedHandler = handler
    return () => {
      indexChangedHandler = null
    }
  })
  onSessionLifecycleEvent.mockReset()
  onSessionLifecycleEvent.mockImplementation(async (handler: LifecycleHandler) => {
    lifecycleHandlers.add(handler)
    return () => {
      lifecycleHandlers.delete(handler)
    }
  })
  onChecksReportChanged.mockReset()
  onChecksReportChanged.mockResolvedValue(() => {})
  getChecksReport.mockReset()
  getChecksReport.mockResolvedValue(null)
  onPopoverShown.mockReset()
  onPopoverShown.mockImplementation(async (handler: () => void) => {
    popoverShownHandler = handler
    return () => {
      popoverShownHandler = null
    }
  })
  onPopoverHidden.mockReset()
  onPopoverHidden.mockImplementation(async (handler: () => void) => {
    popoverHiddenHandler = handler
    return () => {
      popoverHiddenHandler = null
    }
  })
  noteInteraction.mockReset()
  isCurrentWindowVisible.mockReset()
  isCurrentWindowVisible.mockResolvedValue(false)
  getSessionLimitAllocations.mockReset()
  getSessionLimitAllocations.mockResolvedValue({
    generatedAt: "2027-01-15T08:00:00Z",
    allocations: [],
  })
  getProviderUsage.mockReset()
  getProviderUsage.mockResolvedValue(EMPTY_PROVIDER_USAGE)
})

describe("PopoverSession surface presentation", () => {
  afterEach(() => {
    vi.unstubAllGlobals()
    vi.useRealTimers()
  })

  it("exposes activity-only snapshot state", () => {
    const snapshot = new PopoverSession().getSnapshot() as unknown as Record<string, unknown>

    expect(snapshot).not.toHaveProperty("stack")
    expect(snapshot).not.toHaveProperty("presentedSurface")
    expect(snapshot).not.toHaveProperty("presentedSession")
    expect(snapshot).not.toHaveProperty("analysis")
    expect(snapshot).not.toHaveProperty("analysisRefreshing")
  })

  it("ends the initial Checks loading state after a report failure", async () => {
    getChecksReport.mockRejectedValue(new Error("report failed"))
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => undefined)

    await vi.waitFor(() => expect(session.getSnapshot().checksUnavailable).toBe(true))

    unsubscribe()
  })

  it("does not count a hidden prewarmed renderer as a surface view", async () => {
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => undefined)

    await vi.waitFor(() => expect(popoverShownHandler).not.toBeNull())
    await vi.waitFor(() => expect(session.getSnapshot().entries).not.toBeNull())

    expect(noteInteraction).not.toHaveBeenCalled()
    unsubscribe()
  })

  it("records the cached activity outcome when the popover reaches the screen", async () => {
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(popoverShownHandler).not.toBeNull())
    await vi.waitFor(() => expect(session.getSnapshot().usage).not.toBeNull())

    popoverShownHandler?.()
    popoverShownHandler?.()

    expect(noteInteraction.mock.calls).toEqual([
      [{ kind: "surfaceViewed", surface: "activity", origin: "user" }],
      [
        {
          kind: "surfaceStateObserved",
          surface: "activity",
          state: "empty",
          origin: "user",
        },
      ],
    ])
    unsubscribe()
  })

  it("records useful activity before the secondary usage read settles", async () => {
    getProviderUsage.mockReturnValue(new Promise(() => undefined))
    listRecentSessions.mockResolvedValue([activityEntry()])
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(popoverShownHandler).not.toBeNull())
    popoverShownHandler?.()

    await vi.waitFor(() =>
      expect(noteInteraction).toHaveBeenCalledWith({
        kind: "surfaceStateObserved",
        surface: "activity",
        state: "ready",
        origin: "user",
      }),
    )
    unsubscribe()
  })

  it("records a failed activity read as error without also calling it empty", async () => {
    listRecentSessions.mockRejectedValue(new Error("list failed"))
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().entriesUnavailable).toBe(true))
    await vi.waitFor(() => expect(popoverShownHandler).not.toBeNull())
    popoverShownHandler?.()

    const states = noteInteraction.mock.calls
      .map(([interaction]) => interaction)
      .filter((interaction) => interaction.kind === "surfaceStateObserved")
    expect(states).toEqual([
      {
        kind: "surfaceStateObserved",
        surface: "activity",
        state: "error",
        origin: "user",
      },
    ])
    unsubscribe()
  })

  it("records a visible refresh failure when no cached activity is useful", async () => {
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().entries).toEqual([]))
    await vi.waitFor(() => expect(sessionUpdatedHandler).not.toBeNull())
    await vi.waitFor(() => expect(popoverShownHandler).not.toBeNull())
    popoverShownHandler?.()
    listRecentSessions.mockRejectedValue(new Error("refresh failed"))

    emitUpdated(activityEntry({ sessionId: "not-cached" }))

    await vi.waitFor(() => expect(session.getSnapshot().entriesUnavailable).toBe(true))
    expect(noteInteraction).toHaveBeenCalledWith({
      kind: "surfaceStateObserved",
      surface: "activity",
      state: "error",
      origin: "user",
    })
    unsubscribe()
  })

  it("uses the native visibility read when the shown event was missed", async () => {
    isCurrentWindowVisible.mockResolvedValue(true)
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => undefined)

    await vi.waitFor(() =>
      expect(noteInteraction).toHaveBeenCalledWith({
        kind: "surfaceViewed",
        surface: "activity",
        origin: "user",
      }),
    )
    popoverShownHandler?.()

    expect(
      noteInteraction.mock.calls.filter(
        ([interaction]) => interaction.kind === "surfaceViewed",
      ),
    ).toHaveLength(1)
    unsubscribe()
  })

  it("does not invent another view when the controller remounts in a visible window", async () => {
    isCurrentWindowVisible.mockResolvedValue(true)
    const session = new PopoverSession()
    const unsubscribeFirst = session.subscribe(() => undefined)
    await vi.waitFor(() =>
      expect(noteInteraction).toHaveBeenCalledWith({
        kind: "surfaceViewed",
        surface: "activity",
        origin: "user",
      }),
    )
    unsubscribeFirst()

    const unsubscribeSecond = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(isCurrentWindowVisible).toHaveBeenCalledTimes(2))

    expect(
      noteInteraction.mock.calls.filter(
        ([interaction]) => interaction.kind === "surfaceViewed",
      ),
    ).toHaveLength(1)
    unsubscribeSecond()
  })

  it("registers visibility listeners before reading the native state", async () => {
    let finishShownListener!: () => void
    onPopoverShown.mockImplementation(
      () =>
        new Promise<() => void>((resolve) => {
          finishShownListener = () => {
            popoverShownHandler = () => undefined
            resolve(() => {
              popoverShownHandler = null
            })
          }
        }),
    )
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(finishShownListener).toBeTypeOf("function"))

    expect(isCurrentWindowVisible).not.toHaveBeenCalled()
    finishShownListener()
    await vi.waitFor(() => expect(isCurrentWindowVisible).toHaveBeenCalledOnce())
    unsubscribe()
  })

  it("ignores a stale native visibility read after a hidden event", async () => {
    let resolveVisible!: (visible: boolean) => void
    isCurrentWindowVisible.mockReturnValue(
      new Promise<boolean>((resolve) => {
        resolveVisible = resolve
      }),
    )
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(popoverHiddenHandler).not.toBeNull())

    popoverHiddenHandler?.()
    resolveVisible(true)
    await Promise.resolve()

    expect(noteInteraction).not.toHaveBeenCalled()
    unsubscribe()
  })

  it("ignores an activity result from a stopped generation", async () => {
    let resolveStale!: (entries: ActivityEntryPayload[]) => void
    listRecentSessions.mockReturnValueOnce(
      new Promise<ActivityEntryPayload[]>((resolve) => {
        resolveStale = resolve
      }),
    )
    const session = new PopoverSession()
    const unsubscribeFirst = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(listRecentSessions).toHaveBeenCalledTimes(1))
    unsubscribeFirst()

    listRecentSessions.mockResolvedValue([])
    const unsubscribeSecond = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().entries).toEqual([]))
    await vi.waitFor(() => expect(popoverShownHandler).not.toBeNull())
    popoverShownHandler?.()
    resolveStale([activityEntry()])
    await Promise.resolve()

    expect(session.getSnapshot().entries).toEqual([])
    expect(noteInteraction).not.toHaveBeenCalledWith({
      kind: "surfaceStateObserved",
      surface: "activity",
      state: "ready",
      origin: "user",
    })
    unsubscribeSecond()
  })

  it("ignores an activity error from a stopped generation", async () => {
    let rejectStale!: (error: Error) => void
    listRecentSessions.mockReturnValueOnce(
      new Promise<ActivityEntryPayload[]>((_resolve, reject) => {
        rejectStale = reject
      }),
    )
    const session = new PopoverSession()
    const unsubscribeFirst = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(listRecentSessions).toHaveBeenCalledTimes(1))
    unsubscribeFirst()

    listRecentSessions.mockResolvedValue([])
    const unsubscribeSecond = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().entries).toEqual([]))
    await vi.waitFor(() => expect(popoverShownHandler).not.toBeNull())
    popoverShownHandler?.()
    rejectStale(new Error("stale failure"))
    await Promise.resolve()

    expect(session.getSnapshot().entriesUnavailable).toBe(false)
    expect(noteInteraction).not.toHaveBeenCalledWith({
      kind: "surfaceStateObserved",
      surface: "activity",
      state: "error",
      origin: "user",
    })
    unsubscribeSecond()
  })

  it("coalesces allocation events behind one refresh floor", async () => {
    vi.useFakeTimers()
    vi.setSystemTime("2027-01-15T08:00:00Z")
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    await vi.advanceTimersByTimeAsync(0)
    expect(getSessionLimitAllocations).toHaveBeenCalledTimes(1)

    emitUpdated(changedEntry())
    emitUpdated(changedEntry())
    await vi.advanceTimersByTimeAsync(29_999)
    expect(getSessionLimitAllocations).toHaveBeenCalledTimes(1)

    await vi.advanceTimersByTimeAsync(1)
    expect(getSessionLimitAllocations).toHaveBeenCalledTimes(2)
    unsubscribe()
  })

  it("publishes a current allocation response during an event storm", async () => {
    vi.useFakeTimers()
    vi.setSystemTime("2027-01-15T08:00:00Z")
    let resolveFirst!: (value: SessionLimitAllocationSummaryPayload) => void
    getSessionLimitAllocations
      .mockImplementationOnce(
        () =>
          new Promise<SessionLimitAllocationSummaryPayload>((resolve) => {
            resolveFirst = resolve
          }),
      )
      .mockResolvedValue({ generatedAt: "later", allocations: [] })
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    await vi.advanceTimersByTimeAsync(0)
    expect(getSessionLimitAllocations).toHaveBeenCalledTimes(1)

    for (let event = 0; event < 20; event += 1) {
      emitUpdated(changedEntry())
    }
    resolveFirst({
      generatedAt: "current",
      allocations: [
        {
          agent: "claude-code",
          sessionId: "session-1",
          wslDistro: null,
          provider: "anthropic",
          displayName: "Claude",
          accountKey: null,
          metric: "weekly",
          windowId: "weekly",
          percent: 10,
          confidence: "learned",
        },
      ],
    })
    await vi.advanceTimersByTimeAsync(0)

    expect(session.getSnapshot().sessionLimitAllocations.generatedAt).toBe("current")
    expect(getSessionLimitAllocations).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(30_000)
    expect(getSessionLimitAllocations).toHaveBeenCalledTimes(2)
    unsubscribe()
  })

  it("keeps a learned allocation cached as time passes", async () => {
    vi.useFakeTimers()
    vi.setSystemTime("2027-01-15T08:00:00Z")
    getSessionLimitAllocations.mockResolvedValue({
      generatedAt: "2027-01-15T08:00:00Z",
      allocations: [
        {
          agent: "claude-code",
          sessionId: "session-1",
          wslDistro: null,
          provider: "anthropic",
          displayName: "Claude",
          accountKey: null,
          metric: "weekly",
          windowId: "weekly",
          percent: 10,
          confidence: "learned",
        },
      ],
    })
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    await vi.advanceTimersByTimeAsync(0)
    expect(session.getSnapshot().sessionLimitAllocations.allocations).toHaveLength(1)
    await vi.advanceTimersByTimeAsync(1_001)

    expect(session.getSnapshot().sessionLimitAllocations.allocations).toHaveLength(1)
    unsubscribe()
  })

  it("keeps cached history but skips recurring allocation reads while hidden", async () => {
    vi.useFakeTimers()
    vi.setSystemTime("2027-01-15T08:00:00Z")
    getSessionLimitAllocations.mockResolvedValue({
      generatedAt: "2027-01-15T08:00:00Z",
      allocations: [
        {
          agent: "claude-code",
          sessionId: "session-1",
          wslDistro: null,
          provider: "anthropic",
          displayName: "Claude",
          accountKey: null,
          metric: "weekly",
          windowId: "weekly",
          percent: 10,
          confidence: "learned",
        },
      ],
    })
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    await vi.advanceTimersByTimeAsync(0)
    expect(getSessionLimitAllocations).toHaveBeenCalledTimes(1)
    expect(session.getSnapshot().sessionLimitAllocations.allocations).toHaveLength(1)

    popoverHiddenHandler?.()
    emitUpdated(changedEntry())
    await vi.advanceTimersByTimeAsync(60_000)
    expect(getSessionLimitAllocations).toHaveBeenCalledTimes(1)
    expect(session.getSnapshot().sessionLimitAllocations.allocations).toHaveLength(1)

    popoverShownHandler?.()
    await vi.advanceTimersByTimeAsync(0)
    expect(getSessionLimitAllocations).toHaveBeenCalledTimes(2)
    expect(session.getSnapshot().sessionLimitAllocations.allocations).toHaveLength(1)
    unsubscribe()
  })

  it("preserves a disabled cached read queued behind a stale hidden response", async () => {
    vi.useFakeTimers()
    vi.setSystemTime("2027-01-15T08:00:00Z")
    let resolveFirst!: (value: SessionLimitAllocationSummaryPayload) => void
    getSessionLimitAllocations
      .mockImplementationOnce(
        () =>
          new Promise<SessionLimitAllocationSummaryPayload>((resolve) => {
            resolveFirst = resolve
          }),
      )
      .mockResolvedValue({ generatedAt: "shown", allocations: [] })
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    await vi.advanceTimersByTimeAsync(0)
    expect(getSessionLimitAllocations).toHaveBeenCalledTimes(1)

    popoverHiddenHandler?.()
    popoverShownHandler?.()
    resolveFirst({ generatedAt: "hidden", allocations: [] })
    await vi.advanceTimersByTimeAsync(0)

    expect(getSessionLimitAllocations).toHaveBeenCalledTimes(2)
    expect(session.getSnapshot().sessionLimitAllocations.generatedAt).toBe("shown")
    unsubscribe()
  })

  it("refreshes inactive cached history for local row updates", async () => {
    vi.useFakeTimers()
    vi.setSystemTime("2027-01-15T08:00:00Z")
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    await vi.advanceTimersByTimeAsync(0)
    expect(getSessionLimitAllocations).toHaveBeenCalledTimes(1)

    emitUpdated(changedEntry())
    await vi.advanceTimersByTimeAsync(29_999)
    expect(getSessionLimitAllocations).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(1)
    expect(getSessionLimitAllocations).toHaveBeenCalledTimes(2)
    unsubscribe()
  })

  it("does not refresh allocations or checks for a title-only update", async () => {
    vi.useFakeTimers()
    vi.setSystemTime("2027-01-15T08:00:00Z")
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    await vi.advanceTimersByTimeAsync(0)
    expect(getSessionLimitAllocations).toHaveBeenCalledTimes(1)
    const checksBaseline = getChecksReport.mock.calls.length

    emitUpdated(changedEntry(), { title: true })
    await vi.advanceTimersByTimeAsync(60_000)

    expect(getSessionLimitAllocations).toHaveBeenCalledTimes(1)
    expect(getChecksReport).toHaveBeenCalledTimes(checksBaseline)
    unsubscribe()
  })

  it("refreshes checks for a checks-facet update", async () => {
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    await vi.waitFor(() => expect(sessionUpdatedHandler).not.toBeNull())
    await vi.waitFor(() => expect(getChecksReport).toHaveBeenCalled())
    const baseline = getChecksReport.mock.calls.length

    emitUpdated(changedEntry(), { checks: true })

    await vi.waitFor(() => expect(getChecksReport.mock.calls.length).toBeGreaterThan(baseline))
    unsubscribe()
  })

  it("keeps the activity clock current while visible and refreshes it when shown", async () => {
    vi.useFakeTimers()
    vi.setSystemTime("2027-01-15T08:00:00Z")
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    await vi.advanceTimersByTimeAsync(0)
    expect(popoverHiddenHandler).not.toBeNull()
    expect(popoverShownHandler).not.toBeNull()

    const initial = session.getSnapshot().now
    await vi.advanceTimersByTimeAsync(30_000)
    expect(session.getSnapshot().now).toBe(initial + 30_000)

    popoverHiddenHandler?.()
    const hidden = session.getSnapshot().now
    await vi.advanceTimersByTimeAsync(60_000)
    expect(session.getSnapshot().now).toBe(hidden)

    popoverShownHandler?.()
    expect(session.getSnapshot().now).toBe(Date.now())

    await vi.advanceTimersByTimeAsync(30_000)
    expect(session.getSnapshot().now).toBe(Date.now())
    unsubscribe()
  })
})

describe("PopoverSession live sessions", () => {
  const ref = { environmentKey: "native", agent: "claude-code", sessionId: "session-1" }

  it("follows the lifecycle bus and the snapshot", async () => {
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(lifecycleHandlers.size).toBe(2))
    await vi.waitFor(() => expect(getLiveSessions).toHaveBeenCalledTimes(1))
    expect(session.getSnapshot().sessionLive).toBe(false)
    expect(session.getSnapshot().liveProviders).toEqual([])

    const at = Math.floor(Date.now() / 1000)
    emitLifecycleActivity(at)
    expect(session.getSnapshot().sessionLive).toBe(true)
    expect(session.getSnapshot().liveProviders).toEqual([])

    for (const handler of lifecycleHandlers)
      handler({
        seq: ++updateSeq,
        kind: "quiet",
        session: ref,
        agent: "claude-code",
        at: at + 30,
        aggregate: { working: 0, total: 1, anonymous: 0, sweep: [] },
      })
    expect(session.getSnapshot().sessionLive).toBe(false)
    expect(session.getSnapshot().liveProviders).toEqual([])

    unsubscribe()
    expect(lifecycleHandlers.size).toBe(0)
  })

  it("starts live when the snapshot lists a session with a recent write", async () => {
    getLiveSessions.mockResolvedValue({
      seq: 0,
      working: 1,
      total: 1,
      anonymous: [],
      sessions: [
        {
          session: ref,
          agent: "claude-code",
          lastActivityAt: Math.floor(Date.now() / 1000),
          quiet: false,
        },
      ],
      sweep: [
        {
          agent: "claude-code",
          working: 1,
          anonymous: 0,
          modelPendingWorking: 1,
          modelFailedWorking: 0,
          modelNoneWorking: 0,
          models: [],
        },
      ],
    })
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => undefined)

    await vi.waitFor(() => expect(session.getSnapshot().sessionLive).toBe(true))
    expect(session.getSnapshot().liveProviders).toEqual([])
    unsubscribe()
  })

  it("immediately reads scoped counts when joining an existing tracker", async () => {
    getLiveSessions.mockResolvedValue({
      seq: 0,
      working: 1,
      total: 129,
      sessions: [],
      anonymous: [],
      sweep: [
        {
          agent: "claude-code",
          working: 1,
          anonymous: 0,
          modelPendingWorking: 0,
          modelFailedWorking: 0,
          modelNoneWorking: 0,
          models: [
            {
              model: "sonnet",
              working: 1,
              providerRoute: "anthropic",
              recordedProvider: "anthropic",
              modelVendor: null,
            },
          ],
        },
      ],
    })
    const keepAlive = liveSessions.subscribe(() => undefined)
    await vi.waitFor(() => expect(liveSessions.getSnapshot().ready).toBe(true))
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => undefined)
    expect(session.getSnapshot().sessionLive).toBe(true)
    expect(session.getSnapshot().liveProviders).toEqual(["anthropic"])
    expect(session.getSnapshot().liveModels).toEqual({ anthropic: ["sonnet"] })
    expect(getLiveSessions).toHaveBeenCalledTimes(1)
    unsubscribe()
    keepAlive()
  })

  it("keeps the shared registry subscription when the popover is shown", async () => {
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(popoverShownHandler).not.toBeNull())
    await vi.waitFor(() => expect(getLiveSessions).toHaveBeenCalledTimes(1))

    popoverShownHandler?.()
    expect(getLiveSessions).toHaveBeenCalledTimes(1)
    unsubscribe()
  })
})

/**
 * The event-driven refresh behind the activity list. Membership changes
 * arrive as `session:index-changed`; row changes as `session:updated`;
 * usage freshness rides lifecycle `activity` on a shared floor.
 */
describe("PopoverSession event-driven refresh", () => {
  const entryPayload = activityEntry

  it("refetches the list once for an update whose session is not on screen", async () => {
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    await vi.waitFor(() => expect(listRecentSessions).toHaveBeenCalledTimes(1))
    await vi.waitFor(() => expect(sessionUpdatedHandler).not.toBeNull())

    emitUpdated(entryPayload({ sessionId: "unknown-session" }))

    await vi.waitFor(() => expect(listRecentSessions).toHaveBeenCalledTimes(2))
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(listRecentSessions).toHaveBeenCalledTimes(2)
    unsubscribe()
  })

  it("patches a known row in place without another list query", async () => {
    listRecentSessions.mockResolvedValue([entryPayload()])
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    await vi.waitFor(() => expect(session.getSnapshot().entries).toHaveLength(1))
    await vi.waitFor(() => expect(sessionUpdatedHandler).not.toBeNull())
    const listQueries = listRecentSessions.mock.calls.length

    emitUpdated(entryPayload({ title: "Renamed by the projection" }), { title: true })

    await vi.waitFor(() =>
      expect(session.getSnapshot().entries?.[0]?.title).toBe("Renamed by the projection"),
    )
    expect(listRecentSessions).toHaveBeenCalledTimes(listQueries)
    unsubscribe()
  })

  it("refetches the list when membership changes, coalescing an event burst", async () => {
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    await vi.waitFor(() => expect(listRecentSessions).toHaveBeenCalledTimes(1))
    await vi.waitFor(() => expect(indexChangedHandler).not.toBeNull())

    let resolveList!: (entries: ActivityEntryPayload[]) => void
    listRecentSessions.mockImplementationOnce(
      () =>
        new Promise<ActivityEntryPayload[]>((resolve) => {
          resolveList = resolve
        }),
    )
    indexChangedHandler?.({ seq: 10, cause: "scan_pass" })
    indexChangedHandler?.({ seq: 11, cause: "removed", removal: "deleted" })
    indexChangedHandler?.({ seq: 12, cause: "invalidated" })
    await vi.waitFor(() => expect(listRecentSessions).toHaveBeenCalledTimes(2))

    resolveList([])
    // The burst behind the in-flight query coalesces into one follow-up.
    await vi.waitFor(() => expect(listRecentSessions).toHaveBeenCalledTimes(3))
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(listRecentSessions).toHaveBeenCalledTimes(3)
    unsubscribe()
  })

  it("forces a usage refresh on membership changes, except resync", async () => {
    vi.useFakeTimers()
    vi.setSystemTime("2027-01-15T08:00:00Z")
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    await vi.advanceTimersByTimeAsync(0)
    expect(indexChangedHandler).not.toBeNull()
    const baseline = getProviderUsage.mock.calls.length

    indexChangedHandler?.({ seq: 1, cause: "scan_pass" })
    await vi.advanceTimersByTimeAsync(0)
    expect(getProviderUsage).toHaveBeenCalledTimes(baseline + 1)

    // A resync refetches the list but leaves usage to its own floor/poll.
    indexChangedHandler?.({ seq: 2, cause: "resync" })
    await vi.advanceTimersByTimeAsync(0)
    expect(getProviderUsage).toHaveBeenCalledTimes(baseline + 1)

    unsubscribe()
    vi.useRealTimers()
  })

  // R6: usage freshness while the popover is visible is its own poll,
  // independent of any scan. The session starts visible (see `visible`'s
  // doc comment), so polling starts immediately; `popover:hidden` stops it
  // and `popover:shown` resumes it.
  it("polls usage on its own interval while visible, stopping and resuming with the popover", async () => {
    vi.useFakeTimers()
    vi.setSystemTime("2027-01-15T08:00:00Z")
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    await vi.advanceTimersByTimeAsync(0)
    expect(popoverHiddenHandler).not.toBeNull()
    const baseline = getProviderUsage.mock.calls.length

    await vi.advanceTimersByTimeAsync(60_000)
    expect(getProviderUsage).toHaveBeenCalledTimes(baseline + 1)

    await vi.advanceTimersByTimeAsync(60_000)
    expect(getProviderUsage).toHaveBeenCalledTimes(baseline + 2)

    popoverHiddenHandler?.()
    await vi.advanceTimersByTimeAsync(120_000)
    expect(getProviderUsage).toHaveBeenCalledTimes(baseline + 2)

    popoverShownHandler?.()
    await vi.advanceTimersByTimeAsync(0)
    // `popover:shown` also runs its own immediate usage refresh, independent
    // of the poll it resumes.
    expect(getProviderUsage).toHaveBeenCalledTimes(baseline + 3)

    await vi.advanceTimersByTimeAsync(60_000)
    expect(getProviderUsage).toHaveBeenCalledTimes(baseline + 4)

    unsubscribe()
    vi.useRealTimers()
  })

  // R6: lifecycle `activity` shares one usage floor while the popover is
  // visible, so an active session's totals stay current between passes;
  // hidden, it does nothing, since nobody is looking.
  it("refreshes usage from lifecycle activity at most once per floor, only while visible", async () => {
    vi.useFakeTimers()
    vi.setSystemTime("2027-01-15T08:00:00Z")
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    await vi.advanceTimersByTimeAsync(0)
    expect(lifecycleHandlers.size).toBeGreaterThan(0)
    const baseline = getProviderUsage.mock.calls.length

    emitLifecycleActivity()
    await vi.advanceTimersByTimeAsync(0)
    expect(getProviderUsage).toHaveBeenCalledTimes(baseline + 1)

    // A second event 1 s later lands inside the floor and refreshes nothing
    // further.
    await vi.advanceTimersByTimeAsync(1_000)
    emitLifecycleActivity()
    await vi.advanceTimersByTimeAsync(0)
    expect(getProviderUsage).toHaveBeenCalledTimes(baseline + 1)

    // Hidden, even past the floor, activity refreshes nothing.
    popoverHiddenHandler?.()
    await vi.advanceTimersByTimeAsync(30_000)
    emitLifecycleActivity()
    await vi.advanceTimersByTimeAsync(0)
    expect(getProviderUsage).toHaveBeenCalledTimes(baseline + 1)

    unsubscribe()
    vi.useRealTimers()
  })

  it("re-sorts a revived session to the top of the list", async () => {
    const older = entryPayload({
      sessionId: "session-old",
      timestamp: "2024-01-01T00:00:00.000Z",
    })
    const newer = entryPayload({
      sessionId: "session-new",
      timestamp: "2024-01-02T00:00:00.000Z",
    })
    listRecentSessions.mockResolvedValue([newer, older])
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    await vi.waitFor(() =>
      expect(session.getSnapshot().entries?.map((entry) => entry.sessionId)).toEqual([
        "session-new",
        "session-old",
      ]),
    )
    await vi.waitFor(() => expect(sessionUpdatedHandler).not.toBeNull())

    emitUpdated(
      entryPayload({ sessionId: "session-old", timestamp: "2024-01-03T00:00:00.000Z" }),
    )

    await vi.waitFor(() =>
      expect(session.getSnapshot().entries?.map((entry) => entry.sessionId)).toEqual([
        "session-old",
        "session-new",
      ]),
    )
    unsubscribe()
  })

  it("derives active pills from the registry snapshot, not the row flag", async () => {
    // The backend row says inactive; the registry says the session is live.
    listRecentSessions.mockResolvedValue([
      entryPayload({ sessionId: "session-live", isActive: false }),
      entryPayload({ sessionId: "session-idle", isActive: true }),
    ])
    getLiveSessions.mockResolvedValue({
      seq: 4,
      working: 1,
      total: 1,
      sessions: [
        {
          session: {
            environmentKey: "native",
            agent: "claude-code",
            sessionId: "session-live",
          },
          agent: "claude-code",
          lastActivityAt: 1_800_000_000,
          quiet: false,
        },
      ],
      anonymous: [],
    })
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})

    await vi.waitFor(() => {
      const entries = session.getSnapshot().entries
      expect(entries?.find((entry) => entry.sessionId === "session-live")?.isActive).toBe(true)
      expect(entries?.find((entry) => entry.sessionId === "session-idle")?.isActive).toBe(false)
    })
    // A complete snapshot needs no presence read.
    expect(getLiveSessionsFor).not.toHaveBeenCalled()
    unsubscribe()
  })

  it("asks the registry by name for rows the bounded snapshot omitted", async () => {
    listRecentSessions.mockResolvedValue([
      entryPayload({ sessionId: "session-listed", isActive: false }),
      entryPayload({ sessionId: "session-omitted-live", isActive: false }),
      entryPayload({ sessionId: "session-omitted-idle", isActive: true }),
    ])
    // The registry holds 300 live sessions; the snapshot's rows hold one
    // of them. The two other listed rows are unknown until named.
    getLiveSessions.mockResolvedValue({
      seq: 10,
      working: 300,
      total: 300,
      sessions: [
        {
          session: {
            environmentKey: "native",
            agent: "claude-code",
            sessionId: "session-listed",
          },
          agent: "claude-code",
          lastActivityAt: 1_800_000_000,
          quiet: false,
        },
      ],
      anonymous: [],
    })
    let answer!: (presence: Ipc.LivePresencePayload) => void
    getLiveSessionsFor.mockImplementation(
      () =>
        new Promise<Ipc.LivePresencePayload>((resolve) => {
          answer = resolve
        }),
    )
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})

    await vi.waitFor(() => expect(getLiveSessionsFor).toHaveBeenCalledTimes(1))
    // Only the rows the snapshot did not answer are named.
    expect(getLiveSessionsFor.mock.calls[0]?.[0]).toEqual([
      { environmentKey: "native", agent: "claude-code", sessionId: "session-omitted-live" },
      { environmentKey: "native", agent: "claude-code", sessionId: "session-omitted-idle" },
    ])
    const before = session.getSnapshot().entries
    expect(before?.find((entry) => entry.sessionId === "session-listed")?.isActive).toBe(true)
    // Unknown rows keep their flag rather than flashing off.
    expect(before?.find((entry) => entry.sessionId === "session-omitted-idle")?.isActive).toBe(
      true,
    )
    expect(before?.find((entry) => entry.sessionId === "session-omitted-live")?.isActive).toBe(
      false,
    )

    answer({
      seq: 11,
      present: [
        {
          session: {
            environmentKey: "native",
            agent: "claude-code",
            sessionId: "session-omitted-live",
          },
          agent: "claude-code",
          lastActivityAt: 1_800_000_001,
          quiet: true,
        },
      ],
      absent: [
        { environmentKey: "native", agent: "claude-code", sessionId: "session-omitted-idle" },
      ],
    })
    await vi.waitFor(() => {
      const entries = session.getSnapshot().entries
      expect(
        entries?.find((entry) => entry.sessionId === "session-omitted-live")?.isActive,
      ).toBe(true)
      expect(
        entries?.find((entry) => entry.sessionId === "session-omitted-idle")?.isActive,
      ).toBe(false)
    })
    unsubscribe()
  })
})
