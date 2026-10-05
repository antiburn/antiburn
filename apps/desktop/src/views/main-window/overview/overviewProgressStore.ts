// The Overview's progress store. Subscribes to the scan status store, the
// checks report, and settings — the external-system boundary — so no
// component needs an effect. Replaces the fake-timer prototype
// (`ftuePrototype.ts`) with the real scan and check pipeline.
//
// Drives the first-run takeover (one step at a time, each gated on the
// reader's own Next) and the permanent side-nav status rows. The
// first-run-only latch lives in `firstRun.ts`; this module adds the flow
// and the steady state on top of it.

import {
  cancelChecksReport,
  getChecksReport,
  onChecksReportChanged,
  type BurnCheckDetectorId,
  type ChecksCategoryPayload,
  type ChecksReportPayload,
} from "../../../lib/insightsIpc"
import {
  advanceFirstRun,
  finishFirstRun,
  ftueDiag, // TEMP ftue-diag
  getFolderPermissions,
  getSettings,
  noteInteraction,
  onFtueReset,
  onSettingsChanged,
  refreshLiveUsage,
  setSettings,
  startLiveUsage,
  type AgentFoundCount,
  type AppSettings,
  type FirstRunResult,
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
  INITIAL_FIRST_RUN_LATCH,
  advanceFirstRunLatch,
  resetFirstRunLatch,
  unlatchSessionsOutcome,
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

interface AgentsStep {
  done: boolean
  rows: FindRow[]
}

interface SessionsStep {
  done: boolean
  completed: number
  total: number
  /** The gate outcome, once the read stage this session has finished once. */
  gate: ReadGateCounts | null
  includeNonRepoFolders: boolean
  /** Protected folders the last pass declined to read. Shown in the Read
   *  step, wherever that step's content shows. */
  deferred: DeferredPermissionDir[]
}

interface ChecksStep {
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

/**
 * Where the first-run takeover is, in order. Meaningful only in `firstRun`
 * mode: a new first run starts at `"welcome"`, and `steady` mode behaves as
 * `"done"`.
 */
export type FlowStep =
  "welcome" | "agents" | "limits" | "sessions" | "checks" | "fixes" | "done"

/** A step with its own nav row and modal. `"limits"` and `"done"` have
 *  neither: the live limits step moves into the right-hand pane instead, and
 *  `"done"` is the takeover's end, not a step. */
export type ProgressStepKey = "agents" | "sessions" | "checks" | "fixes"

const FLOW_ORDER: readonly FlowStep[] = [
  "welcome",
  "agents",
  "limits",
  "sessions",
  "checks",
  "fixes",
  "done",
]

function flowIndex(step: FlowStep): number {
  return FLOW_ORDER.indexOf(step)
}

/** The flow stage reached once a step's own card has moved down to its nav
 *  row — the stage `nextStep` advances *to* when that step's Next (or the
 *  fixes step's Done) is pressed. */
const STEP_DOCKED_AT: Record<ProgressStepKey, FlowStep> = {
  agents: "limits",
  sessions: "checks",
  checks: "fixes",
  fixes: "done",
}

/** Whether the fixes step has fixes to show: the window has sessions, and
 *  at least one check fails. */
export function fixesFound(progress: OverviewProgress): boolean {
  return progress.checks.windowSessions > 0 && progress.failingCount > 0
}

/** The first check the fixes step lists as needing a fix: where Enhance
 *  takes the reader in Burn Checks. */
export function firstFailingCheck(progress: OverviewProgress): BurnCheckDetectorId | undefined {
  return progress.categories.find((category) => category.status === "needsFix")?.id
}

/** Where `flow` moves on this step's Next (or the fixes step's Done). The
 *  live limits step shows only while live usage is off: a reader who went
 *  back to the agents step after Show live limits has nothing to do there. */
function nextFlow(from: ProgressStepKey, liveUsageOn: boolean): FlowStep {
  if (from === "agents" && liveUsageOn) return "sessions"
  return STEP_DOCKED_AT[from]
}

/**
 * Whether `step`'s card has already moved down to its nav row, at `flow`.
 *
 * Exported so a test, and `ProgressNav`, can derive row visibility from the
 * same rule the store uses for its own values: `steady`'s `flow` is always
 * `"done"` (see {@link deriveOverviewProgress}), so every step reads as
 * docked there without a separate steady-mode branch.
 */
export function stepDocked(flow: FlowStep, step: ProgressStepKey): boolean {
  return flowIndex(flow) >= flowIndex(STEP_DOCKED_AT[step])
}

/** Shared by a step's takeover card and its nav row — or its open modal, see
 *  `ProgressNav.tsx` — so a view transition moves the one element between
 *  wherever it currently lives. */
export function progressStepTransitionName(step: ProgressStepKey): string {
  return `progress-step-${step}`
}

/** Shared by the live limits card and the right-hand pane, so "Show live
 *  limits" moves the card into the pane as it appears. */
export const LIVE_LIMITS_TRANSITION_NAME = "progress-live-limits"

/** Shared by Recent sessions under the takeover and in the finished
 *  Overview, so the card moves to its place when the first run ends. */
export const RECENT_SESSIONS_TRANSITION_NAME = "overview-recent-sessions"

/** The same as {@link RECENT_SESSIONS_TRANSITION_NAME}, for the usage card. */
export const USAGE_TRANSITION_NAME = "overview-usage"

export interface OverviewProgress {
  /**
   * `pending`: the first-run latch has not decided yet. Render nothing.
   * `firstRun`: this session shows the takeover and the docking steps, as it
   * always has. `steady`: the device has scanned before; the nav rows are a
   * permanent status row for the current 30 days.
   */
  mode: "pending" | "firstRun" | "steady"
  /** The takeover's current stage. `"done"` in every mode but `firstRun`. */
  flow: FlowStep
  /** The step whose modal is open, if any. */
  openStep: ProgressStepKey | null
  /**
   * Whether the takeover shows the card for `flow`. False while the previous
   * card moves to its place, so the next card appears only after it lands.
   */
  stepShown: boolean
  agents: AgentsStep
  sessions: SessionsStep
  checks: ChecksStep
  /** Every category in the checks report, for the persistent checklist. */
  categories: FixCategory[]
  failingCount: number
  /** The planned background history pass's progress. Null unless the
   *  backend reports it and it is still under way. */
  history: HistoryProgress | null
}

/* -------------------------------------------------------------------------
 * Pure derivation. Exported so a test can drive it without any IPC mocking.
 * ---------------------------------------------------------------------- */

export interface ProgressInputs extends FirstRunInputs {
  includeNonRepoFolders: boolean
  deferred: DeferredPermissionDir[]
}

/**
 * The last finished pass's agents and read numbers. Every full scan pass
 * resets `ScanStatus.foundByAgent` and `read` at the start of the pass, so a
 * row that read them live would pulse and count up every 5 minutes. The
 * store keeps these instead, and only a docked row reads them (see
 * {@link advanceLastPass}).
 */
export interface LastPass {
  lastFound: AgentFoundCount[] | null
  lastRead: { completed: number; total: number } | null
}

export const INITIAL_LAST_PASS: LastPass = { lastFound: null, lastRead: null }

/**
 * Keeps the last finished pass's agents and read numbers across a routine
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
  // A pending pass has found nothing yet, and during a first run it waits
  // for the first run to finish. Show only a pass with sessions to report.
  if (history.state !== "running" || history.total === 0) return null
  return { completed: history.completed, total: history.total }
}

function deriveMode(latch: FirstRunLatch): OverviewProgress["mode"] {
  if (!latch.decided) return "pending"
  return latch.showSteps ? "firstRun" : "steady"
}

export function deriveOverviewProgress(
  latch: FirstRunLatch,
  inputs: ProgressInputs,
  flow: FlowStep,
  openStep: ProgressStepKey | null,
  stepShown: boolean,
  lastPass: LastPass,
): OverviewProgress {
  const mode = deriveMode(latch)
  // Steady behaves as "done": every step reads as docked, so the row always
  // shows the live/last-pass numbers rather than a one-time latch.
  const exposedFlow: FlowStep = mode === "steady" ? "done" : flow

  const agentsDocked = stepDocked(exposedFlow, "agents")
  const sessionsDocked = stepDocked(exposedFlow, "sessions")
  const checksDocked = stepDocked(exposedFlow, "checks")

  const agentsRows = agentsDocked
    ? (lastPass.lastFound ?? inputs.scanStatus?.foundByAgent ?? [])
    : latch.agentsDone
      ? latch.agentsFound
      : (inputs.scanStatus?.foundByAgent ?? [])
  const agents: AgentsStep = {
    done: latch.agentsDone,
    rows: agentsRows.map((row) => ({
      agent: row.agent,
      label: agentDisplayName(row.agent),
      sessions: row.sessions,
      done: row.done,
    })),
  }

  const sessionsSource = sessionsDocked
    ? (lastPass.lastRead ?? inputs.scanStatus?.read ?? { completed: 0, total: 0 })
    : latch.sessionsDone
      ? latch.sessionsRead
      : (inputs.scanStatus?.read ?? { completed: 0, total: 0 })
  const sessions: SessionsStep = {
    done: latch.sessionsDone,
    completed: sessionsSource.completed,
    total: sessionsSource.total,
    gate: latch.sessionsDone ? latch.sessionsGate : null,
    includeNonRepoFolders: inputs.includeNonRepoFolders,
    deferred: inputs.deferred,
  }

  const checks: ChecksStep = checksDocked
    ? {
        done: latch.checksDone,
        windowSessions: inputs.checksReport?.windowSessions ?? 0,
        pendingEvidence: inputs.checksReport?.pendingEvidence ?? 0,
        deferredEvidence: inputs.checksReport?.deferredEvidence ?? 0,
      }
    : latch.checksDone
      ? {
          done: true,
          windowSessions: latch.checksResult.windowSessions,
          pendingEvidence: latch.checksResult.deferredEvidence,
          deferredEvidence: latch.checksResult.deferredEvidence,
        }
      : {
          done: false,
          windowSessions: inputs.checksReport?.windowSessions ?? 0,
          pendingEvidence: inputs.checksReport?.pendingEvidence ?? 0,
          deferredEvidence: inputs.checksReport?.deferredEvidence ?? 0,
        }

  const categories = (inputs.checksReport?.categories ?? []).map(toFixCategory)
  const failingCount = categories.filter((category) => category.status === "needsFix").length

  return {
    mode,
    flow: exposedFlow,
    openStep,
    stepShown,
    agents,
    sessions,
    checks,
    categories,
    failingCount,
    history: deriveHistory(inputs.scanStatus?.history),
  }
}

/* -------------------------------------------------------------------------
 * The live store: latch and flow state plus the IPC boundary that feeds it.
 * ---------------------------------------------------------------------- */

let latch: FirstRunLatch = INITIAL_FIRST_RUN_LATCH
let flow: FlowStep = "welcome"
let openStep: ProgressStepKey | null = null
let stepShown = true
let liveScanStatus: ScanStatus | null = null
let liveChecksReport: ChecksReportPayload | null = null
let liveIncludeNonRepoFolders = false
let liveOnboardingCompleted: boolean | null = null
let liveUsageOn = false
let liveDeferred: DeferredPermissionDir[] = []
let liveChecksReportCurrent = false
let lastPass: LastPass = INITIAL_LAST_PASS
// Counts scan status updates that show a running pass. A checks report
// request records this count, so a pass that starts before the report
// arrives makes that report not current.
let scanRunsSeen = 0
let requestChecks: (() => void) | null = null
let refreshFolderPermissions: (() => void) | null = null
// One-shot flags for `antiburn.first_run_step_reached`. Reset alongside the
// latch whenever a wipe starts a new first run.
let reportedFirstRunStarted = false
let reportedFirstRunFound = false
let reportedFirstRunRead = false
let reportedFirstRunChecked = false
let reportedFirstRunResult = false

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

let snapshot: OverviewProgress = deriveOverviewProgress(
  latch,
  currentInputs(),
  flow,
  openStep,
  stepShown,
  lastPass,
)
const listeners = new Set<() => void>()

function recompute(): void {
  snapshot = deriveOverviewProgress(latch, currentInputs(), flow, openStep, stepShown, lastPass)
  for (const listener of listeners) listener()
  maybeReportFirstRunSteps()
}

/** Which result the finished check step shows, for `first_run_step_reached`. */
function firstRunResult(progress: OverviewProgress): FirstRunResult {
  if (progress.checks.windowSessions === 0) return "empty"
  return progress.failingCount === 0 ? "clean" : "fixes_found"
}

/**
 * Report each fixed first-run funnel step the first time its work is done.
 *
 * One flag per step, so a later re-render of the same finished step reports
 * nothing. Only in `firstRun` mode; a steady-state device never reaches this.
 * The `result` step is reported separately, from {@link nextStep}, the
 * moment the fixes step first shows — not from here, since a step's work can
 * finish well before the reader presses its Next.
 */
function maybeReportFirstRunSteps(): void {
  if (snapshot.mode !== "firstRun") return
  if (!reportedFirstRunStarted) {
    reportedFirstRunStarted = true
    noteInteraction({ kind: "firstRunStepReached", step: "started" })
  }
  if (!reportedFirstRunFound && snapshot.agents.done) {
    reportedFirstRunFound = true
    const sessions = snapshot.agents.rows.reduce((sum, row) => sum + row.sessions, 0)
    noteInteraction({ kind: "firstRunStepReached", step: "found", sessions })
  }
  if (!reportedFirstRunRead && snapshot.sessions.done) {
    reportedFirstRunRead = true
    noteInteraction({ kind: "firstRunStepReached", step: "read" })
  }
  if (!reportedFirstRunChecked && snapshot.checks.done) {
    reportedFirstRunChecked = true
    noteInteraction({ kind: "firstRunStepReached", step: "checked" })
  }
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
    agentsDone: latch.agentsDone,
    sessionsDone: latch.sessionsDone,
    checksDone: latch.checksDone,
  })
}

