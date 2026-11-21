import {
  cancelChecksReport,
  getChecksReport,
  onChecksReportChanged,
  type CheckId,
  type ChecksReportPayload,
} from "../../../lib/insightsIpc"
import {
  getFolderPermissions,
  type AgentFoundCount,
  type ScanHistoryProgress,
  type ScanStatus,
} from "../../../lib/ipc"
import { getMainWindowVisible, onMainWindowVisibilityChanged } from "../../../lib/mainWindowIpc"
import { agentDisplayName } from "../../../lib/presentation/agents"
import { checkRows, checkLabel, type CheckRow } from "../../../lib/presentation/checkRows"
import {
  checkStatusFacts,
  type CheckStatusFacts,
} from "../../../lib/presentation/checkRowStatus"
import { scanStatusStore } from "../../../lib/scanStatusStore"
import type { DeferredPermissionDir } from "../../../lib/types/repository"
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
  /** Sessions read in the current window. */
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
  | "needsFix"
  | "awaitingVerification"
  | "passing"
  | "checking"
  | "error"
  | "waiting"
  | "notChecked"
  | "snoozed"

export interface FixCategory extends CheckStatusFacts {
  id: CheckId
  label: string
  status: FixStatus
  /** Hundredths of one percent; null when the check has no estimate. */
  estimatedBurnBasisPoints: number | null
  /** Assessed sessions with and without a finding. */
  finding: number
  clean: number
  /** Discovery slugs of the agents whose sessions the check read. */
  agents: string[]
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

export interface OverviewProgress {
  agents: AgentsStep
  sessions: SessionsStep
  checks: ChecksStep
  /** Every category in the checks report, for the persistent checklist. */
  categories: FixCategory[]
  failingCount: number
  history: HistoryProgress | null
}

export interface ProgressInputs {
  scanStatus: ScanStatus | null
  checksReport: ChecksReportPayload | null
  checksReportCurrent: boolean
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

function toFixCategory(category: CheckRow): FixCategory {
  const facts = checkStatusFacts(category)
  const status: FixStatus =
    facts.lifecycle === "failing"
      ? "needsFix"
      : facts.lifecycle === "awaitingVerification"
        ? "awaitingVerification"
        : facts.lifecycle === "passing"
          ? "passing"
          : facts.checking
            ? "checking"
            : (facts.reviewFailed ?? 0) > 0
              ? "error"
              : (facts.waitingForDataCount ?? 0) > 0 || (facts.waitingForResultsCount ?? 0) > 0
                ? "waiting"
                : "notChecked"
  return {
    ...facts,
    id: category.id,
    label: checkLabel(category),
    status,
    estimatedBurnBasisPoints: category.estimatedTokenBurnBasisPoints ?? null,
    agents: category.agents ?? [],
  }
}

function deriveHistory(history: ScanHistoryProgress | undefined): HistoryProgress | null {
  if (!history || history.state === "none") return null
  if (history.state === "pending") return { state: "pending", completed: 0, total: 0 }
  if (history.state === "running") {
    return history.total === 0
      ? { state: "looking", completed: 0, total: 0 }
      : { state: "reading", completed: history.completed, total: history.total }
  }
  return { state: "done", completed: history.completed, total: history.total }
}

export function deriveOverviewProgress(
  inputs: ProgressInputs,
  lastPass: LastPass,
): OverviewProgress {
  const agentsRows = lastPass.lastFound ?? inputs.scanStatus?.foundByAgent ?? []
  const agents: AgentsStep = {
    done: agentsRows.length > 0 && agentsRows.every((row) => row.done),
    rows: agentsRows.map((row) => ({ ...row, label: agentDisplayName(row.agent) })),
  }
  const sessionsSource = lastPass.lastRead ??
    inputs.scanStatus?.read ?? { completed: 0, total: 0 }
  const history = deriveHistory(inputs.scanStatus?.history)
  const historyToCount =
    history && (history.state === "reading" || history.state === "done") ? history : null
  const sessions: SessionsStep = {
    done: lastPass.lastRead != null,
    ...sessionsSource,
    displayCompleted: sessionsSource.completed + (historyToCount?.completed ?? 0),
    displayTotal: sessionsSource.total + (historyToCount?.total ?? 0),
    deferred: inputs.deferred,
  }
  const checks: ChecksStep = {
    done: inputs.checksReportCurrent && (inputs.checksReport?.evidenceSettled ?? false),
    windowSessions: inputs.checksReport?.windowSessions ?? 0,
    pendingEvidence: inputs.checksReport?.pendingEvidence ?? 0,
    deferredEvidence: inputs.checksReport?.deferredEvidence ?? 0,
  }
  const categories = inputs.checksReport
    ? checkRows(inputs.checksReport).map(toFixCategory)
    : []
  const failingCount = categories.filter((category) => category.status === "needsFix").length

  return {
    agents,
    sessions,
    checks,
    categories,
    failingCount,
    history,
  }
}

let liveScanStatus: ScanStatus | null = null
let liveChecksReport: ChecksReportPayload | null = null
let liveDeferred: DeferredPermissionDir[] = []
let liveChecksReportCurrent = false
let lastPass: LastPass = INITIAL_LAST_PASS
// Counts scan status updates that show a running pass. A checks report
// request records this count, so a pass that starts before the report
// arrives makes that report not current.
let scanRunsSeen = 0
let requestChecks: (() => void) | null = null
let refreshFolderPermissions: (() => void) | null = null
function currentInputs(): ProgressInputs {
  return {
    scanStatus: liveScanStatus,
    checksReport: liveChecksReport,
    deferred: liveDeferred,
    checksReportCurrent: liveChecksReportCurrent,
  }
}

let snapshot: OverviewProgress = deriveOverviewProgress(currentInputs(), lastPass)
const listeners = new Set<() => void>()

function recompute(): void {
  snapshot = deriveOverviewProgress(currentInputs(), lastPass)
  for (const listener of listeners) listener()
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
  recompute()
}

function onChecksReport(report: ChecksReportPayload | null, current: boolean): void {
  liveChecksReport = report
  liveChecksReportCurrent = current
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
