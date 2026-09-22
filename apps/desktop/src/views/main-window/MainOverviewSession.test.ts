import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { toActivityEntry } from "../../lib/activityEntries"
import type { SessionListEntry } from "../../components/session/SessionList"

import type * as Ipc from "../../lib/ipc"
import type {
  ActivityEntryPayload,
  SessionIndexChangedPayload,
  SessionUpdatedPayload,
} from "../../lib/ipc"
import type {
  AllowanceUsageAccountPayload,
  AllowanceUsageSummaryPayload,
  LiveProviderUsagePayload,
  LiveUsageSummaryPayload,
  ProviderUsageSummaryPayload,
} from "../../lib/providerUsageIpc"
import { readOverviewViewPrefs, writeOverviewViewPrefs } from "./overview/overviewViewPrefs"
import {
  MainOverviewSession,
  overviewUpdateTouchesTotals,
  type MainOverviewAdapter,
  type MainOverviewScanSource,
  type MainOverviewSessionListSource,
} from "./MainOverviewSession"

const mainWindowContentReady = vi.hoisted(() => vi.fn().mockResolvedValue(undefined))

// Only `mainWindowContentReady` is overridden: the test adapter stands in
// for every other ipc call, so the real wrappers underneath it never run.
vi.mock("../../lib/ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof Ipc>()
  return { ...actual, mainWindowContentReady }
})

const usage = (generatedAt: string): ProviderUsageSummaryPayload => ({
  providers: [],
  days: [],
  previousDays: [],
  generatedAt,
})

const allowance = (
  generatedAt: string,
  accounts: AllowanceUsageAccountPayload[] = [],
): AllowanceUsageSummaryPayload => ({
  utilizationSpanDays: 28,
  accounts,
  rangeStartEpoch: 0,
  rangeEndEpoch: 30 * 86400,
  generatedAt,
})

const liveUsage = (
  generatedAt: string,
  providers: LiveProviderUsagePayload[] = [],
): LiveUsageSummaryPayload => ({
  providers,
  errors: [],
  meters: [],
  generatedAt,
})

const accountWithPlan = (): AllowanceUsageAccountPayload => ({
  provider: "anthropic",
  displayName: "Claude",
  accountKey: "account",
  plan: { name: "max", tier: null },
  utilization: null,
  chart: { shortWindows: [], weeklyWindows: [], rolling: [] },
})

const liveProviderWithPlan = (): LiveProviderUsagePayload => ({
  provider: "anthropic",
  accountKey: "account",
  displayName: "Claude",
  support: "live",
  freshness: "fresh",
  sourceLabel: "Test",
  observedAt: "2026-09-22T00:00:00Z",
  windows: [],
  extraUsage: null,
  resetCredits: null,
  plan: { name: "max", tier: null },
  accountUuid: null,
  accountEmail: null,
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
  totalTokens: 0,
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

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason: unknown) => void
  const promise = new Promise<T>((done, fail) => {
    resolve = done
    reject = fail
  })
  return { promise, resolve, reject }
}

/** Long enough to prove the debounce coalesces, short enough not to slow the
 *  suite. The production value is `OVERVIEW_REFRESH_DEBOUNCE_MS`. */
const TEST_DEBOUNCE_MS = 5

/** Far enough out that only the test that asks for it ever reaches the cap. */
const TEST_SCAN_HOLD_CAP_MS = 10_000

interface SetupOptions {
  scanRunning?: boolean
  debounceMs?: number
  scanHoldCapMs?: number
  rememberPlan?: (hadPlan: boolean) => void
}