function onSettings(settings: AppSettings): void {
  liveIncludeNonRepoFolders = settings.includeNonRepoFolders
  liveOnboardingCompleted = settings.onboardingCompleted
  liveUsageOn = settings.liveUsageEnabled && settings.liveUsageStarted
  latch = advanceFirstRunLatch(latch, currentInputs())
  logLatch() // TEMP ftue-diag
  recompute()
}

function onReset(): void {
  latch = resetFirstRunLatch()
  flow = "welcome"
  openStep = null
  stepShown = true
  // The wipe clears `onboardingCompleted`, so this device is a first run
  // again until the pass the reset triggers finishes it.
  liveOnboardingCompleted = false
  liveChecksReportCurrent = false
  // The wipe also clears every session the last pass found and read.
  lastPass = INITIAL_LAST_PASS
  // The reset starts a new first run, so its funnel must report again.
  reportedFirstRunStarted = false
  reportedFirstRunFound = false
  reportedFirstRunRead = false
  reportedFirstRunChecked = false
  reportedFirstRunResult = false
  void ftueDiag("onReset", { generation, listeners: listeners.size }) // TEMP ftue-diag
  recompute()
}

/**
 * Start live usage from the live limits step. On success, moves to the read
 * step and opens the backend's sessions gate. On failure, the flow stays on the
 * live limits step — nothing here mutates state before `startLiveUsage`
 * settles — so the caller's own catch can show an error line beside the
 * button.
 */
