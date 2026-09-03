import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type * as Ipc from "../../lib/ipc"
import {
  EMPTY_PROVIDER_USAGE,
  type ActivityEntryPayload,
  type ScanStatus,
  type SessionAnalysisPayload,
} from "../../lib/ipc"
import { PopoverSession, sessionKey } from "./PopoverSession"
import type { SessionSubject } from "./SessionPane"

const getSessionAnalysis = vi.hoisted(() => vi.fn())
const getSubagentAnalysis = vi.hoisted(() => vi.fn())
const getSessionLimitAllocations = vi.hoisted(() => vi.fn())
const getProviderUsage = vi.hoisted(() => vi.fn())
const setPopoverHeight = vi.hoisted(() => vi.fn())
const listRecentSessions = vi.hoisted(() => vi.fn())
const onSessionEntryChanged = vi.hoisted(() => vi.fn())
const onScanEvent = vi.hoisted(() => vi.fn())

// The analysis, list, and event-subscription commands are overridden. All
// other wrappers keep their real no-shell fallback because `hasShell()` is
// false outside Tauri.
vi.mock("../../lib/ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof Ipc>()
  return {
    ...actual,
    getSessionAnalysis,
    getSubagentAnalysis,
    getSessionLimitAllocations,
    getProviderUsage,
    setPopoverHeight,
    listRecentSessions,
    onSessionEntryChanged,
    onScanEvent,
  }
})

type EntryChangedHandler = (entry: ActivityEntryPayload) => void
type ScanEventHandler = (status: ScanStatus, phase: "started" | "progress" | "finished") => void

let entryChangedHandler: EntryChangedHandler | null = null
let scanEventHandler: ScanEventHandler | null = null

beforeEach(() => {
  entryChangedHandler = null
  scanEventHandler = null
  getSessionAnalysis.mockReset()
  getSessionAnalysis.mockResolvedValue(null)
  getSubagentAnalysis.mockReset()
  getSubagentAnalysis.mockResolvedValue(null)
  setPopoverHeight.mockReset()
  setPopoverHeight.mockResolvedValue(true)
  listRecentSessions.mockReset()
  listRecentSessions.mockResolvedValue([])
  onSessionEntryChanged.mockReset()
  onSessionEntryChanged.mockImplementation(async (handler: EntryChangedHandler) => {
    entryChangedHandler = handler
    return () => {
      entryChangedHandler = null
    }
  })
  onScanEvent.mockReset()
  onScanEvent.mockImplementation(async (handler: ScanEventHandler) => {
    scanEventHandler = handler
    return () => {
      scanEventHandler = null
    }
  })
  getSessionLimitAllocations.mockReset()
  getSessionLimitAllocations.mockResolvedValue({
    generatedAt: "2027-01-15T08:00:00Z",
    allocations: [],
  })
  getProviderUsage.mockReset()
  getProviderUsage.mockResolvedValue(EMPTY_PROVIDER_USAGE)
})

