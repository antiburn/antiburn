// The Overview's first-run latch. Decides, once per session, whether to show
// the find/read/check steps block, and remembers each step's own numbers so
// a later routine pass does not reset a checklist the reader already saw.
// Pure code, with no module state, so a test drives it without any IPC
// mocking.

import type { ChecksReportPayload } from "../../../lib/insightsIpc"
import type { AgentFoundCount, ReadGateCounts, ScanStatus } from "../../../lib/ipc"

/** The wait after the last step finishes, before the first step moves down. */
export const DOCK_START_DELAY_MS = 700
/** The pause between two steps that move down. */
export const DOCK_STEP_PAUSE_MS = 450

export interface FirstRunInputs {
  scanStatus: ScanStatus | null
  checksReport: ChecksReportPayload | null
  /**
   * Whether the persisted `scan_state` table has ever recorded a completed
   * pass for any agent — the durable "this device is not a first run" signal
   * (`get_scan_status` fills `agents` from that table; a pushed `scan:*`
   * event does not, so this must come from a direct `getScanStatus()` read,
   * never from an event payload). Null until that read resolves.
   */
  hasScanHistory: boolean | null
  /**
   * Whether `checksReport` was requested after the last scan pass finished.
   * A report from before that point can miss the sessions the pass saved,
   * so step 3 does not latch on it.
   */
  checksReportCurrent: boolean
}

export interface FirstRunLatch {
  /** Whether the "show steps this session" question has been answered. */
  decided: boolean
  showSteps: boolean
  step1Done: boolean
  step1Rows: AgentFoundCount[]
  step2Done: boolean
  step2Read: { completed: number; total: number }
  step2Gate: ReadGateCounts | null
  step3Done: boolean
  step3Check: { windowSessions: number; deferredEvidence: number }
}

export const INITIAL_FIRST_RUN_LATCH: FirstRunLatch = {
  decided: false,
  // Not a guess: showing first-run UI before the device's scan history is
  // known would be as wrong as hiding it would be. Stays hidden until
  // {@link advanceFirstRunLatch} decides, or {@link resetFirstRunLatch} forces
  // it on.
  showSteps: false,
  step1Done: false,
  step1Rows: [],
  step2Done: false,
  step2Read: { completed: 0, total: 0 },
  step2Gate: null,
  step3Done: false,
  step3Check: { windowSessions: 0, deferredEvidence: 0 },
}

/**
 * Advance the latch from one set of inputs.
 *
 * Decides "show the steps block" once, the first time
 * {@link FirstRunInputs.hasScanHistory} is known: true when the device has no
 * persisted scan history, false otherwise. The answer then holds for the
 * rest of the session (see {@link resetFirstRunLatch} for `ftue:reset`).
 *
 * `hasScanHistory` — not `ScanStatus.finishedAt` and not the checks report's
 * `evidenceSettled` — is the only signal, because both of those are
 * ordinarily unsettled for a few seconds after every launch: `finished_at`
 * lives only in the in-memory `ScanController` and is cleared every time a
 * pass starts (`scan/mod.rs`), and `evidenceSettled` goes false while the
 * evidence worker catches up with whatever a live agent session wrote since
 * the last launch. An Overview that read either signal during that window
 * would misread an ordinary launch as a first run and show the steps block
 * every time. The persisted `scan_state` table survives across launches —
 * cleared only by the index wipe — so it tells "never scanned before" from
 * "scanning again" correctly. One accepted consequence: a revision-bump
 * re-ingest does not bring the steps block back, even though it marks
 * evidence unsettled again — intended, since the device has scanned before.
 *
 * Steps 1 and 2 each latch their own numbers the first time they finish, so
 * a later routine pass — every 5 minutes, and every launch, per the scan
 * design — does not reset a checklist the reader already saw.
 *
 * Step 3 latches when the worker has no work it can claim: every pending
 * session is deferred by a retry backoff. A live session changes during its
 * check, backs off, and goes pending again after each turn, so neither
 * `evidenceSettled` nor a live pending count can tell when the first check
 * is done. In the steps block, step 3 also waits for step 2 and for a report
 * requested after the pass finished, so a report from before the pass saved
 * its sessions cannot latch an empty result.
 */
export function advanceFirstRunLatch(
  latch: FirstRunLatch,
  inputs: FirstRunInputs,
): FirstRunLatch {
  let next = latch
  if (!next.decided && inputs.hasScanHistory != null) {
    next = {
      ...next,
      decided: true,
      showSteps: !inputs.hasScanHistory,
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
  const report = inputs.checksReport
  const readSettled = !next.showSteps || (next.step2Done && inputs.checksReportCurrent)
  if (
    !next.step3Done &&
    next.decided &&
    readSettled &&
    report != null &&
    report.pendingEvidence <= report.deferredEvidence
  ) {
    next = {
      ...next,
      step3Done: true,
      step3Check: {
        windowSessions: report.windowSessions,
        deferredEvidence: report.deferredEvidence,
      },
    }
  }
  return next
}

/** `ftue:reset` wipes the real data, so the pass it triggers must be tracked
 *  from the start. Brings the steps block back at once, rather than waiting
 *  to re-decide against whatever stale status is still on hand. */
export function resetFirstRunLatch(): FirstRunLatch {
  return { ...INITIAL_FIRST_RUN_LATCH, decided: true, showSteps: true }
}

/**
 * Un-latch step 2, so the pass a reader's own "Include them" click
 * triggers replaces the read outcome they just asked to change — unlike a
 * routine tick, this pass has a reader waiting to see its result. Step 3
 * un-latches too, because the pass adds sessions to check. Step 1 is
 * untouched: discovery does not depend on this setting.
 */
export function unlatchReadOutcome(latch: FirstRunLatch): FirstRunLatch {
  return {
    ...latch,
    step2Done: false,
    step2Gate: null,
    step3Done: false,
    step3Check: INITIAL_FIRST_RUN_LATCH.step3Check,
  }
}

/**
 * Whether the persisted `scan_state` table has ever recorded a completed
 * pass for any agent — see {@link FirstRunInputs.hasScanHistory}. Null when
 * `status` itself is unknown (no shell, or the read has not resolved yet).
 */
export function hasScanHistory(status: ScanStatus | null): boolean | null {
  if (!status) return null
  return status.agents.some((agent) => agent.lastCompletedAt != null)
}
