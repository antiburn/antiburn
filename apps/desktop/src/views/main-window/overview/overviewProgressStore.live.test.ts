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
  liveUsageStarted: true,
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
        return { ...DEFAULT_TEST_SETTINGS, onboardingCompleted: true }
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

    await vi.waitFor(() => expect(overviewProgress().find.done).toBe(true))
    expect(overviewProgress().find.rows).toEqual([
      { agent: "claude-code", label: "Claude Code", sessions: 49, done: true },
    ])
    await vi.waitFor(() => expect(overviewProgress().read.done).toBe(true))
    expect(overviewProgress().read.completed).toBe(49)
    expect(overviewProgress().read.total).toBe(49)
    expect(overviewProgress().read.gate).toEqual({
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

    const { subscribeOverviewProgress, overviewProgress } =
      await import("./overviewProgressStore")
    const stop = subscribeOverviewProgress(() => undefined)
    stops.push(stop)

    await vi.waitFor(() => expect(overviewProgress().find.done).toBe(true))
    expect(overviewProgress().find.rows).toEqual([
      { agent: "claude-code", label: "Claude Code", sessions: 49, done: true },
    ])
    await vi.waitFor(() => expect(overviewProgress().read.done).toBe(true))
    expect(overviewProgress().read.completed).toBe(49)
    expect(overviewProgress().read.gate).toEqual({
      kept: 49,
      outsideRepository: 0,
      excluded: 0,
      unreadable: 0,
    })
  })
})

describe("overviewProgressStore vs. a scoped pass overlapping the subscribe point", () => {
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

    const { subscribeOverviewProgress, overviewProgress } =
      await import("./overviewProgressStore")
    const stop = subscribeOverviewProgress(() => undefined)
    stops.push(stop)

    await vi.waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith("get_scan_status"))
    // The 49 sessions the first pass actually found and read are gone from
    // view: the steps read as not started, even though they are done.
    expect(overviewProgress().find.done).toBe(false)
    expect(overviewProgress().read.done).toBe(false)
  })
})

describe("overviewProgressStore vs. a denied get_scan_status command", () => {
  it("(d) catches up find/read from live events even when the direct read is denied", async () => {
    // Simulates the main window's actual capability gap: `get_scan_status`
    // has no `allow-get-scan-status` grant for the "main" window, so every
    // direct read rejects. The live `scan:*` events still arrive (listening
    // is a separate, unscoped grant), so a reader subscribed through the
    // whole pass still sees it finish.
    mocks.invoke.mockImplementation(async (command: string) => {
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

    await vi.waitFor(() => expect(overviewProgress().find.done).toBe(true))
    expect(overviewProgress().read.done).toBe(true)
  })

  it("(e) still resolves the first-run decision when the only recovery read is denied", async () => {
    // The pass already ran and finished before the Overview subscribed (no
    // events left to catch), and the usual recovery path — a direct
    // `get_scan_status` read — is denied. This used to be the reported bug:
    // the old "ever scanned before" signal came from that same denied read,
    // so the steps block's decision never resolved. `onboardingCompleted`
    // comes from `get_settings` instead, a call this scenario does not deny,
    // so the decision — and the steps the find/read steps never latch to —
    // resolve regardless.
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === "get_scan_status") throw new Error("main-window: command not allowed")
      if (command === "get_checks_report") return SETTLED_REPORT
      if (command === "get_settings") return DEFAULT_TEST_SETTINGS
      if (command === "get_folder_permissions")
        return { deferred: [], granted: [], supported: true }
      throw new Error(`Unexpected command: ${command}`)
    })
    runLaunchPass()

    const { subscribeOverviewProgress, overviewProgress } =
      await import("./overviewProgressStore")
    const stop = subscribeOverviewProgress(() => undefined)
    stops.push(stop)

    await vi.waitFor(() => expect(overviewProgress().mode).toBe("steady"))
    // Neither step 1 nor step 2 ever latches without the denied read to
    // source their rows from — the steady row falls back to whatever
    // `scanStatusStore`'s own snapshot holds, which is also empty here.
    expect(overviewProgress().find.done).toBe(false)
    expect(overviewProgress().read.done).toBe(false)
    // Step 3 does not wait on either of them outside the steps block: it
    // finishes from the settled checks report alone.
    await vi.waitFor(() => expect(overviewProgress().check.done).toBe(true))
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
    await vi.waitFor(() => expect(overviewProgress().find.done).toBe(true))
    expect(overviewProgress().read.done).toBe(true)
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

    await vi.waitFor(() => expect(overviewProgress().find.done).toBe(true))
    expect(overviewProgress().read.done).toBe(true)
  })
})

