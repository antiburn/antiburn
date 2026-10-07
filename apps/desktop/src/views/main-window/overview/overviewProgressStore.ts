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
  getFolderPermissions,
  getSettings,
  noteInteraction,
  onFtueReset,
  onSettingsChanged,
  refreshLiveUsage,
  startLiveUsage,
  type AgentFoundCount,
  type AppSettings,
  type FirstRunResult,
  type ScanHistoryProgress,
  type ScanStatus,
} from "../../../lib/ipc"
import { getMainWindowVisible, onMainWindowVisibilityChanged } from "../../../lib/mainWindowIpc"
import { agentDisplayName } from "../../../lib/presentation/agents"
import { CHECK_LABELS } from "../../../lib/presentation/checkDefinitions"
import { scanStatusStore } from "../../../lib/scanStatusStore"
import type { DeferredPermissionDir } from "../../../lib/types/repository"
import { withViewTransition } from "../../../lib/viewTransition"
import {
  INITIAL_FIRST_RUN_LATCH,
  advanceFirstRunLatch,
  resetFirstRunLatch,
  type FirstRunInputs,
  type FirstRunLatch,
} from "./firstRun"

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
  /** Sessions in the current 30-day window. The first-run steps and their
   *  `stepDone` gating read this and `total`, never the combined figures
   *  below, so the gate a reader is waiting on never moves with the
   *  background history pass. */
  completed: number
  total: number
  /** What the Sessions step shows: `completed`/`total` plus the background
   *  history pass's own completed/total, once that pass has sessions of its
   *  own to report. Equal to `completed`/`total` at every other time. */
  displayCompleted: number
  displayTotal: number
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

export type FixStatus =
  "needsFix" | "awaitingVerification" | "passing" | "notChecked" | "snoozed"

export interface FixCategory {
  id: BurnCheckDetectorId
  label: string
  status: FixStatus
  /** Hundredths of one percent; null when the check has no estimate. */
  estimatedBurnBasisPoints: number | null
}

/** History has no total until discovery finishes. */
export type HistoryState = "pending" | "looking" | "reading" | "done"

export interface HistoryProgress {
  state: HistoryState
  /** Older sessions whose full log the pass has read so far. 0 in
   *  `"pending"` and `"looking"`, where there is nothing to count yet. */
  completed: number
  /** Older sessions the pass has found. 0 in `"pending"` and `"looking"`. */
  total: number
}

/** Steady mode always exposes `done`. */
export type FlowStep =
  "welcome" | "agents" | "limits" | "sessions" | "checks" | "fixes" | "done"

/** Limits docks into the provider pane instead of a nav row. */
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

const STEP_DOCKED_AT: Record<ProgressStepKey, FlowStep> = {
  agents: "limits",
  sessions: "checks",
  checks: "fixes",
  fixes: "done",
}

export function fixesFound(progress: OverviewProgress): boolean {
  return progress.checks.windowSessions > 0 && progress.failingCount > 0
}

/** The failing check with the highest estimated burn: the top row of the
 *  Checks list, which uses the same order. Ties keep report order. */
export function firstFailingCheck(progress: OverviewProgress): BurnCheckDetectorId | undefined {
  let top: FixCategory | undefined
  for (const category of progress.categories) {
    if (category.status !== "needsFix") continue
    if (
      !top ||
      (category.estimatedBurnBasisPoints ?? -1) > (top.estimatedBurnBasisPoints ?? -1)
    ) {
      top = category
    }
  }
  return top?.id
}

// Skip Limits on a revisit if live usage is already active.
function nextFlow(from: ProgressStepKey, liveUsageOn: boolean): FlowStep {
  if (from === "agents" && liveUsageOn) return "sessions"
  return STEP_DOCKED_AT[from]
}

export function stepDocked(flow: FlowStep, step: ProgressStepKey): boolean {
  return flowIndex(flow) >= flowIndex(STEP_DOCKED_AT[step])
}

// Reuse the name when a card moves within the takeover. The nav rows and the
// step modal carry no name, so they open and close without motion.
export function progressStepTransitionName(step: ProgressStepKey): string {
  return `progress-step-${step}`
}

export const LIVE_LIMITS_TRANSITION_NAME = "progress-live-limits"

