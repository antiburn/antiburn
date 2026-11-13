/**
 * Integration tests for the Overview progress store's real IPC boundary.
 *
 * These drive `subscribeOverviewProgress`/`overviewProgress` against mocked
 * `@tauri-apps/api/core` and `@tauri-apps/api/event` — the lowest edge —
 * rather than mocking `lib/ipc` or `lib/scanStatusStore` directly, so the
 * real `onScanEvent`, `getScanStatus`, and `scanStatusStore` wiring all run.
 * A `FakeScanController` mirrors the Rust `ScanController`: one mutable
 * status that both `get_scan_status` reads directly and every `scan:*`
 * event pushes, so a test can reproduce the real emission order.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { DEFAULT_SETTINGS } from "../../../lib/ipc"
import type { ScanPhase, ScanStatus } from "../../../lib/ipcPayloads"
import { installedCheckFixture } from "../burn-checks/tests/installedCheckFixture"

type Handler = (event: { payload: unknown }) => void

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn(),
  handlers: new Map<string, Set<Handler>>(),
}))

vi.mock("@tauri-apps/api/core", () => ({
  invoke: mocks.invoke,
  isTauri: () => true,
}))

vi.mock("@tauri-apps/api/event", () => ({
  listen: mocks.listen,
}))

function emit(event: string, payload: unknown): void {
  for (const handler of mocks.handlers.get(event) ?? []) handler({ payload })
}

function waitForListener(event: string): Promise<void> {
  return vi.waitFor(() => expect(mocks.handlers.has(event)).toBe(true))
}

/**
 * Mirrors `ScanController` in `src-tauri/src/scan/mod.rs`: one mutable
 * status shared by every pass. `get_scan_status` clones it and fills
 * `agents` from persisted state; a push event carries the same status with
 * `agents` always empty, matching the real backend (see `withKnownAgents`).
 */
class FakeScanController {
  private current: ScanStatus = {
    running: false,
    completedAgents: 0,
    totalAgents: 0,
    sessions: 0,
    finishedAt: null,
    cancelled: false,
    error: null,
    agents: [],
    listChanged: false,
    reDescribed: 0,
    phase: "idle",
    foundByAgent: [],
    read: { completed: 0, total: 0 },
    gate: null,
  }
  /** The persisted `scan_state` table, filled only on a direct `get_scan_status` read. */
  persistedAgents: ScanStatus["agents"] = []

  private setPhase(phase: ScanPhase): void {
    this.current = { ...this.current, phase }
  }

  /** What `get_scan_status` returns right now. */
  statusCommand(): ScanStatus {
    return { ...this.current, agents: this.persistedAgents }
  }

  start(): void {
    this.current = {
      ...this.current,
      running: true,
      completedAgents: 0,
      totalAgents: 1,
      sessions: 0,
      listChanged: false,
      reDescribed: 0,
      cancelled: false,
      error: null,
      phase: "finding",
      foundByAgent: [],
      read: { completed: 0, total: 0 },
      gate: null,
    }
    emit("scan:started", { ...this.current, agents: [] })
  }

  found(agent: string, sessions: number): void {
    this.current = {
      ...this.current,
      foundByAgent: [...this.current.foundByAgent, { agent, sessions, done: true }],
    }
    emit("scan:progress", { ...this.current, agents: [] })
  }

  toReading(total: number): void {
    this.setPhase("reading")
    this.current = { ...this.current, read: { completed: 0, total } }
    emit("scan:progress", { ...this.current, agents: [] })
  }

  readProgress(completed: number, total: number): void {
    this.current = { ...this.current, read: { completed, total } }
    emit("scan:progress", { ...this.current, agents: [] })
  }

  toSaving(gate: NonNullable<ScanStatus["gate"]>): void {
    this.setPhase("saving")
    this.current = { ...this.current, gate }
    emit("scan:progress", { ...this.current, agents: [] })
  }

  finish(agent: string, sessionsSeen: number): void {
    this.current = { ...this.current, running: false, finishedAt: "2026-10-01T22:32:13Z" }
    this.persistedAgents = [
      ...this.persistedAgents.filter((existing) => existing.agent !== agent),
      { agent, lastCompletedAt: this.current.finishedAt, sessionsSeen },
    ]
    emit("scan:finished", { ...this.current, agents: [] })
  }
}

const DEFAULT_TEST_SETTINGS = { ...DEFAULT_SETTINGS }

const SETTLED_REPORT = {
  evidenceSettled: true,
  windowSessions: 142,
  pendingEvidence: 0,
  deferredEvidence: 0,
  estimatedTokenBurnBasisPoints: null,
  categories: [],
}

