import type {
  ChecksCategoryLifecycle,
  InstalledCheckAvailability,
  InstalledCheckReportPayload,
} from "../insightsIpc"
import { isInstalledCheck, type CheckRow } from "./checkRows"
import { smartCheckReviewPresentation } from "./smartCheckReview"
import {
  installedCheckErrorStatus,
  installedCheckSourceBlocked,
} from "./installedCheckDiagnostics"

type StatusKind = "failed" | "passed" | "checking" | "waiting" | "neutral"

interface CheckResultStatus {
  parts: Array<{ kind: StatusKind; text: string }>
  text: string
}

export interface CheckStatusFacts {
  lifecycle?: ChecksCategoryLifecycle | null
  finding: number
  clean: number
  hasStarted?: boolean
  checking?: boolean
  checkingCount?: number
  reviewFailed?: number
  reviewedWithoutStatus?: number
  waitingForDataCount?: number
  waitingForResultsCount?: number
  waitingForEvents?: InstalledCheckReportPayload["waitingForEvents"]
  installedAvailability?: InstalledCheckAvailability
  diagnosticStatus?: string | null
  sourceBlocked?: boolean
  terminalLabel?: string | null
}

function installedCheckStatus(facts: CheckStatusFacts, snoozed: boolean): CheckResultStatus {
  const single = (kind: StatusKind, text: string): CheckResultStatus => ({
    parts: [{ kind, text }],
    text,
  })
  if (snoozed) return single("neutral", "Snoozed")
  switch (facts.installedAvailability?.status) {
    case "paused":
      return single("neutral", facts.diagnosticStatus ?? "Paused")
    case "invalid":
      return single("neutral", facts.diagnosticStatus ?? "Check needs fixing")
    case "unsupported":
      return single("neutral", facts.diagnosticStatus ?? "Not supported")
  }
  const parts: CheckResultStatus["parts"] = []
  if (facts.finding > 0) parts.push({ kind: "failed", text: `${facts.finding} failed` })
  if (
    facts.clean > 0 ||
    (facts.finding === 0 && !facts.sourceBlocked && (facts.hasStarted || facts.checking))
  ) {
    parts.push({
      kind: "passed",
      text: facts.clean > 1 || facts.finding > 0 ? `${facts.clean} passed` : "Passed",
    })
  }
  const errorStatus =
    facts.diagnosticStatus ?? ((facts.reviewFailed ?? 0) > 0 ? "Could not check" : null)
  if (errorStatus) parts.push({ kind: "neutral", text: errorStatus })
  if (parts.length === 0) parts.push({ kind: "neutral", text: "Not assessed" })
  if (facts.checking) {
    parts.push({ kind: "checking", text: "Checking" })
  } else if (
    !errorStatus &&
    (facts.waitingForEvents?.catchingUp || (facts.waitingForDataCount ?? 0) > 0)
  ) {
    parts.push({ kind: "waiting", text: "Catching up" })
  } else if (!errorStatus && facts.waitingForEvents && facts.waitingForEvents.count > 0) {
    const { count } = facts.waitingForEvents
    parts.push({
      kind: "waiting",
      text: `Waiting on ${count} ${count === 1 ? "tool result" : "tool results"}`,
    })
  }
  return { parts, text: parts.map((part) => part.text).join(" · ") }
}

export function checkResultStatus(facts: CheckStatusFacts, snoozed = false): CheckResultStatus {
  if (facts.installedAvailability) return installedCheckStatus(facts, snoozed)
  const parts: CheckResultStatus["parts"] = []
  const passed =
    !facts.terminalLabel &&
    facts.lifecycle === "passing" &&
    (facts.clean > 0 || !facts.checking)
  if (!passed && facts.finding > 0)
    parts.push({ kind: "failed", text: `${facts.finding} failed` })
  if (passed || facts.clean > 0)
    parts.push({
      kind: "passed",
      text: !passed && facts.finding > 0 ? `${facts.clean} passed` : "Passed",
    })
  if (facts.lifecycle === "awaitingVerification" && !snoozed)
    parts.push({ kind: "neutral", text: "Verifying" })
  if (facts.checking && !snoozed) {
    const text = facts.checkingCount == null ? "Checking" : `${facts.checkingCount} checking`
    parts.push({ kind: "checking", text })
  }
  if ((facts.reviewFailed ?? 0) > 0)
    parts.push({
      kind: "neutral",
      text: facts.reviewFailed === 1 ? "Could not check" : `${facts.reviewFailed} check errors`,
    })
  if ((facts.reviewedWithoutStatus ?? 0) > 0)
    parts.push({ kind: "neutral", text: "No status change" })
  if (parts.length === 0 && facts.terminalLabel)
    parts.push({ kind: "neutral", text: facts.terminalLabel })
  if (snoozed) parts.push({ kind: "neutral", text: "Snoozed" })
  if (parts.length === 0) parts.push({ kind: "neutral", text: "Not assessed" })
  return { parts, text: parts.map((part) => part.text).join(" · ") }
}

export function checkStatusFacts(check: CheckRow): CheckStatusFacts {
  const review = smartCheckReviewPresentation(check)
  return {
    lifecycle:
      review.terminal && check.finding === 0 && check.clean === 0
        ? null
        : (check.lifecycle ?? null),
    finding: check.finding,
    clean: check.clean,
    ...(check.checkingCount == null ? {} : { checkingCount: check.checkingCount }),
    checking:
      check.checking === true &&
      check.lifecycle !== "awaitingVerification" &&
      check.reviewState == null,
    ...(isInstalledCheck(check)
      ? {
          hasStarted: check.installed.hasStarted,
          reviewFailed: check.reviewFailed,
          reviewedWithoutStatus: check.reviewedWithoutStatus,
          waitingForDataCount: check.installed.waitingForDataCount,
          waitingForResultsCount: check.installed.waitingForResultsCount,
          waitingForEvents: check.installed.waitingForEvents,
          installedAvailability: check.installed.availability,
          diagnosticStatus: installedCheckErrorStatus(check.installed.diagnostics),
          sourceBlocked: installedCheckSourceBlocked(check.installed.diagnostics),
        }
      : {}),
    terminalLabel: review.terminalLabel,
  }
}

export function checkRowStatus(check: CheckRow, snoozed = false) {
  return checkResultStatus(checkStatusFacts(check), snoozed)
}
