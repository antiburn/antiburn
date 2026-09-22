import type {
  BurnCheckDetectorId,
  ChecksCategoryPayload,
  ChecksReportPayload,
} from "../insightsIpc"
import { aggregateBurnCheckPresentation, type BurnCheckPresentation } from "./burnChecks"
import { activeChecksReport } from "../snoozedBurnChecks"

export const CHECK_LABELS: Record<BurnCheckDetectorId, string> = {
  sessionsOverDepth: "Session overdepth",
  modelOverthinking: "Model overthinking",
  overpoweredSubagents: "Overpowered subagents",
  unusedMcpServers: "Unused MCP servers",
  unusedBuiltInTools: "Unused built-in tools",
  unusedSkills: "Unused skills",
  oldModelUsage: "Old model usage",
  overuseOfFastMode: "Fast mode overuse",
  cacheChurn: "Excess cache rehydration",
}

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
    (category) => category.lifecycle != null,
  )
  const activeUnavailable = activeReport.categories.filter(
    (category) => category.lifecycle == null,
  )
  const snoozedCategories = report.categories.filter((category) => snoozed.has(category.id))
  const noActiveChecks = report.evidenceSettled && activeAssessed.length === 0
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
    wins: activeAssessed.filter((category) => category.lifecycle === "passing"),
    unavailable: activeUnavailable,
    refreshUnavailable,
    noActiveChecks,
    burnChecks: noActiveChecks
      ? {
          ...burnChecks,
          headline: "No active checks",
          accessibleDescription: "No active Burn Checks.",
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

  const completePass = presentation.wins.length > 0
  return {
    result: completePass ? "No issues found" : "No checks assessed",
    summary: completePass
      ? `${presentation.wins.length} check${presentation.wins.length === 1 ? "" : "s"} passed`
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