let controller: FakeScanController

beforeEach(() => {
  vi.resetModules()
  vi.clearAllMocks()
  mocks.handlers.clear()
  controller = new FakeScanController()
  mocks.listen.mockImplementation(async (name: string, handler: Handler) => {
    let set = mocks.handlers.get(name)
    if (!set) {
      set = new Set()
      mocks.handlers.set(name, set)
    }
    set.add(handler)
    return () => set?.delete(handler)
  })
  mocks.invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
    switch (command) {
      case "get_main_window_visible":
        return true
      case "get_scan_status":
        return controller.statusCommand()
      case "get_checks_report":
        return SETTLED_REPORT
      case "cancel_checks_report":
        return undefined
      case "get_settings":
        return DEFAULT_TEST_SETTINGS
      case "set_settings":
        return { ...DEFAULT_TEST_SETTINGS, ...(args?.settings as object) }
      case "get_folder_permissions":
        return { deferred: [], granted: [], supported: true }
      case "finish_first_run":
        return { ...DEFAULT_TEST_SETTINGS }
      case "advance_first_run":
        return undefined
      case "start_live_usage":
        return { ...DEFAULT_TEST_SETTINGS, liveUsageStarted: true }
      case "refresh_live_usage":
        return null
      case "note_interaction":
        return undefined
      default:
        throw new Error(`Unexpected command: ${command}`)
    }
  })
})

const stops: Array<() => void> = []
afterEach(() => {
  for (const stop of stops.splice(0)) stop()
})

/** Run one full pass through the fake controller, same sequence as `pass()`
 *  in `src-tauri/src/scan/mod.rs`: finding, reading, saving, finished. */
function runLaunchPass(): void {
  controller.start()
  controller.found("claude-code", 49)
  controller.toReading(49)
  controller.readProgress(49, 49)
  controller.toSaving({ kept: 49, outsideRepository: 0, excluded: 0, unreadable: 0 })
  controller.finish("claude-code", 49)
}

describe("overviewProgressStore's live IPC boundary", () => {
  it("(a) latches steps 1 and 2 from the pass's own events when it subscribed first", async () => {
    const { subscribeOverviewProgress, overviewProgress } =
      await import("./overviewProgressStore")
    const stop = subscribeOverviewProgress(() => undefined)
    stops.push(stop)

    // The store has attached its scan-event listeners before any pass runs —
    // the ordinary launch shape: the main window's JS loads well before the
    // launch pass starts.
    await waitForListener("scan:started")
    await waitForListener("scan:progress")
    await waitForListener("scan:finished")

    runLaunchPass()

    await vi.waitFor(() => expect(overviewProgress().agents.done).toBe(true))
    await vi.waitFor(() =>
      expect(overviewProgress().agents.rows).toEqual([
        { agent: "claude-code", label: "Claude Code", sessions: 49, done: true },
      ]),
    )
    await vi.waitFor(() => expect(overviewProgress().sessions.done).toBe(true))
    expect(overviewProgress().sessions.completed).toBe(49)
    expect(overviewProgress().sessions.total).toBe(49)
  })

  it("(b) latches steps 1 and 2 from a direct read when it subscribed after the pass finished", async () => {
    // The pass already ran and finished before anything in the frontend
    // subscribed — e.g. the Overview mounted 16 seconds late. There are no
    // events left to catch; only `get_scan_status`'s direct read can recover
    // the outcome, and it returns the last phase the pass reached: "saving".
    runLaunchPass()
    expect(controller.statusCommand().phase).toBe("saving")

    const { subscribeOverviewProgress, overviewProgress } =
      await import("./overviewProgressStore")
    const stop = subscribeOverviewProgress(() => undefined)
    stops.push(stop)

    await vi.waitFor(() => expect(overviewProgress().agents.done).toBe(true))
    await vi.waitFor(() =>
      expect(overviewProgress().agents.rows).toEqual([
        { agent: "claude-code", label: "Claude Code", sessions: 49, done: true },
      ]),
    )
    await vi.waitFor(() => expect(overviewProgress().sessions.done).toBe(true))
    expect(overviewProgress().sessions.completed).toBe(49)
  })
})

