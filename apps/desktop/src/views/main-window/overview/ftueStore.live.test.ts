/**
 * Integration tests for the Overview first-run store's real IPC boundary.
 *
 * These drive `subscribeFtue`/`ftueSnapshot` against mocked
 * `@tauri-apps/api/core` and `@tauri-apps/api/event` — the lowest edge —
 * rather than mocking `lib/ipc` or `lib/scanStatusStore` directly, so the
 * real `onScanEvent`, `getScanStatus`, and `scanStatusStore` wiring all run.
 * A `FakeScanController` mirrors the Rust `ScanController`: one mutable
 * status that both `get_scan_status` reads directly and every `scan:*`
 * event pushes, so a test can reproduce the real emission order.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { AppSettings, ScanPhase, ScanStatus } from "../../../lib/ipcPayloads"

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
      foundByAgent: [...this.current.foundByAgent, { agent, sessions }],
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

const DEFAULT_TEST_SETTINGS: AppSettings = {
  theme: "system",
  interfaceScalePercent: 100,
  activityWindowDays: 7,
  sessionDataRetentionDays: -1,
  onboardingCompleted: true,
  launchAtLogin: true,
  trayIconVisible: true,
  dockIconVisible: true,
  autoUpdate: true,
  discoveryPaused: false,
  includeNonRepoFolders: false,
  notificationsEnabled: true,
  notifyUpdateAvailable: true,
  notifyScanFailure: true,
  nudgePlacement: "topRight",
  nudgeAutoDismissSecs: 8,
  notificationSound: true,
  nudgesRespectDnd: false,
  diskSpaceDisplay: "whenLow",
  diskSpaceThresholdGb: 10,
  notifyDiskSpaceLow: true,
  milestones5h: [],
  milestonesWeekly: [],
  liveUsageEnabled: true,
  liveUsageHiddenProviders: [],
  disabledAgents: [],
  analyticsEnabled: true,
  overviewLimitsExpanded: true,
  skillsMcpExpanded: false,
  sessionBadgeMetric: "cost",
  sessionFilter: "",
  workingWeek: "seven",
}

const SETTLED_REPORT = {
  evidenceSettled: true,
  windowSessions: 142,
  pendingEvidence: 0,
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

describe("ftueStore's live IPC boundary", () => {
  it("(a) latches steps 1 and 2 from the pass's own events when it subscribed first", async () => {
    const { subscribeFtue, ftueSnapshot } = await import("./ftueStore")
    const stop = subscribeFtue(() => undefined)
    stops.push(stop)

    // The store has attached its scan-event listeners before any pass runs —
    // the ordinary launch shape: the main window's JS loads well before the
    // launch pass starts.
    await waitForListener("scan:started")
    await waitForListener("scan:progress")
    await waitForListener("scan:finished")

    runLaunchPass()

    await vi.waitFor(() => expect(ftueSnapshot().find.done).toBe(true))
    expect(ftueSnapshot().find.rows).toEqual([{ agent: "Claude Code", sessions: 49 }])
    await vi.waitFor(() => expect(ftueSnapshot().read.done).toBe(true))
    expect(ftueSnapshot().read.completed).toBe(49)
    expect(ftueSnapshot().read.total).toBe(49)
    expect(ftueSnapshot().read.gate).toEqual({
      kept: 49,
      outsideRepository: 0,
      excluded: 0,
      unreadable: 0,
    })
  })

  it("(b) latches steps 1 and 2 from a direct read when it subscribed after the pass finished", async () => {
    // The pass already ran and finished before anything in the frontend
    // subscribed — e.g. the Overview mounted 16 seconds late. There are no
    // events left to catch; only `get_scan_status`'s direct read can recover
    // the outcome, and it returns the last phase the pass reached: "saving".
    runLaunchPass()
    expect(controller.statusCommand().phase).toBe("saving")

    const { subscribeFtue, ftueSnapshot } = await import("./ftueStore")
    const stop = subscribeFtue(() => undefined)
    stops.push(stop)

    await vi.waitFor(() => expect(ftueSnapshot().find.done).toBe(true))
    expect(ftueSnapshot().find.rows).toEqual([{ agent: "Claude Code", sessions: 49 }])
    await vi.waitFor(() => expect(ftueSnapshot().read.done).toBe(true))
    expect(ftueSnapshot().read.completed).toBe(49)
    expect(ftueSnapshot().read.gate).toEqual({
      kept: 49,
      outsideRepository: 0,
      excluded: 0,
      unreadable: 0,
    })
  })
})

describe("ftueStore vs. a scoped pass overlapping the subscribe point", () => {
  it("(c) loses the finished full pass's outcome when a later pass has already reset it", async () => {
    // The full launch pass completed (49 sessions, phase saving, gate set).
    runLaunchPass()
    expect(controller.statusCommand().phase).toBe("saving")

    // Before the Overview ever looked, a second pass started — e.g. a
    // watcher-triggered scoped rediscovery of one actively-writing agent
    // (very plausible while a live coding session, such as this one, keeps
    // appending to its own transcript). It has not reached "reading" yet.
    controller.start()
    expect(controller.statusCommand().phase).toBe("finding")

    const { subscribeFtue, ftueSnapshot } = await import("./ftueStore")
    const stop = subscribeFtue(() => undefined)
    stops.push(stop)

    await vi.waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith("get_scan_status"))
    // The 49 sessions the first pass actually found and read are gone from
    // view: the steps read as not started, even though they are done.
    expect(ftueSnapshot().find.done).toBe(false)
    expect(ftueSnapshot().read.done).toBe(false)
  })
})