export const RECENT_SESSIONS_TRANSITION_NAME = "overview-recent-sessions"

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
  /** A control to reveal and focus inside the open step's settings, from a
   *  search result. Null when the modal opened without one — a nav-row
   *  click, say — and the modal shows no particular control. */
  openStepControl: string | null
  /** Bumped on every `openProgressStep` call that names a control, so
   *  choosing the same control twice still re-reveals it. */
  openStepControlRevision: number
  /**
   * Whether the takeover shows the card for `flow`. False while the previous
   * card moves to its place, so the next card appears only after it lands.
   */
  stepShown: boolean
  actionPending: boolean
  actionError: string | null
  agents: AgentsStep
  sessions: SessionsStep
  checks: ChecksStep
  /** Every category in the checks report, for the persistent checklist. */
  categories: FixCategory[]
  failingCount: number
  /** Hide history until the first-run flow finishes. */
  history: HistoryProgress | null
}

export interface ProgressInputs extends FirstRunInputs {
  deferred: DeferredPermissionDir[]
}

/** Keep docked counts stable while a routine scan resets its live counters. */
export interface LastPass {
  lastFound: AgentFoundCount[] | null
  lastRead: { completed: number; total: number } | null
}

export const INITIAL_LAST_PASS: LastPass = { lastFound: null, lastRead: null }

// Saving counts contain admitted sessions; discovery counts include rejected candidates.
export function advanceLastPass(previous: LastPass, status: ScanStatus | null): LastPass {
  if (!status) return previous
  let { lastFound, lastRead } = previous
  if (
    status.phase === "saving" &&
    status.foundByAgent.length > 0 &&
    status.foundByAgent.every((row) => row.done)
  ) {
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
  return {
    id: category.id,
    label: CHECK_LABELS[category.id],
    status,
    estimatedBurnBasisPoints: category.estimatedTokenBurnBasisPoints ?? null,
  }
}

function deriveHistory(
  exposedFlow: FlowStep,
  history: ScanHistoryProgress | undefined,
): HistoryProgress | null {
  if (exposedFlow !== "done") return null
  if (!history || history.state === "none") return null
  if (history.state === "pending") return { state: "pending", completed: 0, total: 0 }
  if (history.state === "running") {
    return history.total === 0
      ? { state: "looking", completed: 0, total: 0 }
      : { state: "reading", completed: history.completed, total: history.total }
  }
  return { state: "done", completed: history.completed, total: history.total }
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
  openStepControl: string | null = null,
  openStepControlRevision = 0,
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
  const history = deriveHistory(exposedFlow, inputs.scanStatus?.history)
  // The combined figures count the history pass's own sessions once it has
  // some to report. Before then — including throughout the first-run steps,
  // where `history` is always null — they equal the 30-day numbers.
  const historyToCount =
    history && (history.state === "reading" || history.state === "done") ? history : null
  const sessions: SessionsStep = {
    done: latch.sessionsDone,
    completed: sessionsSource.completed,
    total: sessionsSource.total,
    displayCompleted: historyToCount
      ? sessionsSource.completed + historyToCount.completed
      : sessionsSource.completed,
    displayTotal: historyToCount
      ? sessionsSource.total + historyToCount.total
      : sessionsSource.total,
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
    openStepControl,
    openStepControlRevision,
    stepShown,
    actionPending: false,
    actionError: null,
    agents,
    sessions,
    checks,
    categories,
    failingCount,
    history,
  }
}

let latch: FirstRunLatch = INITIAL_FIRST_RUN_LATCH
let flow: FlowStep = "welcome"
let openStep: ProgressStepKey | null = null
let openStepControl: string | null = null
let openStepControlRevision = 0
let stepShown = true
let actionPending = false
let actionError: string | null = null
let flowRevision = 0
let liveScanStatus: ScanStatus | null = null
let liveChecksReport: ChecksReportPayload | null = null
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
  openStepControl,
  openStepControlRevision,
)
const listeners = new Set<() => void>()

function recompute(): void {
  snapshot = deriveOverviewProgress(
    latch,
    currentInputs(),
    flow,
    openStep,
    stepShown,
    lastPass,
    openStepControl,
    openStepControlRevision,
  )
  snapshot = { ...snapshot, actionPending, actionError }
  for (const listener of listeners) listener()
  maybeReportFirstRunSteps()
}

/** Which result the finished check step shows, for `first_run_step_reached`. */
function firstRunResult(progress: OverviewProgress): FirstRunResult {
  if (progress.checks.windowSessions === 0) return "empty"
  if (progress.categories.length === 0) return "checks_disabled"
  return progress.failingCount === 0 ? "clean" : "fixes_found"
}

// Report completed work once. Report the result only when its step opens.
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
  const previous = liveScanStatus
  liveScanStatus = status
  lastPass = advanceLastPass(lastPass, status)
  if (status?.running) {
    scanRunsSeen += 1
    liveChecksReportCurrent = false
  } else if (status && (previous == null || previous.running)) {
    // The pass saved its sessions. Request a report that includes them, and
    // re-read which protected folders still need permission.
    requestChecks?.()
    refreshFolderPermissions?.()
  }
  latch = advanceFirstRunLatch(latch, currentInputs())
  recompute()
}

