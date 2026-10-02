// The Overview's progress store. Subscribes to the scan status store, the
// checks report, and settings — the external-system boundary — so no
// component needs an effect. Replaces the fake-timer prototype
// (`ftuePrototype.ts`) with the real scan and check pipeline.
//
// Feeds the row of compact cells above Recent sessions. During the first run
// the row shows the find/read/check steps as they finish; afterwards it is
// a permanent status row for the current 30 days. The first-run-only latch
// lives in `firstRun.ts`; this module adds the steady state on top of it.

import {
  cancelChecksReport,
  getChecksReport,
  onChecksReportChanged,
  type BurnCheckDetectorId,
  type ChecksCategoryPayload,
  type ChecksReportPayload,
} from "../../../lib/insightsIpc"
import {
  finishFirstRun,
  ftueDiag, // TEMP ftue-diag
  getFolderPermissions,
  getSettings,
  onFtueReset,
  onSettingsChanged,
  setSettings,
  type AgentFoundCount,
  type AppSettings,
  type ReadGateCounts,
  type ScanHistoryProgress,
  type ScanStatus,
} from "../../../lib/ipc"
import { agentDisplayName } from "../../../lib/presentation/agents"
import { CHECK_LABELS } from "../../../lib/presentation/checkDefinitions"
import { scanStatusStore } from "../../../lib/scanStatusStore"
import type { DeferredPermissionDir } from "../../../lib/types/repository"
import { withViewTransition } from "../../../lib/viewTransition"
import {
  DOCK_START_DELAY_MS,
  DOCK_STEP_PAUSE_MS,
  INITIAL_FIRST_RUN_LATCH,
  advanceFirstRunLatch,
  resetFirstRunLatch,
  unlatchReadOutcome,
  type FirstRunInputs,
  type FirstRunLatch,
} from "./firstRun"

/* -------------------------------------------------------------------------
 * Snapshot shape the Overview renders.
 * ---------------------------------------------------------------------- */

interface FindRow {
  /** The agent's discovery slug, for its icon. */
  agent: string
  /** Display label, already mapped from the discovery slug. */
  label: string
  sessions: number
  /** Whether this agent's search has finished. */
  done: boolean
}

interface FindStep {
  done: boolean
  rows: FindRow[]
}

interface ReadStep {
  done: boolean
  completed: number
  total: number
  /** The gate outcome, once the read stage this session has finished once. */
  gate: ReadGateCounts | null
  includeNonRepoFolders: boolean
  /** Protected folders the last pass declined to read. Shown in the Read
   *  step, in `firstRun` mode and whenever the opened steps show outside it. */
  deferred: DeferredPermissionDir[]
}

interface CheckStep {
  done: boolean
  windowSessions: number
  pendingEvidence: number
  /** Pending sessions that wait for a retry backoff, usually a live session
   *  whose transcript changed during its check. The step does not wait for
   *  them. */
  deferredEvidence: number
}

export type FixStatus = "needsFix" | "awaitingVerification" | "passing" | "notChecked"

export interface FixCategory {
  id: BurnCheckDetectorId
  label: string
  status: FixStatus
}

interface HistoryProgress {
  completed: number
  total: number
}

export interface OverviewProgress {
  /**
   * `pending`: the first-run latch has not decided yet. Render no row and no
   * middle overlay. `firstRun`: the steps block shows this session, as it
   * always has. `steady`: the device has scanned before; the row is a
   * permanent status row for the current 30 days.
   */
  mode: "pending" | "firstRun" | "steady"
  find: FindStep
  read: ReadStep
  check: CheckStep
  /** Every category in the checks report, for the persistent checklist. */
  categories: FixCategory[]
  failingCount: number
  /** The planned background history pass's progress. Null unless the
   *  backend reports it and it is still under way. */
  history: HistoryProgress | null
  /** Whether the find, read and check steps have all finished. */
  stepsDone: boolean
  dock: ProgressDock
  /**
   * Whether the check result (fixes found, clean, or empty) is on screen:
   * the check step is done, and, in `firstRun` mode, every step has docked.
   * `maybeFinishFirstRun` calls `finish_first_run` the moment this turns
   * true.
   */
  resultReady: boolean
}

/**
 * Where the steps and the fixes callout show: in the middle of the Fixes
 * section, or as compact cells in the row above Recent sessions.
 */