describe("PopoverSession surface presentation", () => {
  const subject: SessionSubject = {
    agent: "claude-code",
    sessionId: "session-1",
    wslDistro: null,
  }

  afterEach(() => {
    vi.unstubAllGlobals()
    vi.useRealTimers()
  })

  it("keeps Usage presented until the winning resize presents the requested session", async () => {
    const pending: ((completed: boolean) => void)[] = []
    setPopoverHeight.mockImplementation(
      () =>
        new Promise<boolean>((resolve) => {
          pending.push(resolve)
        }),
    )
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    await vi.waitFor(() => expect(session.getSnapshot().usage).not.toBeNull())
    expect(pending).toHaveLength(1)
    pending.shift()?.(true)
    await Promise.resolve()

    session.setShowUsage(true)
    expect(session.getSnapshot().showUsage).toBe(true)
    expect(pending).toHaveLength(1)
    pending.shift()?.(true)
    await vi.waitFor(() => expect(session.getSnapshot().presentedSurface).toBe("usage"))

    session.setShowUsage(false)
    session.openSession(subject)
    expect(session.getSnapshot().presentedSurface).toBe("usage")

    pending.shift()?.(true)
    await Promise.resolve()
    expect(session.getSnapshot().presentedSurface).toBe("usage")

    pending.shift()?.(true)
    await vi.waitFor(() => expect(session.getSnapshot().presentedSurface).toBe("session"))
    unsubscribe()
  })

  it("presents equal-height navigation without waiting for native completion", () => {
    setPopoverHeight.mockImplementation(() => new Promise<boolean>(() => {}))
    const session = new PopoverSession()

    session.openSession(subject)

    expect(session.getSnapshot().presentedSurface).toBe("session")
    expect(session.getSnapshot().presentedSession).toEqual(subject)
  })

  it("does not leave Usage presented after the winning contraction fails", async () => {
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    await vi.waitFor(() => expect(session.getSnapshot().usage).not.toBeNull())

    session.setShowUsage(true)
    expect(session.getSnapshot().presentedSurface).toBe("usage")
    setPopoverHeight.mockResolvedValue(false)
    session.setShowUsage(false)
    expect(session.getSnapshot().presentedSurface).toBe("usage")

    await vi.waitFor(() => expect(session.getSnapshot().presentedSurface).toBe("activity"))
    unsubscribe()
  })

  it("requests an immediate native resize when reduced motion is enabled", async () => {
    vi.stubGlobal(
      "matchMedia",
      vi.fn(() => ({ matches: true })),
    )
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})

    await vi.waitFor(() => expect(setPopoverHeight).toHaveBeenCalledWith(700, false))
    unsubscribe()
  })

  it("coalesces overlapping allocation requests into one trailing refresh", async () => {
    let resolveFirst!: () => void
    getSessionLimitAllocations
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveFirst = () =>
              resolve({ generatedAt: "2027-01-15T08:00:00Z", allocations: [] })
          }),
      )
      .mockResolvedValue({ generatedAt: "2027-01-15T08:00:01Z", allocations: [] })
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    await vi.waitFor(() => expect(resolveFirst).toBeTypeOf("function"))
    await vi.waitFor(() => expect(session.getSnapshot().usage).not.toBeNull())
    expect(getSessionLimitAllocations).toHaveBeenCalledTimes(1)

    resolveFirst()

    await vi.waitFor(() => expect(getSessionLimitAllocations).toHaveBeenCalledTimes(2))
    unsubscribe()
  })

  it("updates the snapshot when the next cached allocation expires", async () => {
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
          windowId: "weekly-main",
          resetsAt: "2027-01-15T08:00:01Z",
          percent: 10,
        },
      ],
    })
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    await vi.advanceTimersByTimeAsync(0)
    expect(session.getSnapshot().sessionLimitAllocations.allocations).toHaveLength(1)
    const before = session.getSnapshot().now

    await vi.advanceTimersByTimeAsync(1_001)

    expect(session.getSnapshot().now).toBeGreaterThan(before)
    unsubscribe()
  })
})

/**
 * `sessionKey` tags the analysis load a subject's payload belongs to. Get it
 * wrong and a subject that moves between environments — or a sub-agent whose
 * id happens to collide with one from a different parent — shows another
 * session's cached (or in-flight) analysis instead of its own.
 */

describe("sessionKey", () => {
  it("scopes by environment: the same agent and session id in different WSL distros are distinct", () => {
    const native = sessionKey({ agent: "claude-code", sessionId: "same-id", wslDistro: null })
    const ubuntu = sessionKey({
      agent: "claude-code",
      sessionId: "same-id",
      wslDistro: "Ubuntu",
    })
    const debian = sessionKey({
      agent: "claude-code",
      sessionId: "same-id",
      wslDistro: "Debian",
    })

    expect(native).not.toBe(ubuntu)
    expect(ubuntu).not.toBe(debian)
  })

  it("is case-insensitive on the WSL distribution name", () => {
    const lower = sessionKey({
      agent: "claude-code",
      sessionId: "same-id",
      wslDistro: "ubuntu",
    })
    const upper = sessionKey({
      agent: "claude-code",
      sessionId: "same-id",
      wslDistro: "UBUNTU",
    })

    expect(lower).toBe(upper)
  })

  it("scopes a sub-agent key by its parent session, not just the sub-agent id", () => {
    const parentOne = sessionKey({
      agent: "claude-code",
      sessionId: "same-subagent-id",
      wslDistro: null,
      subagent: { parentSessionId: "parent-one", subagentId: "same-subagent-id" },
    })
    const parentTwo = sessionKey({
      agent: "claude-code",
      sessionId: "same-subagent-id",
      wslDistro: null,
      subagent: { parentSessionId: "parent-two", subagentId: "same-subagent-id" },
    })

    expect(parentOne).not.toBe(parentTwo)
  })

  it("does not collide a sub-agent key with a top-level session of the same id", () => {
    const topLevel = sessionKey({
      agent: "claude-code",
      sessionId: "shared-id",
      wslDistro: null,
    })
    const subagent = sessionKey({
      agent: "claude-code",
      sessionId: "shared-id",
      wslDistro: null,
      subagent: { parentSessionId: "shared-id", subagentId: "sub-1" },
    })

    expect(topLevel).not.toBe(subagent)
  })
})