describe("overviewProgressStore's dock sequence", () => {
  it("moves the finished steps down to the row one at a time, then stops", async () => {
    const { subscribeOverviewProgress, overviewProgress, openSteps, shrinkSteps } =
      await import("./overviewProgressStore")
    stops.push(subscribeOverviewProgress(() => undefined))
    await waitForListener("ftue:reset")
    await waitForListener("scan:finished")

    emit("ftue:reset", null)
    runLaunchPass()

    await vi.waitFor(() => expect(overviewProgress().stepsDone).toBe(true))
    expect(overviewProgress().dock.stepsDocked).toBe(0)
    await vi.waitFor(() => expect(overviewProgress().dock.stepsDocked).toBe(1), {
      timeout: 2000,
    })
    await vi.waitFor(() => expect(overviewProgress().dock.stepsDocked).toBe(2), {
      timeout: 2000,
    })
    await vi.waitFor(() => expect(overviewProgress().dock.stepsDocked).toBe(3), {
      timeout: 2000,
    })

    openSteps()
    expect(overviewProgress().dock.stepsOpen).toBe(true)
    shrinkSteps()
    expect(overviewProgress().dock).toEqual({
      stepsDocked: 3,
      stepsOpen: false,
      fixesDocked: false,
    })
  })
})

describe("overviewProgressStore's first-run analytics", () => {
  function noteInteractionCalls(): { interaction: Record<string, unknown> }[] {
    return mocks.invoke.mock.calls
      .filter(([command]) => command === "note_interaction")
      .map(([, args]) => args as { interaction: Record<string, unknown> })
  }

  it("reports each funnel step once, then the result and the finish", async () => {
    mocks.invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      switch (command) {
        case "get_scan_status":
          return controller.statusCommand()
        case "get_checks_report":
          return SETTLED_REPORT
        case "cancel_checks_report":
          return undefined
        case "get_settings":
          return { ...DEFAULT_TEST_SETTINGS, onboardingCompleted: false }
        case "set_settings":
          return { ...DEFAULT_TEST_SETTINGS, ...(args?.settings as object) }
        case "get_folder_permissions":
          return { deferred: [], granted: [], supported: true }
        case "finish_first_run":
          return { ...DEFAULT_TEST_SETTINGS, onboardingCompleted: true }
        case "note_interaction":
          return undefined
        default:
          throw new Error(`Unexpected command: ${command}`)
      }
    })

    const { subscribeOverviewProgress, overviewProgress } =
      await import("./overviewProgressStore")
    stops.push(subscribeOverviewProgress(() => undefined))
    await waitForListener("scan:finished")

    runLaunchPass()

    await vi.waitFor(() => expect(overviewProgress().mode).toBe("firstRun"))
    await vi.waitFor(() => expect(overviewProgress().stepsDone).toBe(true))
    await vi.waitFor(() => expect(overviewProgress().dock.stepsDocked).toBe(1), {
      timeout: 2000,
    })
    await vi.waitFor(() => expect(overviewProgress().dock.stepsDocked).toBe(2), {
      timeout: 2000,
    })
    await vi.waitFor(() => expect(overviewProgress().dock.stepsDocked).toBe(3), {
      timeout: 2000,
    })
    await vi.waitFor(() => expect(overviewProgress().resultReady).toBe(true))
    await vi.waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith("finish_first_run"))

    const steps = noteInteractionCalls().filter(
      ({ interaction }) => interaction.kind === "firstRunStepReached",
    )
    expect(steps.filter((call) => call.interaction.step === "started")).toHaveLength(1)
    const found = steps.filter((call) => call.interaction.step === "found")
    expect(found).toHaveLength(1)
    expect(found[0]?.interaction.sessions).toBe(49)
    expect(steps.filter((call) => call.interaction.step === "read")).toHaveLength(1)
    expect(steps.filter((call) => call.interaction.step === "checked")).toHaveLength(1)
    const result = steps.filter((call) => call.interaction.step === "result")
    expect(result).toHaveLength(1)
    expect(result[0]?.interaction.result).toBe("clean")
    // The shell command records `first_run_finished` after it saves.
    expect(
      noteInteractionCalls().filter(
        ({ interaction }) => interaction.kind === "firstRunFinished",
      ),
    ).toHaveLength(0)
  })

  it("reports nothing in steady mode, where the funnel never shows", async () => {
    // onboardingCompleted stays true (the default fixture), so the device
    // never enters firstRun mode and the funnel events have nothing to fire.
    runLaunchPass()

    const { subscribeOverviewProgress, overviewProgress } =
      await import("./overviewProgressStore")
    stops.push(subscribeOverviewProgress(() => undefined))

    await vi.waitFor(() => expect(overviewProgress().mode).toBe("steady"))
    await vi.waitFor(() => expect(overviewProgress().check.done).toBe(true))
    expect(noteInteractionCalls()).toHaveLength(0)
    expect(mocks.invoke).not.toHaveBeenCalledWith("finish_first_run")
  })
})