export interface ProgressDock {
  /** How many steps moved down to the row, in step order (0 to 3). */
  stepsDocked: number
  /** The reader opened the docked steps again. They show in the middle. */
  stepsOpen: boolean
  /** The fixes callout is in the row. */
  fixesDocked: boolean
}

export const INITIAL_DOCK: ProgressDock = {
  stepsDocked: 0,
  stepsOpen: false,
  fixesDocked: false,
}

/* -------------------------------------------------------------------------
 * Pure derivation. Exported so a test can drive it without any IPC mocking.
 * ---------------------------------------------------------------------- */

export interface ProgressInputs extends FirstRunInputs {
  includeNonRepoFolders: boolean
  deferred: DeferredPermissionDir[]
}

/**
 * The last finished pass's find and read numbers. Every full scan pass
 * resets `ScanStatus.foundByAgent` and `read` at the start of the pass, so a
 * row that read them live would pulse and count up every 5 minutes. The
 * store keeps these instead, and only the steady values read them (see
 * {@link advanceLastPass}).
 */
export interface LastPass {
  lastFound: AgentFoundCount[] | null
  lastRead: { completed: number; total: number } | null
}

export const INITIAL_LAST_PASS: LastPass = { lastFound: null, lastRead: null }

/**
 * Keeps the last finished pass's find and read numbers across a routine
 * pass that resets the live status to zero. `lastFound` updates once a
 * pass's discovery is entirely done; `lastRead` updates once a pass has
 * left the read stage, whether it is still saving or has finished.
 */
export function advanceLastPass(previous: LastPass, status: ScanStatus | null): LastPass {
  if (!status) return previous
  let { lastFound, lastRead } = previous
  if (status.foundByAgent.length > 0 && status.foundByAgent.every((row) => row.done)) {
    lastFound = status.foundByAgent
  }
  if (!status.running && status.phase !== "idle" && status.phase !== "finding") {
    lastRead = status.read
  }
  return { lastFound, lastRead }
}

function toFixCategory(category: ChecksCategoryPayload): FixCategory {
  const status: FixStatus =
    category.lifecycle === "failing"
      ? "needsFix"
      : category.lifecycle === "awaitingVerification"
        ? "awaitingVerification"
        : category.lifecycle === "passing"
          ? "passing"
          : "notChecked"
  return { id: category.id, label: CHECK_LABELS[category.id], status }
}

function deriveHistory(history: ScanHistoryProgress | undefined): HistoryProgress | null {
  if (!history) return null
  if (history.state !== "pending" && history.state !== "running") return null
  return { completed: history.completed, total: history.total }
}

function deriveMode(latch: FirstRunLatch): OverviewProgress["mode"] {
  if (!latch.decided) return "pending"
  return latch.showSteps ? "firstRun" : "steady"
}

export function deriveOverviewProgress(
  latch: FirstRunLatch,
  inputs: ProgressInputs,
  dock: ProgressDock,
  lastPass: LastPass,
): OverviewProgress {
  const mode = deriveMode(latch)
  const stepsDone = latch.step1Done && latch.step2Done && latch.step3Done
  // The switch from latched to steady values happens right after the third
  // step lands in the row — the numbers are the same at that moment, so the
  // switch shows no jump.
  const useSteadyValues =
    mode === "steady" || (mode === "firstRun" && stepsDone && dock.stepsDocked >= 3)

  const findRows = useSteadyValues
    ? (lastPass.lastFound ?? inputs.scanStatus?.foundByAgent ?? [])
    : latch.step1Done
      ? latch.step1Rows
      : (inputs.scanStatus?.foundByAgent ?? [])
  const find: FindStep = {
    done: latch.step1Done,
    rows: findRows.map((row) => ({
      agent: row.agent,
      label: agentDisplayName(row.agent),
      sessions: row.sessions,
      done: row.done,
    })),
  }

  const readSource = useSteadyValues
    ? (lastPass.lastRead ?? inputs.scanStatus?.read ?? { completed: 0, total: 0 })
    : latch.step2Done
      ? latch.step2Read
      : (inputs.scanStatus?.read ?? { completed: 0, total: 0 })
  const read: ReadStep = {
    done: latch.step2Done,
    completed: readSource.completed,
    total: readSource.total,
    gate: latch.step2Done ? latch.step2Gate : null,
    includeNonRepoFolders: inputs.includeNonRepoFolders,
    deferred: inputs.deferred,
  }

  const check: CheckStep = useSteadyValues
    ? {
        done: latch.step3Done,
        windowSessions: inputs.checksReport?.windowSessions ?? 0,
        pendingEvidence: inputs.checksReport?.pendingEvidence ?? 0,
        deferredEvidence: inputs.checksReport?.deferredEvidence ?? 0,
      }
    : latch.step3Done
      ? {
          done: true,
          windowSessions: latch.step3Check.windowSessions,
          pendingEvidence: latch.step3Check.deferredEvidence,
          deferredEvidence: latch.step3Check.deferredEvidence,
        }
      : {
          done: false,
          windowSessions: inputs.checksReport?.windowSessions ?? 0,
          pendingEvidence: inputs.checksReport?.pendingEvidence ?? 0,
          deferredEvidence: inputs.checksReport?.deferredEvidence ?? 0,
        }

  const categories = (inputs.checksReport?.categories ?? []).map(toFixCategory)
  const failingCount = categories.filter((category) => category.status === "needsFix").length
  // Exhaustive once `check.done`: a done check is empty, clean, or has fixes,
  // so "the result is known" collapses to `check.done` itself. 3 is find,
  // read and check — the steps `dock.stepsDocked` counts.
  const resultReady = check.done && (mode !== "firstRun" || dock.stepsDocked >= 3)
  return {
    mode,
    find,
    read,
    check,
    categories,
    failingCount,
    history: deriveHistory(inputs.scanStatus?.history),
    stepsDone,
    resultReady,
    dock,
  }
}