describe("overviewProgressStore vs. a scoped pass overlapping the subscribe point", () => {
  it("waits for the current pass when an earlier pass predates subscription", async () => {
    // The full launch pass completed (49 sessions, phase saving, gate set).
    runLaunchPass()
    expect(controller.statusCommand().phase).toBe("saving")

    // Before the Overview ever looked, a second pass started — e.g. a
    // watcher-triggered scoped rediscovery of one actively-writing agent
    // (very plausible while a live coding session, such as this one, keeps
    // appending to its own transcript). It has not reached "reading" yet.
    controller.start()
    expect(controller.statusCommand().phase).toBe("finding")

    const { subscribeOverviewProgress, overviewProgress } =
      await import("./overviewProgressStore")
    const stop = subscribeOverviewProgress(() => undefined)
    stops.push(stop)

    await vi.waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith("get_scan_status"))
    expect(overviewProgress().sessions.done).toBe(false)
    runLaunchPass()
    await vi.waitFor(() => expect(overviewProgress().sessions.done).toBe(true))
    expect(overviewProgress().sessions.completed).toBe(49)
  })
})

describe("overviewProgressStore vs. a denied get_scan_status command", () => {
  it("(d) catches up agents/read from live events even when the direct read is denied", async () => {
    // Live events can recover from a failed initial status read.
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === "get_main_window_visible") return true
      if (command === "get_scan_status") throw new Error("main-window: command not allowed")
      if (command === "get_checks_report") return SETTLED_REPORT
      if (command === "get_settings") return DEFAULT_TEST_SETTINGS
      if (command === "get_folder_permissions")
        return { deferred: [], granted: [], supported: true }
      throw new Error(`Unexpected command: ${command}`)
    })
    const { subscribeOverviewProgress, overviewProgress } =
      await import("./overviewProgressStore")
    const stop = subscribeOverviewProgress(() => undefined)
    stops.push(stop)
    await waitForListener("scan:finished")

    runLaunchPass()

    await vi.waitFor(() => expect(overviewProgress().agents.done).toBe(true))
    expect(overviewProgress().sessions.done).toBe(true)
  })
})

describe("overviewProgressStore across an unsubscribe/resubscribe cycle", () => {
  it("(f) recovers through scanStatusStore's own live snapshot after a stop/restart", async () => {
    // The ref-counted store tears down when the last listener leaves and
    // restarts fresh on the next one. `scanStatusStore` has no
    // `resetOnStop`, so its snapshot survives the gap; this proves a
    // resubscribe's catch-up (`onScanStatus(scanStatusStore.getSnapshot())`)
    // still sees the finished pass even though nothing was subscribed while
    // it ran.
    const { subscribeOverviewProgress, overviewProgress } =
      await import("./overviewProgressStore")
    const first = subscribeOverviewProgress(() => undefined)
    await waitForListener("scan:finished")
    first() // last listener leaves: the store's own stop() and scanStatusStore's own stop() run.

    runLaunchPass() // Runs entirely while unsubscribed.

    const second = subscribeOverviewProgress(() => undefined)
    stops.push(second)
    await vi.waitFor(() => expect(overviewProgress().agents.done).toBe(true))
    expect(overviewProgress().sessions.done).toBe(true)
  })

  it("(g) a StrictMode-style double subscribe/unsubscribe leaves one working connection", async () => {
    // Two consumers (`OverviewView` and `OverviewFixes`) both call
    // `subscribeOverviewProgress`, and React's dev-mode double-invoke can
    // subscribe, unsubscribe, and resubscribe a listener before any of
    // `start()`'s internal awaits settle. The surviving generation must
    // still end up attached to live events.
    const { subscribeOverviewProgress, overviewProgress } =
      await import("./overviewProgressStore")
    const strictModeStop = subscribeOverviewProgress(() => undefined)
    strictModeStop()
    const overviewViewStop = subscribeOverviewProgress(() => undefined)
    const overviewFixesStop = subscribeOverviewProgress(() => undefined)
    stops.push(overviewViewStop, overviewFixesStop)

    await waitForListener("scan:finished")
    runLaunchPass()

    await vi.waitFor(() => expect(overviewProgress().agents.done).toBe(true))
    expect(overviewProgress().sessions.done).toBe(true)
  })
})

describe("overviewProgressStore's steady mode", () => {
  it("does not change the Agents rows in the snapshot when a new full pass starts", async () => {
    // The device has scanned before: a full pass already ran and finished
    // before this test's store ever subscribed, same shape as case (b).
    runLaunchPass()

    const { subscribeOverviewProgress, overviewProgress } =
      await import("./overviewProgressStore")
    stops.push(subscribeOverviewProgress(() => undefined))

    await vi.waitFor(() =>
      expect(overviewProgress().agents.rows).toEqual([
        { agent: "claude-code", label: "Claude Code", sessions: 49, done: true },
      ]),
    )

    await waitForListener("scan:started")
    // A routine 5-minute tick (or another launch) starts a fresh pass:
    // discovery resets to empty, same as every full pass's start.
    controller.start()

    await vi.waitFor(() =>
      expect(overviewProgress().agents.rows).toEqual([
        { agent: "claude-code", label: "Claude Code", sessions: 49, done: true },
      ]),
    )
  })
})