function setup(
  visibleInitially = true,
  overrides: Partial<MainOverviewAdapter> = {},
  options: SetupOptions = {},
) {
  let visible: (value: boolean) => void = () => undefined
  let indexChangedHandler: (change: SessionIndexChangedPayload) => void = () => undefined
  let updated: (change: SessionUpdatedPayload) => void = () => undefined
  let liveUsageChanged: (value: LiveUsageSummaryPayload) => void = () => undefined
  let entries: SessionListEntry[] = [
    entry("b", "2026-09-13T10:00:00Z"),
    entry("d", "2026-09-14T08:00:00Z"),
    entry("a", "2026-09-12T10:00:00Z"),
    entry("c", "2026-09-14T06:00:00Z"),
  ].map(toActivityEntry)
  const listListeners = new Set<() => void>()
  const sessionList = {
    getSnapshot: () => ({ entries }),
    subscribeList: (listener: () => void) => {
      listListeners.add(listener)
      return () => listListeners.delete(listener)
    },
  }
  const adapter: MainOverviewAdapter = {
    getSessionLimitAllocations: vi
      .fn()
      .mockResolvedValue({ allocations: [], generatedAt: "first" }),
    getUsage: vi.fn().mockResolvedValue(usage("first")),
    getAllowanceUsage: vi.fn().mockResolvedValue(allowance("allowance-first")),
    getLiveUsage: vi.fn().mockResolvedValue(liveUsage("live-first")),
    getVisible: vi.fn().mockResolvedValue(visibleInitially),
    onVisible: vi.fn(async (handler) => {
      visible = handler
      return vi.fn()
    }),
    onLiveUsageChanged: vi.fn(async (handler) => {
      liveUsageChanged = handler
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
  let scanRunning = options.scanRunning ?? false
  const scanListeners = new Set<() => void>()
  const scanSource: MainOverviewScanSource = {
    getSnapshot: () => ({ running: scanRunning }),
    subscribe: (listener: () => void) => {
      scanListeners.add(listener)
      return () => scanListeners.delete(listener)
    },
  }
  const session = new MainOverviewSession(sessionList, adapter, {
    scanSource,
    debounceMs: options.debounceMs ?? TEST_DEBOUNCE_MS,
    scanHoldCapMs: options.scanHoldCapMs ?? TEST_SCAN_HOLD_CAP_MS,
    ...(options.rememberPlan ? { rememberPlan: options.rememberPlan } : {}),
  })
  return {
    adapter,
    session,
    setScanRunning: (value: boolean) => {
      scanRunning = value
      for (const listener of scanListeners) listener()
    },
    setVisible: (value: boolean) => visible(value),
    scanFinished: () => indexChangedHandler(indexChanged("scan_pass")),
    invalidated: () => indexChangedHandler(indexChanged("invalidated")),
    indexChanged: (cause: SessionIndexChangedPayload["cause"]) =>
      indexChangedHandler(indexChanged(cause)),
    meterChanged: (value: LiveUsageSummaryPayload = liveUsage("live-pushed")) =>
      liveUsageChanged(value),
    entryChanged: (facets?: Partial<SessionUpdatedPayload["facets"]>) =>
      updated(update(entry("e", "2026-09-14T09:00:00Z"), facets)),
    setEntries: (next: ActivityEntryPayload[]) => {
      entries = next.map(toActivityEntry)
      for (const listener of listListeners) listener()
    },
  }
}

const sessions: MainOverviewSession[] = []
afterEach(() => sessions.splice(0).forEach((session) => session.dispose()))

beforeEach(() => {
  mainWindowContentReady.mockClear()
  Object.defineProperty(window, "__ANTIBURN_WINDOW_GENERATION__", {
    configurable: true,
    value: 7,
  })
  // The production `rememberPlan` default reads and writes real prefs
  // storage, so each test starts with no memory of a previous run.
  localStorage.clear()
})

describe("MainOverviewSession", () => {
  it("uses the main-window list source for recent sessions", async () => {
    const { adapter } = setup()
    let entries = [
      { ...toActivityEntry(entry("first", "2026-09-14T08:00:00Z")), title: "first" },
    ]
    const listeners = new Set<() => void>()
    const source: MainOverviewSessionListSource = {
      getSnapshot: () => ({ entries }),
      subscribeList: (listener) => {
        listeners.add(listener)
        return () => listeners.delete(listener)
      },
    }
    const session = new MainOverviewSession(source, adapter)
    sessions.push(session)
    const stop = session.subscribe(() => undefined)

    await vi.waitFor(() => expect(session.getSnapshot().recentSessions).not.toBeNull())
    expect(session.getSnapshot().recentSessions?.[0]?.sessionId).toBe("first")

    entries = [{ ...toActivityEntry(entry("second", "2026-09-14T09:00:00Z")), title: "second" }]
    for (const listener of listeners) listener()
    await vi.waitFor(() =>
      expect(session.getSnapshot().recentSessions?.[0]?.sessionId).toBe("second"),
    )
    stop()
  })

  it("defers shared-list updates while inactive and catches up on resume", async () => {
    const { session, setEntries, setVisible } = setup()
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().recentSessions).not.toBeNull())

    setVisible(false)
    setEntries([entry("hidden-update", "2026-09-15T09:00:00Z")])
    expect(session.getSnapshot().recentSessions?.[0]?.sessionId).not.toBe("hidden-update")

    setVisible(true)
    await vi.waitFor(() =>
      expect(session.getSnapshot().recentSessions?.[0]?.sessionId).toBe("hidden-update"),
    )
    stop()
  })

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
    scanFinished()
    invalidated()
    scanFinished()
    // Nothing yet: the burst is still arriving.
    expect(adapter.getUsage).toHaveBeenCalledTimes(1)
    await vi.waitFor(() => expect(adapter.getUsage).toHaveBeenCalledTimes(2))
    await vi.waitFor(() => expect(session.getSnapshot().refreshing).toBe(false))
    // One read for the three events, not one each.
    expect(adapter.getUsage).toHaveBeenCalledTimes(2)
    stop()
  })

  it("coalesces a burst that spans an in-flight read", async () => {
    const { adapter, session, scanFinished, invalidated } = setup()
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().usage).not.toBeNull())
    const pending = deferred<ProviderUsageSummaryPayload>()
    vi.mocked(adapter.getUsage).mockReturnValueOnce(pending.promise)
    scanFinished()
    await vi.waitFor(() => expect(adapter.getUsage).toHaveBeenCalledTimes(2))
    expect(session.getSnapshot().refreshing).toBe(true)
    expect(session.getSnapshot().loading).toBe(false)
    // Events either side of a read in flight still collapse into one more.
    invalidated()
    scanFinished()
    pending.resolve(usage("second"))
    await vi.waitFor(() => expect(adapter.getUsage).toHaveBeenCalledTimes(3))
    await vi.waitFor(() => expect(session.getSnapshot().refreshing).toBe(false))
    expect(adapter.getUsage).toHaveBeenCalledTimes(3)
    stop()
  })

  it("coalesces allowance changes behind one in-flight quota read", async () => {
    const { adapter, session, scanFinished, meterChanged, setVisible } = setup()
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() =>
      expect(session.getSnapshot().allowance?.generatedAt).toBe("allowance-first"),
    )
    const pending = deferred<AllowanceUsageSummaryPayload>()
    vi.mocked(adapter.getAllowanceUsage).mockReturnValueOnce(pending.promise)
    meterChanged()
    await vi.waitFor(() => expect(adapter.getAllowanceUsage).toHaveBeenCalledTimes(2))
    scanFinished()
    meterChanged()
    scanFinished()
    expect(adapter.getAllowanceUsage).toHaveBeenCalledTimes(2)
    setVisible(false)
    pending.resolve(allowance("stale"))
    await vi.waitFor(() => expect(session.getSnapshot().active).toBe(false))
    expect(session.getSnapshot().allowance?.generatedAt).toBe("allowance-first")
    expect(adapter.getAllowanceUsage).toHaveBeenCalledTimes(2)
    setVisible(true)
    await vi.waitFor(() => expect(adapter.getAllowanceUsage).toHaveBeenCalledTimes(3))
    await vi.waitFor(() =>
      expect(session.getSnapshot().allowance?.generatedAt).toBe("allowance-first"),
    )
    stop()
  })

  it("takes a live-usage push while active and still refreshes the allowance", async () => {
    const { adapter, session, meterChanged, setVisible } = setup()
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() =>
      expect(session.getSnapshot().liveUsage?.generatedAt).toBe("live-first"),
    )
    await vi.waitFor(() =>
      expect(session.getSnapshot().allowance?.generatedAt).toBe("allowance-first"),
    )
    vi.mocked(adapter.getAllowanceUsage).mockResolvedValueOnce(allowance("allowance-pushed"))
    meterChanged(liveUsage("live-pushed"))
    expect(session.getSnapshot().liveUsage?.generatedAt).toBe("live-pushed")
    expect(session.getSnapshot().liveUsageSettled).toBe(true)
    await vi.waitFor(() =>
      expect(session.getSnapshot().allowance?.generatedAt).toBe("allowance-pushed"),
    )
    // A push while the section is inactive answers for a screen nobody sees.
    setVisible(false)
    meterChanged(liveUsage("live-hidden"))
    expect(session.getSnapshot().liveUsage?.generatedAt).toBe("live-pushed")
    stop()
  })

  it("does not let a slow live-usage read overwrite a newer push", async () => {
    const pending = deferred<LiveUsageSummaryPayload>()
    const { adapter, session, meterChanged, scanFinished } = setup()
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() =>
      expect(session.getSnapshot().liveUsage?.generatedAt).toBe("live-first"),
    )
    vi.mocked(adapter.getLiveUsage).mockReturnValueOnce(pending.promise)
    // Starts a read that stays in flight, then a push lands before it answers.
    scanFinished()
    meterChanged(liveUsage("live-pushed"))
    expect(session.getSnapshot().liveUsage?.generatedAt).toBe("live-pushed")
    pending.resolve(liveUsage("live-stale"))
    await pending.promise
    await Promise.resolve()
    expect(session.getSnapshot().liveUsage?.generatedAt).toBe("live-pushed")
    stop()
  })

  it("settles liveUsage after a failed read with no source to show", async () => {
    const { adapter, session } = setup()
    sessions.push(session)
    vi.mocked(adapter.getLiveUsage).mockRejectedValueOnce(new Error("Unavailable"))
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().liveUsageSettled).toBe(true))
    expect(session.getSnapshot().liveUsage).toBeNull()
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

  it("keeps the last allowance figures after a failed read", async () => {
    // The cost totals and the allowance totals are separate reads. A failed
    // allowance read must not blank the page the reader is looking at.
    const { adapter, session, scanFinished } = setup()
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().allowance).not.toBeNull())
    expect(session.getSnapshot().allowance?.generatedAt).toBe("allowance-first")
    vi.mocked(adapter.getAllowanceUsage).mockRejectedValueOnce(new Error("Unavailable"))
    scanFinished()
    // The mock records the call before the rejection reaches the catch, so
    // the test waits for the state it checks.
    await vi.waitFor(() => expect(session.getSnapshot().allowanceError).toBe(true))
    expect(adapter.getAllowanceUsage).toHaveBeenCalledTimes(2)
    expect(session.getSnapshot().allowance?.generatedAt).toBe("allowance-first")
    expect(session.getSnapshot().usageError).toBe(false)
    stop()
  })

  it("marks a failed first allowance read, and stops its loading state", async () => {
    // With no figures to keep, the page must state the failure. A loading
    // state that never ends states a read that is still in flight.
    const { adapter, session } = setup()
    sessions.push(session)
    vi.mocked(adapter.getAllowanceUsage).mockRejectedValueOnce(new Error("Unavailable"))
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().allowanceError).toBe(true))
    expect(session.getSnapshot().allowance).toBeNull()
    expect(session.getSnapshot().allowanceLoading).toBe(false)
    stop()
  })

  it("rejects a hidden read's result", async () => {
    const pending = deferred<ProviderUsageSummaryPayload>()
    const { adapter, session, setVisible } = setup()
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().usage).not.toBeNull())
    vi.mocked(adapter.getUsage).mockReturnValueOnce(pending.promise)
    session.refresh()
    setVisible(false)
    pending.resolve(usage("hidden"))
    await Promise.resolve()
    expect(session.getSnapshot().usage?.generatedAt).toBe("first")
    setVisible(true)
    await vi.waitFor(() => expect(adapter.getUsage).toHaveBeenCalledTimes(3))
    stop()
  })

  it("serializes usage reads across an unsubscribe and resubscribe", async () => {
    const pending = deferred<ProviderUsageSummaryPayload>()
    const { adapter, session } = setup(true, {
      getUsage: vi.fn().mockReturnValueOnce(pending.promise).mockResolvedValue(usage("new")),
    })
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(adapter.getUsage).toHaveBeenCalledOnce())
    stop()
    const stopAgain = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().active).toBe(true))
    expect(adapter.getUsage).toHaveBeenCalledOnce()
    pending.resolve(usage("old"))
    await vi.waitFor(() => expect(session.getSnapshot().usage?.generatedAt).toBe("new"))
    expect(adapter.getUsage).toHaveBeenCalledTimes(2)
    stopAgain()
  })

  it("refreshes session allocations on provider updates and retains them after a failed read", async () => {
    const { adapter, session, meterChanged } = setup()
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().sessionLimitAllocations).not.toBeNull())
    const first = session.getSnapshot().sessionLimitAllocations
    vi.mocked(adapter.getSessionLimitAllocations).mockRejectedValueOnce(
      new Error("Unavailable"),
    )
    meterChanged()
    await vi.waitFor(() => expect(adapter.getSessionLimitAllocations).toHaveBeenCalledTimes(2))
    expect(session.getSnapshot().sessionLimitAllocations).toBe(first)
    stop()
  })

  it("keeps the six newest sessions and reads updates from the shared source", async () => {
    const { adapter, session, entryChanged, setEntries } = setup()
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().recentSessions).not.toBeNull())
    expect(session.getSnapshot().recentSessions?.map((item) => item.sessionId)).toEqual([
      "d",
      "c",
      "b",
      "a",
    ])
    setEntries([
      entry("g", "2026-09-14T13:00:00Z"),
      entry("f", "2026-09-14T12:00:00Z"),
      entry("e", "2026-09-14T11:00:00Z"),
      entry("d2", "2026-09-14T10:00:00Z"),
      entry("c2", "2026-09-14T09:00:00Z"),
      entry("b2", "2026-09-14T08:00:00Z"),
      entry("a2", "2026-09-14T07:00:00Z"),
    ])
    entryChanged({ title: true })
    await vi.waitFor(() =>
      expect(session.getSnapshot().recentSessions?.map((item) => item.sessionId)).toEqual([
        "g",
        "f",
        "e",
        "d2",
        "c2",
        "b2",
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
    expect(adapter.getUsage).toHaveBeenCalledTimes(3)
    stop()
  })

  it("holds event reads while the first scan pass runs, then reads once", async () => {
    const { adapter, session, scanFinished, entryChanged, setScanRunning } = setup(
      true,
      {},
      {
        scanRunning: true,
      },
    )
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    // The first read is the one the reader is waiting for; the pass running
    // does not hold it back.
    await vi.waitFor(() => expect(session.getSnapshot().usage).not.toBeNull())
    expect(adapter.getUsage).toHaveBeenCalledTimes(1)

    for (let index = 0; index < 5; index += 1) entryChanged({ analysis: true })
    scanFinished()
    await new Promise((resolve) => setTimeout(resolve, TEST_DEBOUNCE_MS * 4))
    expect(adapter.getUsage).toHaveBeenCalledTimes(1)

    setScanRunning(false)
    await vi.waitFor(() => expect(adapter.getUsage).toHaveBeenCalledTimes(2))
    await vi.waitFor(() => expect(session.getSnapshot().refreshing).toBe(false))
    expect(adapter.getUsage).toHaveBeenCalledTimes(2)
    stop()
  })

  it("does not hold for a scan that starts after an idle activation", async () => {
    const { adapter, session, entryChanged, setScanRunning } = setup(
      true,
      {},
      {
        scanRunning: false,
      },
    )
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().usage).not.toBeNull())
    expect(adapter.getUsage).toHaveBeenCalledTimes(1)

    setScanRunning(true)
    entryChanged({ analysis: true })
    await vi.waitFor(() => expect(adapter.getUsage).toHaveBeenCalledTimes(2))
    stop()
  })

  it("holds the live-usage push's reads but publishes its figures at once", async () => {
    const { adapter, session, meterChanged, setScanRunning } = setup(
      true,
      {},
      {
        scanRunning: true,
      },
    )
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() =>
      expect(session.getSnapshot().allowance?.generatedAt).toBe("allowance-first"),
    )
    expect(adapter.getAllowanceUsage).toHaveBeenCalledTimes(1)

    vi.mocked(adapter.getAllowanceUsage).mockResolvedValue(allowance("allowance-pushed"))
    meterChanged(liveUsage("live-pushed"))
    // The push is the figures, not a hint to go and read them.
    expect(session.getSnapshot().liveUsage?.generatedAt).toBe("live-pushed")
    await new Promise((resolve) => setTimeout(resolve, TEST_DEBOUNCE_MS * 4))
    expect(adapter.getAllowanceUsage).toHaveBeenCalledTimes(1)

    setScanRunning(false)
    await vi.waitFor(() =>
      expect(session.getSnapshot().allowance?.generatedAt).toBe("allowance-pushed"),
    )
    stop()
  })

  it("gives up the hold at the cap when no pass ever finishes", async () => {
    const { adapter, session, entryChanged } = setup(
      true,
      {},
      {
        scanRunning: true,
        scanHoldCapMs: TEST_DEBOUNCE_MS,
      },
    )
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().usage).not.toBeNull())

    entryChanged({ analysis: true })
    // The pass never reports its end, so only the cap can release this.
    await vi.waitFor(() => expect(adapter.getUsage).toHaveBeenCalledTimes(2))
    stop()
  })

  it("does not hold for a later pass in the same activation", async () => {
    const { adapter, session, entryChanged, setScanRunning } = setup(
      true,
      {},
      {
        scanRunning: true,
      },
    )
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().usage).not.toBeNull())

    setScanRunning(false)
    entryChanged({ analysis: true })
    await vi.waitFor(() => expect(adapter.getUsage).toHaveBeenCalledTimes(2))
    await vi.waitFor(() => expect(session.getSnapshot().refreshing).toBe(false))

    // A scoped pass from a watcher burst must not stall the ordinary updates
    // this page exists to show.
    setScanRunning(true)
    entryChanged({ analysis: true })
    await vi.waitFor(() => expect(adapter.getUsage).toHaveBeenCalledTimes(3))
    stop()
  })

  it("classifies which facets move the totals", () => {
    const row = entry("a", "2026-09-12T10:00:00Z")
    expect(overviewUpdateTouchesTotals(update(row, { title: true }))).toBe(false)
    expect(overviewUpdateTouchesTotals(update(row, {}))).toBe(false)
    for (const facet of ["metadata", "analysis", "usage", "checks", "limits"] as const) {
      expect(overviewUpdateTouchesTotals(update(row, { [facet]: true }))).toBe(true)
    }
  })

  it("reports main-window content ready once both reads settle, and only once", async () => {
    const { adapter, session, scanFinished } = setup()
    sessions.push(session)
    const usagePending = deferred<ProviderUsageSummaryPayload>()
    const allowancePending = deferred<AllowanceUsageSummaryPayload>()
    vi.mocked(adapter.getUsage).mockReturnValueOnce(usagePending.promise)
    vi.mocked(adapter.getAllowanceUsage).mockReturnValueOnce(allowancePending.promise)
    const stop = session.subscribe(() => undefined)

    await vi.waitFor(() => expect(adapter.getUsage).toHaveBeenCalledOnce())
    expect(mainWindowContentReady).not.toHaveBeenCalled()

    usagePending.resolve(usage("first"))
    await vi.waitFor(() => expect(session.getSnapshot().usage?.generatedAt).toBe("first"))
    // Usage settled alone must not report: the allowance read is still open.
    expect(mainWindowContentReady).not.toHaveBeenCalled()

    allowancePending.resolve(allowance("allowance-first"))
    await vi.waitFor(() => expect(mainWindowContentReady).toHaveBeenCalledOnce())
    expect(mainWindowContentReady).toHaveBeenCalledWith(7)

    // A later refresh re-reads both, but must not report a second time.
    scanFinished()
    await vi.waitFor(() => expect(adapter.getUsage).toHaveBeenCalledTimes(2))
    await vi.waitFor(() => expect(adapter.getAllowanceUsage).toHaveBeenCalledTimes(2))
    expect(mainWindowContentReady).toHaveBeenCalledOnce()
    stop()
  })

  it("still reports once the allowance settles when usage fails first", async () => {
    const { adapter, session } = setup()
    sessions.push(session)
    const allowancePending = deferred<AllowanceUsageSummaryPayload>()
    vi.mocked(adapter.getUsage).mockRejectedValueOnce(new Error("Unavailable"))
    vi.mocked(adapter.getAllowanceUsage).mockReturnValueOnce(allowancePending.promise)
    const stop = session.subscribe(() => undefined)

    await vi.waitFor(() => expect(session.getSnapshot().usageError).toBe(true))
    // Usage settled with an error; the allowance read has not, so nothing
    // reports yet.
    expect(mainWindowContentReady).not.toHaveBeenCalled()

    allowancePending.resolve(allowance("allowance-first"))
    await vi.waitFor(() => expect(mainWindowContentReady).toHaveBeenCalledOnce())
    stop()
  })

  it("does not settle content ready for a usage read superseded before it resolves", async () => {
    const { adapter, session } = setup()
    sessions.push(session)
    const firstUsage = deferred<ProviderUsageSummaryPayload>()
    const secondUsage = deferred<ProviderUsageSummaryPayload>()
    vi.mocked(adapter.getUsage)
      .mockReturnValueOnce(firstUsage.promise)
      .mockReturnValueOnce(secondUsage.promise)
    const stop = session.subscribe(() => undefined)

    // Allowance settles on the first pass, so only the usage read is left
    // gating content ready.
    await vi.waitFor(() => expect(session.getSnapshot().allowance).not.toBeNull())
    await vi.waitFor(() => expect(adapter.getUsage).toHaveBeenCalledOnce())

    // A second refresh starts, and only then does the first read resolve:
    // it is superseded before it settles.
    session.refresh()
    firstUsage.resolve(usage("stale"))
    await vi.waitFor(() => expect(adapter.getUsage).toHaveBeenCalledTimes(2))
    expect(mainWindowContentReady).not.toHaveBeenCalled()
    expect(session.getSnapshot().usage).toBeNull()

    // The current read settling reports, carrying its own value.
    secondUsage.resolve(usage("second"))
    await vi.waitFor(() => expect(mainWindowContentReady).toHaveBeenCalledOnce())
    expect(mainWindowContentReady).toHaveBeenCalledWith(7)
    expect(session.getSnapshot().usage?.generatedAt).toBe("second")
    stop()
  })

  it("remembers a plan once an allowance read lands", async () => {
    const rememberPlan = vi.fn()
    const { adapter, session } = setup(true, {}, { rememberPlan })
    sessions.push(session)
    vi.mocked(adapter.getAllowanceUsage).mockResolvedValue(
      allowance("allowance-plan", [accountWithPlan()]),
    )
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() =>
      expect(session.getSnapshot().allowance?.generatedAt).toBe("allowance-plan"),
    )
    expect(rememberPlan).toHaveBeenCalledExactlyOnceWith(true)
    stop()
  })

  it("remembers a plan from a live-usage push", async () => {
    const rememberPlan = vi.fn()
    const { session, meterChanged } = setup(true, {}, { rememberPlan })
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() =>
      expect(session.getSnapshot().allowance?.generatedAt).toBe("allowance-first"),
    )
    // The first, plan-free reads already settled and remembered `false`.
    rememberPlan.mockClear()

    meterChanged(liveUsage("live-pushed", [liveProviderWithPlan()]))
    expect(rememberPlan).toHaveBeenCalledExactlyOnceWith(true)
    stop()
  })

  it("does not call rememberPlan again when a settled read repeats the same answer", async () => {
    const rememberPlan = vi.fn()
    const { adapter, session, scanFinished } = setup(true, {}, { rememberPlan })
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() =>
      expect(session.getSnapshot().allowance?.generatedAt).toBe("allowance-first"),
    )
    expect(rememberPlan).toHaveBeenCalledExactlyOnceWith(false)

    // A later read with no plan repeats the same answer already remembered.
    scanFinished()
    await vi.waitFor(() => expect(adapter.getAllowanceUsage).toHaveBeenCalledTimes(2))
    expect(rememberPlan).toHaveBeenCalledOnce()
    stop()
  })

  it("holds a negative answer while live usage is still pending, then remembers a plan it finds", async () => {
    const rememberPlan = vi.fn()
    const livePending = deferred<LiveUsageSummaryPayload>()
    const { session } = setup(
      true,
      { getLiveUsage: vi.fn().mockReturnValueOnce(livePending.promise) },
      { rememberPlan },
    )
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() =>
      expect(session.getSnapshot().allowance?.generatedAt).toBe("allowance-first"),
    )
    // The allowance read landed with no plan, but live usage has not
    // answered yet, so a negative answer is not certain.
    expect(rememberPlan).not.toHaveBeenCalled()

    livePending.resolve(liveUsage("live-plan", [liveProviderWithPlan()]))
    await vi.waitFor(() => expect(session.getSnapshot().liveUsageSettled).toBe(true))
    expect(rememberPlan).toHaveBeenCalledExactlyOnceWith(true)
    stop()
  })

  it("remembers a negative answer only once both reads land with no plan", async () => {
    const rememberPlan = vi.fn()
    const livePending = deferred<LiveUsageSummaryPayload>()
    const { session } = setup(
      true,
      { getLiveUsage: vi.fn().mockReturnValueOnce(livePending.promise) },
      { rememberPlan },
    )
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() =>
      expect(session.getSnapshot().allowance?.generatedAt).toBe("allowance-first"),
    )
    expect(rememberPlan).not.toHaveBeenCalled()

    livePending.resolve(liveUsage("live-first"))
    await vi.waitFor(() => expect(session.getSnapshot().liveUsageSettled).toBe(true))
    expect(rememberPlan).toHaveBeenCalledExactlyOnceWith(false)
    stop()
  })

  it("remembers a negative answer once a failed allowance read and a plan-free live-usage read have both settled", async () => {
    const rememberPlan = vi.fn()
    const { adapter, session } = setup(true, {}, { rememberPlan })
    sessions.push(session)
    vi.mocked(adapter.getAllowanceUsage).mockRejectedValueOnce(new Error("Unavailable"))
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() => expect(session.getSnapshot().allowanceError).toBe(true))
    await vi.waitFor(() => expect(session.getSnapshot().liveUsageSettled).toBe(true))
    expect(rememberPlan).toHaveBeenCalledExactlyOnceWith(false)
    stop()
  })

  it("starts from what a previous run remembered, in the production default", async () => {
    // The default adapter's first allowance read has no plan, so a memory
    // of `false` already agrees with it.
    writeOverviewViewPrefs({ hadSubscriptionPlan: false })
    const setItemSpy = vi.spyOn(Storage.prototype, "setItem")
    const { adapter, session } = setup()
    sessions.push(session)
    const stop = session.subscribe(() => undefined)
    await vi.waitFor(() =>
      expect(session.getSnapshot().allowance?.generatedAt).toBe("allowance-first"),
    )
    // The first read repeats the remembered answer, so it must not write
    // again: the constructor took its memory from the same prefs.
    expect(setItemSpy).not.toHaveBeenCalled()

    // A read that disagrees corrects the memory.
    vi.mocked(adapter.getAllowanceUsage).mockResolvedValueOnce(
      allowance("allowance-changed", [accountWithPlan()]),
    )
    session.refreshAllowance()
    await vi.waitFor(() =>
      expect(session.getSnapshot().allowance?.generatedAt).toBe("allowance-changed"),
    )
    expect(readOverviewViewPrefs().hadSubscriptionPlan).toBe(true)
    stop()
    setItemSpy.mockRestore()
  })
})