export async function showLiveLimits(): Promise<void> {
  if (flow !== "limits" || !stepShown) return
  await startLiveUsage()
  // Starting collects nothing. Ask for the first reading now, as Settings →
  // Usage does, so the pane does not wait for the next background pass.
  void refreshLiveUsage().catch(() => undefined)
  noteInteraction({ kind: "firstRunAction", action: "live_usage_started" })
  await moveTo("sessions")
}

/** Skip live usage and move to the Sessions step. Starts no live usage. */
export function skipLiveLimits(): void {
  if (flow !== "limits" || !stepShown) return
  noteInteraction({ kind: "firstRunAction", action: "live_usage_skipped" })
  void moveTo("sessions")
}

/**
 * Move the takeover to `to` in two view transitions. The first moves the
 * current card to its place and shows no card. The work of `to` starts when
 * that card lands. The second transition then shows the card for `to`.
 */
async function moveTo(to: FlowStep): Promise<void> {
  await withViewTransition(() => {
    flow = to
    openStep = null
    stepShown = false
    recompute()
  })
  if (to === "agents" || to === "sessions" || to === "checks") {
    void advanceFirstRun(to)
  } else if (to === "fixes" && !reportedFirstRunResult) {
    reportedFirstRunResult = true
    noteInteraction({
      kind: "firstRunStepReached",
      step: "result",
      result: firstRunResult(snapshot),
    })
  } else if (to === "done") {
    // `finish_first_run` records `first_run_finished` itself, only when it
    // saves the change.
    void finishFirstRun().catch((error: unknown) => {
      console.error("finishFirstRun failed", error)
    })
  }
  await withViewTransition(() => {
    stepShown = true
    recompute()
  })
}