describe("overviewProgressStore's enableNonRepoFolders", () => {
  it("fires include_non_repo_folders only on an actual transition", async () => {
    const { subscribeOverviewProgress, enableNonRepoFolders } =
      await import("./overviewProgressStore")
    stops.push(subscribeOverviewProgress(() => undefined))
    await vi.waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith("get_settings"))

    await enableNonRepoFolders()
    expect(mocks.invoke).toHaveBeenCalledWith("note_interaction", {
      interaction: { kind: "firstRunAction", action: "include_non_repo_folders" },
    })

    mocks.invoke.mockClear()
    // A device that already has the setting on must skip both the write and
    // the event. Simulated by answering get_settings with it already true.
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === "get_settings") {
        return { ...DEFAULT_TEST_SETTINGS, includeNonRepoFolders: true }
      }
      throw new Error(`Unexpected command: ${command}`)
    })
    await enableNonRepoFolders()
    expect(mocks.invoke).not.toHaveBeenCalledWith("set_settings", expect.anything())
    expect(mocks.invoke).not.toHaveBeenCalledWith("note_interaction", expect.anything())
  })
})

describe("overviewProgressStore's steady mode", () => {
  it("does not change the Find rows in the snapshot when a new full pass starts", async () => {
    // The device has scanned before: a full pass already ran and finished
    // before this test's store ever subscribed, same shape as case (b).
    runLaunchPass()

    const { subscribeOverviewProgress, overviewProgress } =
      await import("./overviewProgressStore")
    stops.push(subscribeOverviewProgress(() => undefined))

    await vi.waitFor(() => expect(overviewProgress().mode).toBe("steady"))
    expect(overviewProgress().find.rows).toEqual([
      { agent: "claude-code", label: "Claude Code", sessions: 49, done: true },
    ])

    await waitForListener("scan:started")
    // A routine 5-minute tick (or another launch) starts a fresh pass:
    // discovery resets to empty, same as every full pass's start.
    controller.start()

    expect(overviewProgress().find.rows).toEqual([
      { agent: "claude-code", label: "Claude Code", sessions: 49, done: true },
    ])
  })
})