/**
 * The event-driven refresh behind an open detail pane and the activity list:
 * `sessions:entry-changed` replaces the old fingerprint poll, and
 * `scan:finished` is the list's own backstop when a pass reports no change.
 */
describe("PopoverSession event-driven refresh", () => {
  const subject: SessionSubject = {
    agent: "claude-code",
    sessionId: "session-1",
    wslDistro: null,
  }

  const analysisPayload = (
    overrides: Partial<SessionAnalysisPayload> = {},
  ): SessionAnalysisPayload => ({
    summary: null,
    supportsAnalysis: true,
    title: null,
    wslDistro: null,
    isActive: false,
    cost: null,
    topLevelCost: null,
    subagentsCost: null,
    inclusiveTokens: null,
    subagentsTokens: null,
    efficiency: null,
    models: [],
    modelRuns: [],
    orchestration: null,
    relations: null,
    sourcePath: null,
    startedAtEpoch: null,
    analysisPending: false,
    analysisStale: false,
    ...overrides,
  })

  const entryPayload = (
    overrides: Partial<ActivityEntryPayload> = {},
  ): ActivityEntryPayload => ({
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
  })

  const scanStatus = (overrides: Partial<ScanStatus> = {}): ScanStatus => ({
    running: false,
    completedAgents: 1,
    totalAgents: 1,
    sessions: 1,
    finishedAt: "2024-01-01T00:00:00.000Z",
    cancelled: false,
    error: null,
    agents: [],
    listChanged: false,
    ...overrides,
  })

  it("refreshes the open analysis when a matching entry event lands", async () => {
    getSessionAnalysis.mockResolvedValue(analysisPayload())
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    session.openSession(subject)
    await vi.waitFor(() => expect(getSessionAnalysis).toHaveBeenCalledTimes(1))
    await vi.waitFor(() => expect(entryChangedHandler).not.toBeNull())

    entryChangedHandler?.(entryPayload())

    await vi.waitFor(() => expect(getSessionAnalysis).toHaveBeenCalledTimes(2))
    unsubscribe()
  })

  it("refreshes a sub-agent subject's analysis on its parent's entry event", async () => {
    getSubagentAnalysis.mockResolvedValue(analysisPayload())
    const subagent: SessionSubject = {
      ...subject,
      subagent: { parentSessionId: subject.sessionId, subagentId: "subagent-1" },
    }
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    session.openSession(subagent)
    await vi.waitFor(() => expect(getSubagentAnalysis).toHaveBeenCalledTimes(1))
    await vi.waitFor(() => expect(entryChangedHandler).not.toBeNull())

    entryChangedHandler?.(entryPayload({ sessionId: subject.sessionId }))

    await vi.waitFor(() => expect(getSubagentAnalysis).toHaveBeenCalledTimes(2))
    unsubscribe()
  })

  it("does not refresh the open analysis when the entry event names a different session", async () => {
    getSessionAnalysis.mockResolvedValue(analysisPayload())
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    session.openSession(subject)
    await vi.waitFor(() => expect(getSessionAnalysis).toHaveBeenCalledTimes(1))
    await vi.waitFor(() => expect(entryChangedHandler).not.toBeNull())

    entryChangedHandler?.(entryPayload({ sessionId: "another-session" }))
    await new Promise((resolve) => setTimeout(resolve, 0))

    expect(getSessionAnalysis).toHaveBeenCalledTimes(1)
    unsubscribe()
  })

  it("coalesces two matching events that land during one in-flight refresh into exactly one more", async () => {
    getSessionAnalysis.mockResolvedValue(analysisPayload())
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    session.openSession(subject)
    await vi.waitFor(() => expect(getSessionAnalysis).toHaveBeenCalledTimes(1))
    await vi.waitFor(() => expect(entryChangedHandler).not.toBeNull())

    const pendingResolvers: ((payload: SessionAnalysisPayload) => void)[] = []
    getSessionAnalysis.mockImplementationOnce(
      () =>
        new Promise<SessionAnalysisPayload>((resolve) => {
          pendingResolvers.push(resolve)
        }),
    )
    const matching = entryPayload()
    entryChangedHandler?.(matching)
    await vi.waitFor(() => expect(getSessionAnalysis).toHaveBeenCalledTimes(2))

    // Both land while the refresh above is still in flight.
    entryChangedHandler?.(matching)
    entryChangedHandler?.(matching)

    pendingResolvers.shift()?.(analysisPayload())
    await vi.waitFor(() => expect(getSessionAnalysis).toHaveBeenCalledTimes(3))

    // No further call follows the coalesced one.
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(getSessionAnalysis).toHaveBeenCalledTimes(3)
    unsubscribe()
  })

  it("refetches the list once for an entry event whose session is not on screen", async () => {
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    await vi.waitFor(() => expect(listRecentSessions).toHaveBeenCalledTimes(1))
    await vi.waitFor(() => expect(entryChangedHandler).not.toBeNull())

    entryChangedHandler?.(entryPayload({ sessionId: "unknown-session" }))

    await vi.waitFor(() => expect(listRecentSessions).toHaveBeenCalledTimes(2))
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(listRecentSessions).toHaveBeenCalledTimes(2)
    unsubscribe()
  })

  it("does not refetch on a scan:finished within the reconcile interval when the list did not change", async () => {
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    await vi.waitFor(() => expect(listRecentSessions).toHaveBeenCalledTimes(1))
    await vi.waitFor(() => expect(scanEventHandler).not.toBeNull())

    scanEventHandler?.(scanStatus({ listChanged: true }), "finished")
    await vi.waitFor(() => expect(listRecentSessions).toHaveBeenCalledTimes(2))

    scanEventHandler?.(scanStatus({ listChanged: false }), "finished")
    await new Promise((resolve) => setTimeout(resolve, 0))

    expect(listRecentSessions).toHaveBeenCalledTimes(2)
    unsubscribe()
  })

  // F1: `scan:finished` refreshes usage on its own floor
  // (`USAGE_REFRESH_MIN_MS`), separate from the list reconcile interval
  // above — a scan pass finishing is not by itself a reason to recompute
  // 30-day usage totals on every one of an active session's rapid passes.
  it("refreshes usage on a floor, bypassed by a reported list change", async () => {
    vi.useFakeTimers()
    vi.setSystemTime("2027-01-15T08:00:00Z")
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    await vi.advanceTimersByTimeAsync(0)
    expect(scanEventHandler).not.toBeNull()
    const baseline = getProviderUsage.mock.calls.length

    // First scan:finished always refreshes: nothing has refreshed usage
    // from this handler yet.
    scanEventHandler?.(scanStatus({ listChanged: false }), "finished")
    await vi.advanceTimersByTimeAsync(0)
    expect(getProviderUsage).toHaveBeenCalledTimes(baseline + 1)

    // A second event 1 s later, still no list change, lands inside the
    // floor and refreshes nothing further.
    await vi.advanceTimersByTimeAsync(1_000)
    scanEventHandler?.(scanStatus({ listChanged: false }), "finished")
    await vi.advanceTimersByTimeAsync(0)
    expect(getProviderUsage).toHaveBeenCalledTimes(baseline + 1)

    // A reported list change bypasses the floor.
    scanEventHandler?.(scanStatus({ listChanged: true }), "finished")
    await vi.advanceTimersByTimeAsync(0)
    expect(getProviderUsage).toHaveBeenCalledTimes(baseline + 2)

    // Once the floor elapses, an unchanged list refreshes again too.
    await vi.advanceTimersByTimeAsync(30_000)
    scanEventHandler?.(scanStatus({ listChanged: false }), "finished")
    await vi.advanceTimersByTimeAsync(0)
    expect(getProviderUsage).toHaveBeenCalledTimes(baseline + 3)

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
    await vi.waitFor(() => expect(entryChangedHandler).not.toBeNull())

    entryChangedHandler?.(
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
})
