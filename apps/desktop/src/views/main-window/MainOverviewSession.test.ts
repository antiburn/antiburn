import { afterEach, describe, expect, it, vi } from "vitest"

import type { ChecksReportPayload } from "../../lib/insightsIpc"
import type {
  ActivityEntryPayload,
  SessionIndexChangedPayload,
  SessionRefPayload,
  SessionUpdatedPayload,
} from "../../lib/ipc"
import type {
  LiveUsageSummaryPayload,
  ProviderUsageSummaryPayload,
} from "../../lib/providerUsageIpc"
import {
  sessionRefKey,
  type LiveSessionsSnapshot,
  type LiveSessionsSource,
  type TrackedSession,
} from "../../lib/sessionLifecycle"
import {
  MainOverviewSession,
  overviewUpdateTouchesTotals,
  type MainOverviewAdapter,
} from "./MainOverviewSession"

const usage = (generatedAt: string): ProviderUsageSummaryPayload => ({
  providers: [],
  days: [],
  previousDays: [],
  generatedAt,
})

const liveUsage = (generatedAt: string): LiveUsageSummaryPayload => ({
  providers: [],
  errors: [],
  meters: [],
  generatedAt,
})

const report = (pendingEvidence: number): ChecksReportPayload => ({
  evidenceSettled: pendingEvidence === 0,
  pendingEvidence,
  estimatedTokenBurnBasisPoints: null,
  categories: [],
})

const entry = (sessionId: string, timestamp: string): ActivityEntryPayload => ({
  agent: "claude",
  sessionId,
  repo: "antiburn",
  timestamp,
  isActive: false,
  surface: "cli",
  wslDistro: null,
  title: sessionId,
  hasForkParent: false,
  forkChildCount: 0,
  cost: null,
  models: [],
  modelRuns: [],
})

const indexChanged = (
  cause: SessionIndexChangedPayload["cause"] = "scan_pass",
): SessionIndexChangedPayload => ({ seq: 1, cause })

