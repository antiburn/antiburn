import type {
  BurnCheckDetectorId,
  ChecksCategoryPayload,
  ChecksReportPayload,
} from "../insightsIpc"
import {
  aggregateBurnCheckPresentation,
  checkHasProvisionalResult,
  type BurnCheckPresentation,
} from "./checkStatus"
import { activeChecksReport } from "../snoozedBurnChecks"
import { smartCheckReviewPresentation } from "./smartCheckReview"

export { CHECK_LABELS } from "./checkDefinitions"

interface ChecksEstimate {
  tokenBurnBasisPoints: number | null
}

export interface ChecksPresentation {
  activeAssessed: ChecksCategoryPayload[]
  activeUnavailable: ChecksCategoryPayload[]
  snoozed: ChecksCategoryPayload[]
  failures: ChecksCategoryPayload[]
  awaiting?: ChecksCategoryPayload[]
  wins: ChecksCategoryPayload[]
  unavailable: ChecksCategoryPayload[]
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

function estimateOrder(category: ChecksCategoryPayload): number {
  return category.estimatedTokenBurnBasisPoints ?? -1
}

export function checksPresentation(
  report: ChecksReportPayload,
  refreshUnavailable = false,
  snoozed: ReadonlySet<BurnCheckDetectorId> = new Set(),
): ChecksPresentation {
  const activeReport = activeChecksReport(report, snoozed)
  const activeAssessed = activeReport.categories.filter(
    (category) =>
      category.lifecycle != null &&
      smartCheckReviewPresentation(category).terminalLabel == null,
  )
  const activeUnavailable = activeReport.categories.filter(
    (category) =>
      (category.lifecycle == null ||
        smartCheckReviewPresentation(category).terminalLabel != null) &&
      !checkHasProvisionalResult(category),
  )
  const snoozedCategories = report.categories.filter((category) => snoozed.has(category.id))
  const noEnabledChecks = report.evidenceSettled && report.categories.length === 0
  const noActiveChecks = report.evidenceSettled && activeReport.categories.length === 0
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
    wins: activeReport.categories.filter(
      (category) =>
        smartCheckReviewPresentation(category).terminalLabel == null &&
        (category.lifecycle === "passing" || checkHasProvisionalResult(category)),
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
      tokenBurnBasisPoints: activeReport.estimatedTokenBurnBasisPoints,
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
    const failed = `${failureCount} check${failureCount === 1 ? "" : "s"} failed`
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
      result: "Awaiting verification",
      summary: `${awaitingCount} check${awaitingCount === 1 ? "" : "s"} awaiting verification`,
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
      ? `${passed.length} check${passed.length === 1 ? "" : "s"} passed`
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