/* -------------------------------------------------------------------------
 * The live store: latch state plus the IPC boundary that feeds it.
 * ---------------------------------------------------------------------- */

let latch: FirstRunLatch = INITIAL_FIRST_RUN_LATCH
let liveScanStatus: ScanStatus | null = null
let liveChecksReport: ChecksReportPayload | null = null
let liveIncludeNonRepoFolders = false
let liveOnboardingCompleted: boolean | null = null
let liveDeferred: DeferredPermissionDir[] = []
let liveChecksReportCurrent = false
let lastPass: LastPass = INITIAL_LAST_PASS
// Counts scan status updates that show a running pass. A checks report
// request records this count, so a pass that starts before the report
// arrives makes that report not current.
let scanRunsSeen = 0
let requestChecks: (() => void) | null = null
let refreshFolderPermissions: (() => void) | null = null
let dock: ProgressDock = INITIAL_DOCK
let dockTimer: ReturnType<typeof setTimeout> | null = null
// Set once, the first time the result shows — see `maybeFinishFirstRun`.
let firstRunFinished = false

function currentInputs(): ProgressInputs {
  return {
    scanStatus: liveScanStatus,
    checksReport: liveChecksReport,
    includeNonRepoFolders: liveIncludeNonRepoFolders,
    deferred: liveDeferred,
    onboardingCompleted: liveOnboardingCompleted,
    checksReportCurrent: liveChecksReportCurrent,
  }
}

let snapshot: OverviewProgress = deriveOverviewProgress(latch, currentInputs(), dock, lastPass)
const listeners = new Set<() => void>()

function recompute(): void {
  snapshot = deriveOverviewProgress(latch, currentInputs(), dock, lastPass)
  for (const listener of listeners) listener()
  scheduleDocking()
  maybeFinishFirstRun()
}

/**
 * Commit the first run the moment its result first shows.
 *
 * Fires once per store generation, only in `firstRun` mode. A failure is
 * logged rather than retried: the next launch reads `onboardingCompleted`
 * still false and shows the first run again, which is an acceptable retry on
 * its own.
 */
function maybeFinishFirstRun(): void {
  if (firstRunFinished || snapshot.mode !== "firstRun" || !snapshot.resultReady) return
  firstRunFinished = true
  void finishFirstRun().catch((error: unknown) => {
    console.error("finishFirstRun failed", error)
  })
}

/**
 * Move the finished steps down to the row one at a time. Each move runs
 * in its own view transition, so the reader sees each step travel. The
 * chain stops while the reader has the steps open. Only the first run docks
 * this way; the steady row's cells show docked from the start.
 */
function scheduleDocking(): void {
  if (dockTimer != null || snapshot.mode !== "firstRun" || !snapshot.stepsDone) return
  if (dock.stepsOpen || dock.stepsDocked >= 3) return
  const delay = dock.stepsDocked === 0 ? DOCK_START_DELAY_MS : DOCK_STEP_PAUSE_MS
  dockTimer = setTimeout(() => {
    dockTimer = null
    updateDock({ stepsDocked: dock.stepsDocked + 1 })
  }, delay)
}