function onChecksReport(report: ChecksReportPayload | null, current: boolean): void {
  liveChecksReport = report
  liveChecksReportCurrent = current
  latch = advanceFirstRunLatch(latch, currentInputs())
  recompute()
}

function onSettings(settings: AppSettings): void {
  liveOnboardingCompleted = settings.onboardingCompleted
  liveUsageOn = settings.liveUsageEnabled && settings.liveUsageStarted
  latch = advanceFirstRunLatch(latch, currentInputs())
  recompute()
}

function onReset(): void {
  flowRevision += 1
  actionPending = false
  actionError = null
  latch = resetFirstRunLatch()
  flow = "welcome"
  openStep = null
  openStepControl = null
  stepShown = true
  // The wipe clears `onboardingCompleted`, so this device is a first run
  // again until the pass the reset triggers finishes it.
  liveOnboardingCompleted = false
  // The wipe also clears every session the last pass found and read, so the
  // step cards must not show the old pass, report or folders.
  liveScanStatus = null
  liveChecksReport = null
  liveChecksReportCurrent = false
  liveDeferred = []
  lastPass = INITIAL_LAST_PASS
  // A checks report requested before the wipe is then not current.
  scanRunsSeen += 1
  // The reset starts a new first run, so its funnel must report again.
  reportedFirstRunStarted = false
  reportedFirstRunFound = false
  reportedFirstRunRead = false
  reportedFirstRunChecked = false
  reportedFirstRunResult = false
  recompute()
  refreshFolderPermissions?.()
}

export async function showLiveLimits(): Promise<void> {
  if (flow !== "limits" || !stepShown || actionPending) return
  const revision = flowRevision
  await startLiveUsage()
  if (revision !== flowRevision) return
  // Starting collects nothing. Ask for the first reading now, as Settings →
  // Usage does, so the pane does not wait for the next background pass.
  void refreshLiveUsage().catch(() => undefined)
  noteInteraction({ kind: "firstRunAction", action: "live_usage_started" })
  await moveTo("sessions")
}

export function skipLiveLimits(): void {
  if (flow !== "limits" || !stepShown || actionPending) return
  noteInteraction({ kind: "firstRunAction", action: "live_usage_skipped" })
  void moveTo("sessions")
}

// Confirm the backend command before the next step becomes visible.
async function moveTo(to: FlowStep): Promise<boolean> {
  if (actionPending) return false
  const revision = flowRevision
  actionPending = true
  actionError = null
  recompute()
  try {
    if (to === "agents" || to === "sessions" || to === "checks") {
      await advanceFirstRun(to)
    } else if (to === "done") {
      await finishFirstRun()
    }
    if (revision !== flowRevision) return false
    await withViewTransition(() => {
      if (revision !== flowRevision) return
      flow = to
      openStep = null
      stepShown = false
      recompute()
    })
    if (revision !== flowRevision) return false
    if (to === "fixes" && !reportedFirstRunResult) {
      reportedFirstRunResult = true
      noteInteraction({
        kind: "firstRunStepReached",
        step: "result",
        result: firstRunResult(snapshot),
      })
    }
    await withViewTransition(() => {
      if (revision !== flowRevision) return
      stepShown = true
      recompute()
    })
    return true
  } catch {
    if (revision === flowRevision) {
      stepShown = true
      actionError = "Could not continue setup. Try again."
    }
    return false
  } finally {
    if (revision === flowRevision) {
      actionPending = false
      recompute()
    }
  }
}

export function stepDone(step: ProgressStepKey, progress: OverviewProgress): boolean {
  return step === "fixes" || progress[step].done
}

export async function nextStep(): Promise<void> {
  const from = flow
  // `stepShown` is false while a move runs, so a second press cannot skip a
  // step.
  if (!stepShown || actionPending) return
  if (from === "welcome") {
    await moveTo("agents")
    return
  }
  if (from !== "agents" && from !== "sessions" && from !== "checks" && from !== "fixes") return
  if (!stepDone(from, snapshot)) return
  await moveTo(nextFlow(from, liveUsageOn))
}

export async function enhanceFixes(): Promise<boolean> {
  if (flow !== "fixes" || !stepShown || actionPending) return false
  const finished = await moveTo("done")
  if (finished) noteInteraction({ kind: "firstRunAction", action: "enhance_opened" })
  return finished
}