describe("overview progress recovery", () => {
  it("drops removed installed findings from the sidebar count on the report event", async () => {
    let report = { ...SETTLED_REPORT, installedChecks: [installedCheckFixture] }
    const invoke = mocks.invoke.getMockImplementation()!
    mocks.invoke.mockImplementation(async (command, args) => {
      if (command === "get_checks_report") return report
      return invoke(command, args)
    })
    const store = await import("./overviewProgressStore")
    const { sidebarProgress } = await import("./sidebarProgress")
    stops.push(store.subscribeOverviewProgress(() => undefined))
    await waitForListener("checks:report-changed")
    await vi.waitFor(() => expect(store.overviewProgress().failingCount).toBe(1))
    expect(sidebarProgress("burnChecks", store.overviewProgress(), null).status).toBe(
      "1 to fix",
    )

    report = { ...report, installedChecks: [] }
    emit("checks:report-changed", null)

    await vi.waitFor(() => expect(store.overviewProgress().failingCount).toBe(0))
    expect(sidebarProgress("burnChecks", store.overviewProgress(), null).status).toBe(
      "0 to fix",
    )
    expect(store.overviewProgress().categories).not.toEqual(
      expect.arrayContaining([
        expect.objectContaining({ id: installedCheckFixture.effectiveId }),
      ]),
    )
  })

  it("refreshes checks when a finished scan snapshot arrives after the first report", async () => {
    runLaunchPass()
    let resolveScan!: (status: ScanStatus) => void
    const scan = new Promise<ScanStatus>((resolve) => {
      resolveScan = resolve
    })
    const invoke = mocks.invoke.getMockImplementation()!
    mocks.invoke.mockImplementation(async (command, args) => {
      if (command === "get_scan_status") return scan
      if (command === "get_settings") return { ...DEFAULT_TEST_SETTINGS }
      return invoke(command, args)
    })
    const store = await import("./overviewProgressStore")
    stops.push(store.subscribeOverviewProgress(() => undefined))
    await vi.waitFor(() => expect(store.overviewProgress().checks.windowSessions).toBe(142))
    expect(store.overviewProgress().checks.done).toBe(false)
    resolveScan(controller.statusCommand())
    await vi.waitFor(() => expect(store.overviewProgress().checks.done).toBe(true))
    expect(
      mocks.invoke.mock.calls.filter(([command]) => command === "get_checks_report"),
    ).toHaveLength(2)
  })

  it("releases hidden report work and ignores its late response after reveal", async () => {
    let resolveReport!: (report: typeof SETTLED_REPORT) => void
    const pending = new Promise<typeof SETTLED_REPORT>((resolve) => {
      resolveReport = resolve
    })
    const invoke = mocks.invoke.getMockImplementation()!
    let reportCalls = 0
    mocks.invoke.mockImplementation(async (command, args) => {
      if (command === "get_checks_report" && ++reportCalls === 1) return pending
      return invoke(command, args)
    })
    const store = await import("./overviewProgressStore")
    stops.push(store.subscribeOverviewProgress(() => undefined))
    await vi.waitFor(() => expect(reportCalls).toBe(1))
    const consumerId = mocks.invoke.mock.calls.find(
      ([command]) => command === "get_checks_report",
    )![1].consumerId
    emit("main:visibility-changed", false)
    expect(mocks.invoke).toHaveBeenCalledWith("cancel_checks_report", { consumerId })
    const permissions = mocks.invoke.mock.calls.filter(
      ([command]) => command === "get_folder_permissions",
    ).length
    emit("checks:report-changed", null)
    runLaunchPass()
    expect(reportCalls).toBe(1)
    expect(
      mocks.invoke.mock.calls.filter(([command]) => command === "get_folder_permissions"),
    ).toHaveLength(permissions)
    emit("main:visibility-changed", true)
    await vi.waitFor(() => expect(store.overviewProgress().checks.windowSessions).toBe(142))
    resolveReport({ ...SETTLED_REPORT, windowSessions: 999 })
    await pending
    await Promise.resolve()
    expect(store.overviewProgress().checks.windowSessions).toBe(142)
  })
})