function clearDockTimer(): void {
  if (dockTimer != null) clearTimeout(dockTimer)
  dockTimer = null
}

function updateDock(change: Partial<ProgressDock>): void {
  withViewTransition(() => {
    dock = { ...dock, ...change }
    recompute()
  })
}

function onScanStatus(status: ScanStatus | null): void {
  const wasRunning = liveScanStatus?.running ?? false
  liveScanStatus = status
  lastPass = advanceLastPass(lastPass, status)
  if (status?.running) {
    scanRunsSeen += 1
    liveChecksReportCurrent = false
  } else if (wasRunning) {
    // The pass saved its sessions. Request a report that includes them, and
    // re-read which protected folders still need permission.
    requestChecks?.()
    refreshFolderPermissions?.()
  }
  // TEMP ftue-diag
  void ftueDiag("onScanStatus", {
    running: status?.running ?? null,
    phase: status?.phase ?? null,
    foundByAgentLen: status?.foundByAgent.length ?? null,
    read: status?.read ?? null,
    gate: status?.gate != null,
  })
  latch = advanceFirstRunLatch(latch, currentInputs())
  logLatch() // TEMP ftue-diag
  recompute()
}

function onChecksReport(report: ChecksReportPayload | null, current: boolean): void {
  liveChecksReport = report
  liveChecksReportCurrent = current
  // TEMP ftue-diag
  void ftueDiag("onChecksReport", {
    evidenceSettled: report?.evidenceSettled ?? null,
    windowSessions: report?.windowSessions ?? null,
    pendingEvidence: report?.pendingEvidence ?? null,
    deferredEvidence: report?.deferredEvidence ?? null,
    current,
  })
  latch = advanceFirstRunLatch(latch, currentInputs())
  logLatch() // TEMP ftue-diag
  recompute()
}

// TEMP ftue-diag
function logLatch(): void {
  void ftueDiag("latch", {
    decided: latch.decided,
    showSteps: latch.showSteps,
    step1Done: latch.step1Done,
    step2Done: latch.step2Done,
    step3Done: latch.step3Done,
  })
}

function onSettings(settings: AppSettings): void {
  liveIncludeNonRepoFolders = settings.includeNonRepoFolders
  liveOnboardingCompleted = settings.onboardingCompleted
  latch = advanceFirstRunLatch(latch, currentInputs())
  logLatch() // TEMP ftue-diag
  recompute()
}

function onReset(): void {
  latch = resetFirstRunLatch()
  clearDockTimer()
  dock = INITIAL_DOCK
  // The wipe clears `onboardingCompleted`, so this device is a first run
  // again until the pass the reset triggers finishes it.
  liveOnboardingCompleted = false
  liveChecksReportCurrent = false
  // The wipe also clears every session the last pass found and read.
  lastPass = INITIAL_LAST_PASS
  // The reset starts a new first run, so its result must call
  // `finish_first_run` again once it shows.
  firstRunFinished = false
  void ftueDiag("onReset", { generation, listeners: listeners.size }) // TEMP ftue-diag
  recompute()
}

/** Bring the docked steps back up to the middle, as a group. */
export function openSteps(): void {
  clearDockTimer()
  updateDock({ stepsOpen: true })
}

/** Move the open steps back down to the row, as a group. */
export function shrinkSteps(): void {
  updateDock({ stepsOpen: false, stepsDocked: 3 })
}

/** Move the fixes callout down to the row. */
export function shrinkFixes(): void {
  updateDock({ fixesDocked: true })
}

/** Bring the fixes callout back up to the middle. */
export function openFixes(): void {
  updateDock({ fixesDocked: false })
}

/**
 * Turn on `includeNonRepoFolders`, so sessions outside a git repository are
 * kept. Reuses the Sources pane's own settings write (PR #661: changing this
 * setting already triggers a rescan). Un-latches step 2's outcome, so that
 * rescan's gate counts — the ones the reader is waiting on — replace the
 * stale ones instead of being ignored like a routine pass.
 */
export async function enableNonRepoFolders(): Promise<void> {
  const current = await getSettings()
  if (current.includeNonRepoFolders) return
  await setSettings({ ...current, includeNonRepoFolders: true })
  latch = unlatchReadOutcome(latch)
  recompute()
}