// Rewinding does not lower the backend gate or restart completed work.
export function rewindTo(step: ProgressStepKey): void {
  if (snapshot.mode !== "firstRun" || flow === "done" || !stepShown || actionPending) return
  if (!stepDocked(flow, step)) return
  void withViewTransition(() => {
    flow = step
    openStep = null
    recompute()
  })
}

/** A control target reveals and focuses its row without changing its value. */
export function openProgressStep(key: ProgressStepKey, control?: string): void {
  openStep = key
  if (control) {
    openStepControl = control
    openStepControlRevision += 1
  } else {
    openStepControl = null
  }
  recompute()
  // `fixes` has no settings — `StepSettings` renders nothing for it — so
  // opening its modal reports no exposure.
  if (key !== "fixes") {
    noteInteraction({ kind: "stepSettingsViewed", label: key, detail: "modal" })
  }
}

export function closeProgressStep(): void {
  openStep = null
  recompute()
}

/* ---- Ref-counted subscriptions: start on the first listener, stop on the
   last, same lifecycle as `createExternalStore`. ---- */

let generation = 0
let checksConsumerId: string | null = null
let nextConsumer = 0
const stops: Array<() => void> = []

async function attach(thisGeneration: number, pending: Promise<() => void>): Promise<void> {
  const stop = await pending.catch(() => null)
  if (!stop) return
  if (thisGeneration !== generation) stop()
  else stops.push(stop)
}

async function start(): Promise<void> {
  const thisGeneration = ++generation
  let visible = false
  let visibilityRevision = 0
  let settingsRevision = 0
  let readGeneration = 0
  let checksPending = false
  let checksDirty = false
  let permissionRequest = 0

  function refreshChecks(): void {
    if (!visible) return
    checksDirty = true
    if (checksPending) return
    checksPending = true
    checksDirty = false
    const read = readGeneration
    const runsAtRequest = scanRunsSeen
    const idleAtRequest = liveScanStatus?.running === false
    void getChecksReport(checksConsumerId!)
      .then((report) => {
        if (thisGeneration !== generation || read !== readGeneration) return
        onChecksReport(report, idleAtRequest && runsAtRequest === scanRunsSeen)
      })
      .catch(() => undefined)
      .finally(() => {
        if (thisGeneration !== generation || read !== readGeneration) return
        checksPending = false
        if (checksDirty) refreshChecks()
      })
  }
  requestChecks = refreshChecks

  function refreshPermissions(): void {
    if (!visible) return
    const request = ++permissionRequest
    void getFolderPermissions()
      .then((permissions) => {
        if (thisGeneration !== generation || request !== permissionRequest) return
        liveDeferred = permissions.deferred
        recompute()
      })
      .catch(() => undefined)
  }
  refreshFolderPermissions = refreshPermissions

  function setVisible(next: boolean): void {
    if (visible === next) return
    visible = next
    readGeneration += 1
    permissionRequest += 1
    checksPending = false
    if (!visible) {
      if (checksConsumerId) void cancelChecksReport(checksConsumerId).catch(() => undefined)
      checksConsumerId = null
      return
    }
    checksConsumerId = `overview-progress-${++nextConsumer}`
    refreshChecks()
    refreshPermissions()
  }

  await Promise.all([
    attach(
      thisGeneration,
      onMainWindowVisibilityChanged((visible) => {
        if (thisGeneration !== generation) return
        visibilityRevision += 1
        setVisible(visible)
      }),
    ),
    attach(
      thisGeneration,
      Promise.resolve(
        scanStatusStore.subscribe(() => {
          if (thisGeneration === generation) onScanStatus(scanStatusStore.getSnapshot())
        }),
      ),
    ),
    attach(
      thisGeneration,
      onChecksReportChanged(() => {
        if (thisGeneration === generation) refreshChecks()
      }),
    ),
    attach(
      thisGeneration,
      onSettingsChanged((settings) => {
        if (thisGeneration !== generation) return
        settingsRevision += 1
        onSettings(settings)
      }),
    ),
    attach(
      thisGeneration,
      onFtueReset(() => {
        if (thisGeneration === generation) onReset()
      }),
    ),
  ])
  if (thisGeneration !== generation) return
  onScanStatus(scanStatusStore.getSnapshot())
  const visibilityAtRead = visibilityRevision
  void getMainWindowVisible()
    .then((visible) => {
      if (thisGeneration === generation && visibilityRevision === visibilityAtRead)
        setVisible(visible)
    })
    .catch(() => undefined)
  const settingsAtRead = settingsRevision
  void getSettings()
    .then((settings) => {
      if (thisGeneration === generation && settingsRevision === settingsAtRead)
        onSettings(settings)
    })
    .catch(() => undefined)
}

function stop(): void {
  generation += 1
  requestChecks = null
  refreshFolderPermissions = null
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