/** Whether the step the takeover currently shows has finished its work. */
function currentStepDone(step: FlowStep): boolean {
  switch (step) {
    case "agents":
      return snapshot.agents.done
    case "sessions":
      return snapshot.sessions.done
    case "checks":
      return snapshot.checks.done
    case "fixes":
      return true
    default:
      return false
  }
}

/**
 * Move the takeover to its next stage, from the reader's Next (or the fixes
 * step's Done). Refused while the current step's work is not done yet.
 *
 * Each move runs its own view transition, carrying the finished step's card
 * down into its nav row. `fixes` → `done` ends the first run.
 */
export async function nextStep(): Promise<void> {
  const from = flow
  // `stepShown` is false while a move runs, so a second press cannot skip a
  // step.
  if (!stepShown) return
  if (from === "welcome") {
    await moveTo("agents")
    return
  }
  if (from !== "agents" && from !== "sessions" && from !== "checks" && from !== "fixes") return
  if (!currentStepDone(from)) return
  await moveTo(nextFlow(from, liveUsageOn))
}

/**
 * Enhance on the fixes step: records the choice, then finishes the first run
 * the same way Done does.
 */
export async function enhanceFixes(): Promise<void> {
  if (flow !== "fixes" || !stepShown) return
  noteInteraction({ kind: "firstRunAction", action: "enhance_opened" })
  await moveTo("done")
}