/* ---- Ref-counted subscriptions: start on the first listener, stop on the
   last, same lifecycle as `createExternalStore`. ---- */

let generation = 0
let checksConsumerId: string | null = null
let nextConsumer = 0
const stops: Array<() => void> = []

async function attach(
  thisGeneration: number,
  pending: Promise<() => void>,
  label: string, // TEMP ftue-diag
): Promise<void> {
  const stop = await pending.catch((error: unknown) => {
    // TEMP ftue-diag
    void ftueDiag("attach rejected", { label, thisGeneration, error: String(error) })
    return null
  })
  // TEMP ftue-diag
  void ftueDiag("attach resolved", { label, thisGeneration, generation, ok: stop != null })
  if (!stop) return
  if (thisGeneration !== generation) stop()
  else stops.push(stop)
}

async function start(): Promise<void> {
  const thisGeneration = ++generation
  checksConsumerId = `overview-progress-${++nextConsumer}`
  const consumerId = checksConsumerId
  // TEMP ftue-diag
  void ftueDiag("start entry", { thisGeneration, listeners: listeners.size })

  function refreshChecks(): void {
    const runsAtRequest = scanRunsSeen
    const idleAtRequest = liveScanStatus?.running === false
    void getChecksReport(consumerId)
      .then((report) => {
        if (thisGeneration !== generation) return
        onChecksReport(report, idleAtRequest && runsAtRequest === scanRunsSeen)
      })
      .catch(() => undefined)
  }
  requestChecks = refreshChecks

  // Which protected folders still need permission, read directly rather
  // than from a push event. Called once at start, and again whenever a scan
  // pass finishes — a granted folder or a new protected folder only shows up
  // through this read.
  function refreshPermissions(): void {
    void getFolderPermissions()
      .then((permissions) => {
        if (thisGeneration !== generation) return
        liveDeferred = permissions.deferred
        // TEMP ftue-diag
        void ftueDiag("refreshFolderPermissions resolved", {
          deferredLen: permissions.deferred.length,
        })
        latch = advanceFirstRunLatch(latch, currentInputs())
        logLatch() // TEMP ftue-diag
        recompute()
      })
      .catch((error: unknown) => {
        // TEMP ftue-diag
        void ftueDiag("refreshFolderPermissions rejected", { error: String(error) })
      })
  }
  refreshFolderPermissions = refreshPermissions

  await Promise.all([
    attach(
      thisGeneration,
      Promise.resolve(
        scanStatusStore.subscribe(() => {
          if (thisGeneration === generation) onScanStatus(scanStatusStore.getSnapshot())
        }),
      ),
      "scanStatusStore", // TEMP ftue-diag
    ),
    attach(
      thisGeneration,
      onChecksReportChanged(() => {
        if (thisGeneration === generation) refreshChecks()
      }),
      "checksReportChanged", // TEMP ftue-diag
    ),
    attach(
      thisGeneration,
      onSettingsChanged((settings) => {
        if (thisGeneration === generation) onSettings(settings)
      }),
      "settingsChanged", // TEMP ftue-diag
    ),
    attach(
      thisGeneration,
      onFtueReset(() => {
        if (thisGeneration === generation) onReset()
      }),
      "ftueReset", // TEMP ftue-diag
    ),
  ])
  // TEMP ftue-diag
  void ftueDiag("after Promise.all", {
    thisGeneration,
    generation,
    stale: thisGeneration !== generation,
  })
  if (thisGeneration !== generation) return
  onScanStatus(scanStatusStore.getSnapshot())
  refreshChecks()
  refreshPermissions()
  void getSettings().then((settings) => {
    if (thisGeneration === generation) onSettings(settings)
  })
}

function stop(): void {
  generation += 1
  requestChecks = null
  refreshFolderPermissions = null
  clearDockTimer()
  // TEMP ftue-diag
  void ftueDiag("stop", { generation })
  for (const detach of stops.splice(0)) detach()
  if (checksConsumerId) void cancelChecksReport(checksConsumerId).catch(() => undefined)
  checksConsumerId = null
}

export function subscribeOverviewProgress(listener: () => void): () => void {
  listeners.add(listener)
  if (listeners.size === 1) void start()
  return () => {
    listeners.delete(listener)
    if (listeners.size === 0) stop()
  }
}

export function overviewProgress(): OverviewProgress {
  return snapshot
}
