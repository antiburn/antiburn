// The Overview's first-run store. Subscribes to the scan status store, the
// checks report, and settings — the external-system boundary — so no
// component needs an effect. Replaces the fake-timer prototype
// (`ftuePrototype.ts`) with the real scan and check pipeline.

import {
  cancelChecksReport,
  getChecksReport,
  onChecksReportChanged,
  type BurnCheckDetectorId,
  type ChecksCategoryPayload,
  type ChecksReportPayload,
} from "../../../lib/insightsIpc"
import {
  getScanStatus,
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

/* -------------------------------------------------------------------------
 * Snapshot shape the Overview renders.
 * ---------------------------------------------------------------------- */

export interface FtueFindRow {
  /** Display label, already mapped from the discovery slug. */
  agent: string
  sessions: number
}

export interface FtueFindStep {
  done: boolean
  rows: FtueFindRow[]
}

export interface FtueReadStep {
  done: boolean
  completed: number
  total: number
  /** The gate outcome, once the read stage this session has finished once. */
  gate: ReadGateCounts | null
  includeNonRepoFolders: boolean
}

export interface FtueCheckStep {
  done: boolean
  windowSessions: number
  pendingEvidence: number
}

export type FtueFixStatus = "needsFix" | "awaitingVerification" | "passing" | "notChecked"

export interface FtueFixCategory {
  id: BurnCheckDetectorId
  label: string
  status: FtueFixStatus
}

export interface FtueHistoryProgress {
  completed: number
  total: number
}

export interface FtueSnapshot {
  /** True while the first-run steps block shows this session. */
  showSteps: boolean
  find: FtueFindStep
  read: FtueReadStep
  check: FtueCheckStep
  /** Every category in the checks report, for the persistent checklist. */
  categories: FtueFixCategory[]
  failingCount: number
  /** The planned background history pass's progress. Null unless the
   *  backend reports it and it is still under way. */
  history: FtueHistoryProgress | null
  dismissed: boolean
}

/* -------------------------------------------------------------------------
 * Pure derivation. Exported so a test can drive it without any IPC mocking.
 * ---------------------------------------------------------------------- */

export interface FtueInputs {
  scanStatus: ScanStatus | null
  checksReport: ChecksReportPayload | null
  includeNonRepoFolders: boolean
  /**
   * Whether the persisted `scan_state` table has ever recorded a completed
   * pass for any agent — the durable "this device is not a first run" signal
   * (`get_scan_status` fills `agents` from that table; a pushed `scan:*`
   * event does not, so this must come from a direct `getScanStatus()` read,
   * never from an event payload). Null until that read resolves.
   */
  hasScanHistory: boolean | null
}

export interface FtueLatch {
  /** Whether the "show steps this session" question has been answered. */
  decided: boolean
  showSteps: boolean
  step1Done: boolean
  step1Rows: AgentFoundCount[]
  step2Done: boolean
  step2Read: { completed: number; total: number }
  step2Gate: ReadGateCounts | null
}

export const INITIAL_FTUE_LATCH: FtueLatch = {
  decided: false,
  showSteps: true,
  step1Done: false,
  step1Rows: [],
  step2Done: false,
  step2Read: { completed: 0, total: 0 },
  step2Gate: null,
}

/**
 * Advance the latch from one set of inputs.
 *
 * Decides "show the steps block" once, the first time both the checks report
 * and {@link FtueInputs.hasScanHistory} are known: true when the checks
 * report is not settled, or the device has no persisted scan history. The
 * answer then holds for the rest of the session (see {@link resetFtueLatch}
 * for `ftue:reset`).
 *
 * `hasScanHistory` — not `ScanStatus.finishedAt` — is the signal, because
 * `finished_at` lives only in the in-memory `ScanController` and is cleared
 * every time a pass starts (`scan/mod.rs`): every launch runs a full pass, so
 * an Overview that reads status during that ~3 s window would otherwise
 * misread an ordinary launch as a first run. The persisted `scan_state` table
 * survives across launches — cleared only by the index wipe — so it tells
 * "never scanned before" from "scanning again" correctly. One accepted
 * consequence: a revision-bump re-ingest marks evidence unsettled again,
 * which brings the steps block back after such an upgrade even though
 * `hasScanHistory` stays true. That is fine for now.
 *
 * Steps 1 and 2 each latch their own numbers the first time they finish, so
 * a later routine pass — every 5 minutes, and every launch, per the scan
 * design — does not reset a checklist the reader already saw. Step 3 has no
 * such reset problem (the checks report does not replay a scan from zero),
 * so its numbers stay live.
 */
export function advanceFtueLatch(latch: FtueLatch, inputs: FtueInputs): FtueLatch {
  let next = latch
  if (!next.decided && inputs.checksReport && inputs.hasScanHistory != null) {
    next = {
      ...next,
      decided: true,
      showSteps: !inputs.checksReport.evidenceSettled || !inputs.hasScanHistory,
    }
  }
  const phase = inputs.scanStatus?.phase
  if (!next.step1Done && phase != null && phase !== "finding" && phase !== "idle") {
    next = { ...next, step1Done: true, step1Rows: inputs.scanStatus?.foundByAgent ?? [] }
  }
  if (!next.step2Done && phase === "saving" && inputs.scanStatus?.gate) {
    next = {
      ...next,
      step2Done: true,
      step2Read: inputs.scanStatus.read,
      step2Gate: inputs.scanStatus.gate,
    }
  }
  return next
}

/** `ftue:reset` wipes the real data, so the pass it triggers must be tracked
 *  from the start. Brings the steps block back at once, rather than waiting
 *  to re-decide against whatever stale status is still on hand. */
export function resetFtueLatch(): FtueLatch {
  return { ...INITIAL_FTUE_LATCH, decided: true }
}

/**
 * Un-latch step 2 alone, so the pass a reader's own "Include them" click
 * triggers replaces the read outcome they just asked to change — unlike a
 * routine tick, this pass has a reader waiting to see its result. Step 1 is
 * untouched: discovery does not depend on this setting.
 */
export function unlatchReadOutcome(latch: FtueLatch): FtueLatch {
  return { ...latch, step2Done: false, step2Gate: null }
}

function toFixCategory(category: ChecksCategoryPayload): FtueFixCategory {
  const status: FtueFixStatus =
    category.lifecycle === "failing"
      ? "needsFix"
      : category.lifecycle === "awaitingVerification"
        ? "awaitingVerification"
        : category.lifecycle === "passing"
          ? "passing"
          : "notChecked"
  return { id: category.id, label: CHECK_LABELS[category.id], status }
}

/**
 * Whether the persisted `scan_state` table has ever recorded a completed
 * pass for any agent — see {@link FtueInputs.hasScanHistory}. Null when
 * `status` itself is unknown (no shell, or the read has not resolved yet).
 */
export function hasScanHistory(status: ScanStatus | null): boolean | null {
  if (!status) return null
  return status.agents.some((agent) => agent.lastCompletedAt != null)
}

function deriveHistory(history: ScanHistoryProgress | undefined): FtueHistoryProgress | null {
  if (!history) return null
  if (history.state !== "pending" && history.state !== "running") return null
  return { completed: history.completed, total: history.total }
}

export function deriveFtueSnapshot(
  latch: FtueLatch,
  inputs: FtueInputs,
  dismissed: boolean,
): FtueSnapshot {
  const find: FtueFindStep = {
    done: latch.step1Done,
    rows: (latch.step1Done ? latch.step1Rows : (inputs.scanStatus?.foundByAgent ?? [])).map(
      (row) => ({ agent: agentDisplayName(row.agent), sessions: row.sessions }),
    ),
  }
  const read: FtueReadStep = {
    done: latch.step2Done,
    completed: latch.step2Done
      ? latch.step2Read.completed
      : (inputs.scanStatus?.read.completed ?? 0),
    total: latch.step2Done ? latch.step2Read.total : (inputs.scanStatus?.read.total ?? 0),
    gate: latch.step2Done ? latch.step2Gate : null,
    includeNonRepoFolders: inputs.includeNonRepoFolders,
  }
  const check: FtueCheckStep = {
    done: inputs.checksReport?.evidenceSettled ?? false,
    windowSessions: inputs.checksReport?.windowSessions ?? 0,
    pendingEvidence: inputs.checksReport?.pendingEvidence ?? 0,
  }
  const categories = (inputs.checksReport?.categories ?? []).map(toFixCategory)
  const failingCount = categories.filter((category) => category.status === "needsFix").length
  return {
    showSteps: latch.showSteps,
    find,
    read,
    check,
    categories,
    failingCount,
    history: deriveHistory(inputs.scanStatus?.history),
    dismissed,
  }
}

/* -------------------------------------------------------------------------
 * The live store: latch state plus the IPC boundary that feeds it.
 * ---------------------------------------------------------------------- */

let latch: FtueLatch = INITIAL_FTUE_LATCH
let liveScanStatus: ScanStatus | null = null
let liveChecksReport: ChecksReportPayload | null = null
let liveIncludeNonRepoFolders = false
let liveHasScanHistory: boolean | null = null
let dismissed = false

function currentInputs(): FtueInputs {
  return {
    scanStatus: liveScanStatus,
    checksReport: liveChecksReport,
    includeNonRepoFolders: liveIncludeNonRepoFolders,
    hasScanHistory: liveHasScanHistory,
  }
}

let snapshot: FtueSnapshot = deriveFtueSnapshot(latch, currentInputs(), dismissed)
const listeners = new Set<() => void>()

function recompute(): void {
  snapshot = deriveFtueSnapshot(latch, currentInputs(), dismissed)
  for (const listener of listeners) listener()
}

function onScanStatus(status: ScanStatus | null): void {
  liveScanStatus = status
  latch = advanceFtueLatch(latch, currentInputs())
  recompute()
}

function onChecksReport(report: ChecksReportPayload | null): void {
  liveChecksReport = report
  latch = advanceFtueLatch(latch, currentInputs())
  recompute()
}

function onSettings(settings: AppSettings): void {
  liveIncludeNonRepoFolders = settings.includeNonRepoFolders
  recompute()
}

function onReset(): void {
  latch = resetFtueLatch()
  dismissed = false
  // The wipe clears `scan_state`, so this device has no scan history again
  // until the pass the reset triggers completes and re-populates it.
  liveHasScanHistory = false
  recompute()
}

export function dismissFtueCallout(): void {
  dismissed = true
  recompute()
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

async function attach(thisGeneration: number, pending: Promise<() => void>): Promise<void> {
  const stop = await pending.catch(() => null)
  if (!stop) return
  if (thisGeneration !== generation) stop()
  else stops.push(stop)
}

async function start(): Promise<void> {
  const thisGeneration = ++generation
  checksConsumerId = `overview-ftue-${++nextConsumer}`
  const consumerId = checksConsumerId

  function refreshChecks(): void {
    void getChecksReport(consumerId)
      .then((report) => {
        if (thisGeneration === generation) onChecksReport(report)
      })
      .catch(() => undefined)
  }

  // The durable "ever scanned before" signal, read directly rather than from
  // a push event — see {@link FtueInputs.hasScanHistory}. Only needs to
  // resolve once: the latch decides at most once per store generation.
  function refreshScanHistory(): void {
    void getScanStatus()
      .then((status) => {
        if (thisGeneration !== generation) return
        liveHasScanHistory = hasScanHistory(status)
        latch = advanceFtueLatch(latch, currentInputs())
        recompute()
      })
      .catch(() => undefined)
  }

  await Promise.all([
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
        if (thisGeneration === generation) onSettings(settings)
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
  refreshChecks()
  refreshScanHistory()
  void getSettings().then((settings) => {
    if (thisGeneration === generation) onSettings(settings)
  })
}

function stop(): void {
  generation += 1
  for (const detach of stops.splice(0)) detach()
  if (checksConsumerId) void cancelChecksReport(checksConsumerId).catch(() => undefined)
  checksConsumerId = null
}

export function subscribeFtue(listener: () => void): () => void {
  listeners.add(listener)
  if (listeners.size === 1) void start()
  return () => {
    listeners.delete(listener)
    if (listeners.size === 0) stop()
  }
}

export function ftueSnapshot(): FtueSnapshot {
  return snapshot
}
