import { pluralize } from "./text"
import type { CheckId, ChecksReportPayload } from "../insightsIpc"
import {
  aggregateBurnCheckPresentation,
  checkHasProvisionalResult,
  type BurnCheckPresentation,
} from "./checkStatus"
import { activeChecksReport } from "../snoozedBurnChecks"
import { smartCheckReviewPresentation } from "./smartCheckReview"
import { checkRows, isInstalledCheck, type CheckRow } from "./checkRows"
import { checkStatusFacts } from "./checkRowStatus"

export { CHECK_LABELS } from "./checkDefinitions"

interface ChecksEstimate {
  tokenBurnBasisPoints: number | null
}

export interface ChecksPresentation {
  activeAssessed: CheckRow[]
  activeUnavailable: CheckRow[]
  snoozed: CheckRow[]
  failures: CheckRow[]
  awaiting?: CheckRow[]
  wins: CheckRow[]
  checking: CheckRow[]
  unavailable: CheckRow[]
  refreshUnavailable: boolean
  noActiveChecks?: boolean
  noEnabledChecks?: boolean
  burnChecks: BurnCheckPresentation
  estimate: ChecksEstimate
}

export interface ChecksHeroPresentation {
  result: string
  summary: string | null
  state: "failed" | "passed" | "pending"
  tone: string
}

function estimateOrder(category: CheckRow): number {
  return category.estimatedTokenBurnBasisPoints ?? -1
}

export function checksPresentation(
  report: ChecksReportPayload,
  refreshUnavailable = false,
  snoozed: ReadonlySet<CheckId> = new Set(),
): ChecksPresentation {
  const activeReport = activeChecksReport(report, snoozed)
  const activeRows = checkRows(activeReport)
  const activeAssessed = activeRows.filter(
    (category) =>
      checkStatusFacts(category).lifecycle != null &&
      smartCheckReviewPresentation(category).terminalLabel == null,
  )
  const activeUnavailable = activeRows.filter(
    (category) =>
      (checkStatusFacts(category).lifecycle == null ||
        smartCheckReviewPresentation(category).terminalLabel != null) &&
      (!checkStatusFacts(category).checking ||
        (isInstalledCheck(category) && checkStatusFacts(category).sourceBlocked)),
  )
  const snoozedCategories = checkRows(report).filter((category) => snoozed.has(category.id))
  const noEnabledChecks = report.evidenceSettled && checkRows(report).length === 0
  const noActiveChecks = report.evidenceSettled && activeRows.length === 0
  const burnChecks = aggregateBurnCheckPresentation(activeReport, refreshUnavailable)
  return {
    activeAssessed,
    activeUnavailable,
    snoozed: snoozedCategories,
    failures: activeAssessed
      .filter((category) => category.lifecycle === "failing")
      .sort((left, right) => estimateOrder(right) - estimateOrder(left)),
    awaiting: activeAssessed.filter(
      (category) => category.lifecycle === "awaitingVerification",
    ),
    wins: activeRows.filter(
      (category) =>
        smartCheckReviewPresentation(category).terminalLabel == null &&
        checkStatusFacts(category).lifecycle === "passing" &&
        !checkHasProvisionalResult(category),
    ),
    checking: activeRows.filter(
      (category) =>
        !isInstalledCheck(category) &&
        checkStatusFacts(category).checking &&
        category.finding === 0 &&
        category.clean === 0 &&
        category.lifecycle !== "awaitingVerification" &&
        category.lifecycle !== "failing" &&
        smartCheckReviewPresentation(category).terminalLabel == null,
    ),
    unavailable: activeUnavailable,
    refreshUnavailable,
    noActiveChecks,
    noEnabledChecks,
    burnChecks: noActiveChecks
      ? {
          ...burnChecks,
          headline: noEnabledChecks ? "No checks enabled" : "No active checks",
          accessibleDescription: noEnabledChecks
            ? "No Burn Checks enabled."
            : "No active Burn Checks.",
        }
      : burnChecks,
    estimate: {
      tokenBurnBasisPoints: activeRows.some(
        (check) => isInstalledCheck(check) && check.lifecycle === "failing",
      )
        ? null
        : activeReport.estimatedTokenBurnBasisPoints,
    },
  }
}

export function checksHeroPresentation(
  presentation: ChecksPresentation,
): ChecksHeroPresentation {
  if (presentation.noEnabledChecks) {
    return { result: "No checks enabled", summary: null, state: "pending", tone: "text-label" }
  }
  if (presentation.noActiveChecks) {
    return { result: "No active checks", summary: null, state: "pending", tone: "text-label" }
  }
  const failureCount = presentation.failures.length
  if (failureCount > 0) {
    const failed = `${failureCount} ${pluralize(failureCount, "check", "checks")} failed`
    const basisPoints = presentation.estimate.tokenBurnBasisPoints
    return {
      result:
        basisPoints == null
          ? failed
          : `${formatTokenBurnPercent(basisPoints)} estimated token burn`,
      summary: basisPoints == null ? null : failed,
      state: "failed",
      tone: basisPoints == null ? "text-system-red-text" : tokenBurnTone(basisPoints),
    }
  }

  const awaitingCount = presentation.awaiting?.length ?? 0
  if (awaitingCount > 0) {
    return {
      result: "Verifying",
      summary: `Verifying ${awaitingCount} ${pluralize(awaitingCount, "check", "checks")}`,
      state: "pending",
      tone: "text-label",
    }
  }

  const passed = presentation.wins.filter(
    (category) => category.lifecycle === "passing" && category.clean > 0,
  )
  const completePass = passed.length > 0
  return {
    result: completePass ? "No issues found" : "No checks assessed",
    summary: completePass
      ? `${passed.length} ${pluralize(passed.length, "check", "checks")} passed`
      : null,
    state: completePass ? "passed" : "pending",
    tone: "text-label",
  }
}

export function formatTokenBurnPercent(basisPoints: number): string {
  if (basisPoints > 0 && basisPoints < 100) return "<1%"
  return `${Math.floor(basisPoints / 100)}%`
}

export function tokenBurnTone(basisPoints: number): string {
  if (basisPoints === 0) return "text-system-green"
  return basisPoints < 500 ? "text-system-yellow" : "text-system-red-text"
}

/** Formats an API-equivalent dollar figure as `~$X.XX`, matching `costTotal`
 * in `BurnChecksSavings.tsx`. */
export function formatApiEquivalentUsd(value: number): string {
  return `${value < 0 ? "-" : ""}~$${Math.abs(value).toFixed(2)}`
}
