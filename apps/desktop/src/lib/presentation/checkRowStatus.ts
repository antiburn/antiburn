import type {
  ChecksCategoryLifecycle,
  InstalledCheckAvailability,
  InstalledCheckReportPayload,
} from "../insightsIpc"
import { isInstalledCheck, type CheckRow } from "./checkRows"

type StatusKind = "failed" | "passed" | "checking" | "neutral"

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
}

function installedCheckStatus(facts: CheckStatusFacts, snoozed: boolean): CheckResultStatus {
  const single = (kind: StatusKind, text: string): CheckResultStatus => ({
    parts: [{ kind, text }],
    text,
  })
  if (snoozed) return single("neutral", "Snoozed")
  const availability = facts.installedAvailability?.status
  const unavailable = availability !== "available"
  const parts: CheckResultStatus["parts"] = []
  if (facts.finding > 0) parts.push({ kind: "failed", text: `${facts.finding} failed` })
  if (facts.clean > 0) {
    parts.push({
      kind: "passed",
      text: facts.clean > 1 || facts.finding > 0 ? `${facts.clean} passed` : "Passed",
    })
  }
  if (parts.length === 0) parts.push({ kind: "neutral", text: "Not assessed" })
  if (!unavailable && facts.checking) {
    parts.push({ kind: "checking", text: "Checking" })
  }
  return { parts, text: parts.map((part) => part.text).join(" · ") }
}

export function checkResultStatus(facts: CheckStatusFacts, snoozed = false): CheckResultStatus {
  if (facts.installedAvailability) return installedCheckStatus(facts, snoozed)
  const parts: CheckResultStatus["parts"] = []
  const passed = facts.lifecycle === "passing"
  if (!passed && facts.finding > 0)
    parts.push({ kind: "failed", text: `${facts.finding} failed` })
  if (passed || facts.clean > 0)
    parts.push({
      kind: "passed",
      text: !passed && facts.finding > 0 ? `${facts.clean} passed` : "Passed",
    })
  if (facts.lifecycle === "awaitingVerification" && !snoozed)
    parts.push({ kind: "neutral", text: "Verifying" })
  if (snoozed) parts.push({ kind: "neutral", text: "Snoozed" })
  if (parts.length === 0) parts.push({ kind: "neutral", text: "Not assessed" })
  return { parts, text: parts.map((part) => part.text).join(" · ") }
}

export function checkStatusFacts(check: CheckRow): CheckStatusFacts {
  return {
    lifecycle: check.lifecycle ?? null,
    finding: check.finding,
    clean: check.clean,
    ...(isInstalledCheck(check)
      ? {
          checkingCount: check.checkingCount,
          checking: check.checking,
          hasStarted: check.installed.hasStarted,
          reviewFailed: check.reviewFailed,
          reviewedWithoutStatus: check.reviewedWithoutStatus,
          waitingForDataCount: check.installed.waitingForDataCount,
          waitingForResultsCount: check.installed.waitingForResultsCount,
          waitingForEvents: check.installed.waitingForEvents,
          installedAvailability: check.installed.availability,
        }
      : {}),
  }
}

export function checkRowStatus(check: CheckRow, snoozed = false) {
  return checkResultStatus(checkStatusFacts(check), snoozed)
}
