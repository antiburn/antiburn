import { afterEach, describe, expect, it, vi } from "vitest"

import type { ChecksReportPayload } from "../../lib/insightsIpc"
import type { ActivityEntryPayload } from "../../lib/ipc"
import type {
  LiveUsageSummaryPayload,
  ProviderUsageSummaryPayload,
} from "../../lib/providerUsageIpc"
import { MainOverviewSession, type MainOverviewAdapter } from "./MainOverviewSession"

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
  let scanFinished: () => void = () => undefined
  let invalidated: () => void = () => undefined
  let liveChanged: (value: LiveUsageSummaryPayload) => void = () => undefined
  let reportChanged: () => void = () => undefined
  let entryChanged: () => void = () => undefined
  const adapter: MainOverviewAdapter = {
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
    onSessionsInvalidated: vi.fn(async (handler) => {
      invalidated = handler
      return vi.fn()
    }),
    onSessionEntryChanged: vi.fn(async (handler) => {
      entryChanged = handler
      return vi.fn()
    }),
    onScanFinished: vi.fn(async (handler) => {
      scanFinished = handler
      return vi.fn()
    }),
    ...overrides,
  }
  const session = new MainOverviewSession(adapter)
  return {
    adapter,
    session,
    setVisible: (value: boolean) => visible(value),
    scanFinished: () => scanFinished(),
    invalidated: () => invalidated(),
    liveChanged: (value: LiveUsageSummaryPayload) => liveChanged(value),
    reportChanged: () => reportChanged(),
    entryChanged: () => entryChanged(),
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
    entryChanged()
    await vi.waitFor(() =>
      expect(session.getSnapshot().recentSessions?.map((item) => item.sessionId)).toEqual([
        "e",
      ]),
    )
    expect(adapter.getUsage).toHaveBeenCalledOnce()
    stop()
  })
})