/**
 * Take the first run back to a step whose card is in the nav, from a click
 * on its row. The row's card moves back up into the takeover, and the rows
 * of the later steps leave the nav. The backend gate stays where it is:
 * work that a step already started keeps running, and the step's Next
 * moves forward again without a new wait.
 */
export function rewindTo(step: ProgressStepKey): void {
  if (snapshot.mode !== "firstRun" || flow === "done" || !stepShown) return
  if (!stepDocked(flow, step)) return
  void withViewTransition(() => {
    flow = step
    openStep = null
    recompute()
  })
}

/** Open one step's modal over the current view. */
export function openProgressStep(key: ProgressStepKey): void {
  void withViewTransition(() => {
    openStep = key
    recompute()
  })
}

/** Close the open step modal. */
export function closeProgressStep(): void {
  void withViewTransition(() => {
    openStep = null
    recompute()
  })
}

/**
 * Turn on `includeNonRepoFolders`, so sessions outside a git repository are
 * kept. Reuses the Sources pane's own settings write (PR #661: changing this
 * setting already triggers a rescan). Un-latches the Sessions step's outcome, so that
 * rescan's gate counts — the ones the reader is waiting on — replace the
 * stale ones instead of being ignored like a routine pass.
 */
export async function enableNonRepoFolders(): Promise<void> {
  const current = await getSettings()
  if (current.includeNonRepoFolders) return
  await setSettings({ ...current, includeNonRepoFolders: true })
  noteInteraction({ kind: "firstRunAction", action: "include_non_repo_folders" })
  latch = unlatchSessionsOutcome(latch)
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