const update = (
  changed: ActivityEntryPayload,
  facets: Partial<SessionUpdatedPayload["facets"]> = { metadata: true },
): SessionUpdatedPayload => ({
  seq: 1,
  session: { environmentKey: "native", agent: changed.agent, sessionId: changed.sessionId },
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

/**
 * A scripted registry tracker: the test decides which identities are live,
 * whether the base is complete, and which registered interests are absent.
 */
function fakeLiveSessions() {
  const listeners = new Set<() => void>()
  const interests = new Map<object, Map<string, unknown>>()
  let snapshot: LiveSessionsSnapshot = {
    seq: 0,
    ready: false,
    sessions: new Map(),
    keylessAgents: new Set(),
    working: 0,
    total: 0,
    anonymous: 0,
    sweep: [],
    complete: false,
    absent: new Set(),
  }
  const source: LiveSessionsSource = {
    subscribe: vi.fn((listener: () => void) => {
      listeners.add(listener)
      return () => listeners.delete(listener)
    }),
    getSnapshot: () => snapshot,
    setInterest: vi.fn((owner: object, refs: readonly SessionRefPayload[]) => {
      interests.set(owner, new Map(refs.map((ref) => [sessionRefKey(ref), ref])))
    }),
    clearInterest: vi.fn((owner: object) => {
      interests.delete(owner)
    }),
  }
  return {
    source,
    interests,
    listenerCount: () => listeners.size,
    publish(change: Partial<LiveSessionsSnapshot> & { live?: Record<string, TrackedSession> }) {
      const { live, ...rest } = change
      snapshot = {
        ...snapshot,
        ...rest,
        ...(live ? { sessions: new Map(Object.entries(live)) } : {}),
      }
      for (const listener of listeners) listener()
    },
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

function setup(visibleInitially = true, overrides: Partial<MainOverviewAdapter> = {}) {
  let visible: (value: boolean) => void = () => undefined
  let indexChangedHandler: (change: SessionIndexChangedPayload) => void = () => undefined
  let liveChanged: (value: LiveUsageSummaryPayload) => void = () => undefined
  let reportChanged: () => void = () => undefined
  let updated: (change: SessionUpdatedPayload) => void = () => undefined
  const live = fakeLiveSessions()
  const adapter: MainOverviewAdapter = {
    liveSessions: live.source,
    getUsage: vi.fn().mockResolvedValue(usage("first")),
    getLiveUsage: vi.fn().mockResolvedValue(liveUsage("live-first")),
    getChecksReport: vi.fn().mockResolvedValue(report(0)),
    cancelChecksReport: vi.fn().mockResolvedValue(undefined),
    listRecentSessions: vi
      .fn()
      .mockResolvedValue([
        entry("b", "2026-09-13T10:00:00Z"),
        entry("d", "2026-09-14T08:00:00Z"),
        entry("a", "2026-09-12T10:00:00Z"),
        entry("c", "2026-09-14T06:00:00Z"),
      ]),
    getVisible: vi.fn().mockResolvedValue(visibleInitially),
    onVisible: vi.fn(async (handler) => {
      visible = handler
      return vi.fn()
    }),
    onLiveUsageChanged: vi.fn(async (handler) => {
      liveChanged = handler
      return vi.fn()
    }),
    onChecksReportChanged: vi.fn(async (handler) => {
      reportChanged = handler
      return vi.fn()
    }),
    onSessionIndexChanged: vi.fn(async (handler) => {
      indexChangedHandler = handler
      return vi.fn()
    }),
    onSessionUpdated: vi.fn(async (handler) => {
      updated = handler
      return vi.fn()
    }),
    ...overrides,
  }
  const session = new MainOverviewSession(adapter)
  return {
    adapter,
    session,
    live,
    setVisible: (value: boolean) => visible(value),
    scanFinished: () => indexChangedHandler(indexChanged("scan_pass")),
    invalidated: () => indexChangedHandler(indexChanged("invalidated")),
    indexChanged: (cause: SessionIndexChangedPayload["cause"]) =>
      indexChangedHandler(indexChanged(cause)),
    liveChanged: (value: LiveUsageSummaryPayload) => liveChanged(value),
    reportChanged: () => reportChanged(),
    entryChanged: (facets?: Partial<SessionUpdatedPayload["facets"]>) =>
      updated(update(entry("e", "2026-09-14T09:00:00Z"), facets)),
  }
}

const sessions: MainOverviewSession[] = []
afterEach(() => sessions.splice(0).forEach((session) => session.dispose()))

describe("MainOverviewSession", () => {
  it("loads only while the window is visible and a viewer is active", async () => {
    const { adapter, session, setVisible } = setup(false)
    sessions.push(session)
    const stop = session.subscribeInactive(() => undefined)
    await vi.waitFor(() => expect(adapter.getVisible).toHaveBeenCalled())
    setVisible(true)
    expect(adapter.getUsage).not.toHaveBeenCalled()
    const stopActive = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(adapter.getUsage).toHaveBeenCalledOnce())
    await vi.waitFor(() => expect(session.getSnapshot().usage?.generatedAt).toBe("first"))
    expect(session.getSnapshot().liveUsage?.generatedAt).toBe("live-first")
    setVisible(false)
    expect(session.getSnapshot().active).toBe(false)
    stopActive()
    stop()
  })

  it("refreshes the whole page on every index change cause", async () => {
    const { adapter, session, indexChanged } = setup()
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().usage).not.toBeNull())
    let expected = 1
    for (const cause of ["scan_pass", "invalidated", "removed", "resync"] as const) {
      indexChanged(cause)
      expected += 1
      await vi.waitFor(() => expect(adapter.getUsage).toHaveBeenCalledTimes(expected))
    }
    stop()
  })

  it("refreshes after a scan and after an invalidation, and coalesces bursts", async () => {
    const { adapter, session, scanFinished, invalidated } = setup()
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().usage).not.toBeNull())
    const pending = deferred<ProviderUsageSummaryPayload>()
    vi.mocked(adapter.getUsage).mockReturnValueOnce(pending.promise)
    scanFinished()
    invalidated()
    scanFinished()
    expect(adapter.getUsage).toHaveBeenCalledTimes(2)
    expect(session.getSnapshot().refreshing).toBe(true)
    expect(session.getSnapshot().loading).toBe(false)
    pending.resolve(usage("second"))
    await vi.waitFor(() => expect(adapter.getUsage).toHaveBeenCalledTimes(3))
    await vi.waitFor(() => expect(session.getSnapshot().refreshing).toBe(false))
    stop()
  })

  it("keeps the last usage after a failed read and flags the error", async () => {
    const { adapter, session, scanFinished } = setup()
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().usage).not.toBeNull())
    vi.mocked(adapter.getUsage).mockRejectedValueOnce(new Error("Unavailable"))
    scanFinished()
    await vi.waitFor(() => expect(session.getSnapshot().usageError).toBe(true))
    expect(session.getSnapshot().usage?.generatedAt).toBe("first")
    session.refresh()
    await vi.waitFor(() => expect(session.getSnapshot().usageError).toBe(false))
    stop()
  })

  it("rejects a hidden read's result and takes pushed live usage only while active", async () => {
    const pending = deferred<ProviderUsageSummaryPayload>()
    const { adapter, session, setVisible, liveChanged } = setup()
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().usage).not.toBeNull())
    vi.mocked(adapter.getUsage).mockReturnValueOnce(pending.promise)
    session.refresh()
    setVisible(false)
    liveChanged(liveUsage("live-hidden"))
    pending.resolve(usage("hidden"))
    await Promise.resolve()
    expect(session.getSnapshot().usage?.generatedAt).toBe("first")
    expect(session.getSnapshot().liveUsage?.generatedAt).toBe("live-first")
    setVisible(true)
    liveChanged(liveUsage("live-pushed"))
    expect(session.getSnapshot().liveUsage?.generatedAt).toBe("live-pushed")
    await vi.waitFor(() => expect(adapter.getUsage).toHaveBeenCalledTimes(3))
    stop()
  })

  it("reads the checks report under its own consumer and releases it when inactive", async () => {
    const { adapter, session, setVisible, reportChanged } = setup()
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().report).not.toBeNull())
    const consumerId = vi.mocked(adapter.getChecksReport).mock.calls[0]?.[0]
    expect(consumerId).toMatch(/^main-home-\d+$/)
    vi.mocked(adapter.getChecksReport).mockResolvedValueOnce(report(2))
    reportChanged()
    await vi.waitFor(() => expect(session.getSnapshot().report?.pendingEvidence).toBe(2))
    expect(adapter.getUsage).toHaveBeenCalledOnce()
    setVisible(false)
    expect(adapter.cancelChecksReport).toHaveBeenCalledWith(consumerId)
    stop()
  })

  it("keeps the three newest sessions and re-reads them when an entry changes", async () => {
    const { adapter, session, entryChanged } = setup()
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().recentSessions).not.toBeNull())
    expect(session.getSnapshot().recentSessions?.map((item) => item.sessionId)).toEqual([
      "d",
      "c",
      "b",
    ])
    vi.mocked(adapter.listRecentSessions).mockResolvedValueOnce([
      entry("e", "2026-09-14T09:00:00Z"),
    ])
    entryChanged({ title: true })
    await vi.waitFor(() =>
      expect(session.getSnapshot().recentSessions?.map((item) => item.sessionId)).toEqual([
        "e",
      ]),
    )
    expect(adapter.getUsage).toHaveBeenCalledOnce()
    stop()
  })

  it("re-reads the totals when a row update touches what they count", async () => {
    const { adapter, session, entryChanged } = setup()
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().usage).not.toBeNull())
    entryChanged({ metadata: true })
    await vi.waitFor(() => expect(adapter.getUsage).toHaveBeenCalledTimes(2))
    entryChanged({ analysis: true })
    await vi.waitFor(() => expect(adapter.getUsage).toHaveBeenCalledTimes(3))
    await vi.waitFor(() => expect(session.getSnapshot().refreshing).toBe(false))
    entryChanged({ title: true })
    await vi.waitFor(() => expect(adapter.listRecentSessions).toHaveBeenCalledTimes(4))
    expect(adapter.getUsage).toHaveBeenCalledTimes(3)
    stop()
  })

  it("derives recent-row pills from the registry and registers the rows as its interest", async () => {
    const { session, live } = setup()
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().recentSessions).not.toBeNull())
    // Rows loaded before the registry answered keep the backend flag.
    expect(session.getSnapshot().recentSessions?.map((item) => item.isActive)).toEqual([
      false,
      false,
      false,
    ])
    const owner = [...live.interests.keys()][0]
    expect(owner).toBe(session)
    expect([...(live.interests.get(session)?.keys() ?? [])]).toEqual([
      JSON.stringify(["native", "claude", "d"]),
      JSON.stringify(["native", "claude", "c"]),
      JSON.stringify(["native", "claude", "b"]),
    ])

    // A truncated base names `d` live and says nothing about `c`; a presence
    // answer then says `b` is absent. Only evidence moves a pill; `c` keeps
    // its flag until the registry names it.
    live.publish({
      ready: true,
      seq: 5,
      complete: false,
      live: {
        [JSON.stringify(["native", "claude", "d"])]: {
          agent: "claude",
          lastActivityAt: 1,
          quiet: false,
        },
      },
      absent: new Set([JSON.stringify(["native", "claude", "b"])]),
    })
    expect(
      session.getSnapshot().recentSessions?.map((item) => [item.sessionId, item.isActive]),
    ).toEqual([
      ["d", true],
      ["c", false],
      ["b", false],
    ])

    // A complete base makes every unlisted row inactive without a timestamp.
    live.publish({ seq: 6, complete: true, live: {} })
    expect(session.getSnapshot().recentSessions?.every((item) => !item.isActive)).toBe(true)
    live.publish({
      seq: 7,
      live: {
        [JSON.stringify(["native", "claude", "c"])]: {
          agent: "claude",
          lastActivityAt: 2,
          quiet: true,
        },
      },
    })
    expect(
      session.getSnapshot().recentSessions?.find((item) => item.sessionId === "c")?.isActive,
    ).toBe(true)

    // Stopping removes the interest and the registry subscription.
    stop()
    expect(live.interests.size).toBe(0)
    expect(live.listenerCount()).toBe(0)
  })

  it("classifies which facets move the totals", () => {
    const row = entry("a", "2026-09-12T10:00:00Z")
    expect(overviewUpdateTouchesTotals(update(row, { title: true }))).toBe(false)
    expect(overviewUpdateTouchesTotals(update(row, {}))).toBe(false)
    for (const facet of ["metadata", "analysis", "usage", "checks", "limits"] as const) {
      expect(overviewUpdateTouchesTotals(update(row, { [facet]: true }))).toBe(true)
    }
  })
})

it.each(["hidden", "inactive"])(
  "suspends Overview lifecycle overlays while %s and reconciles on resume",
  async (mode) => {
    const { session, live, setVisible } = setup()
    sessions.push(session)
    const listener = vi.fn()
    const stopInactive = session.subscribeInactive(listener)
    const stopActive = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().recentSessions).not.toBeNull())
    expect(live.interests.has(session)).toBe(true)
    if (mode === "hidden") setVisible(false)
    else stopActive()
    expect(session.getSnapshot().active).toBe(false)
    const before = session.getSnapshot()
    listener.mockClear()
    live.publish({
      ready: true,
      complete: true,
      live: {
        '["native","claude","d"]': { agent: "claude", lastActivityAt: 100, quiet: false },
      },
    })
    expect(session.getSnapshot()).toBe(before)
    expect(listener).not.toHaveBeenCalled()
    expect(live.interests.has(session)).toBe(false)
    if (mode === "hidden") setVisible(true)
    else session.subscribe(() => undefined)
    expect(live.interests.has(session)).toBe(true)
    expect(
      session.getSnapshot().recentSessions?.find((row) => row.sessionId === "d")?.isActive,
    ).toBe(true)
    stopInactive()
  },
)
