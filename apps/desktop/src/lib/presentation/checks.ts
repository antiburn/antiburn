import type { ChecksCategoryPayload, ChecksReportPayload } from "../insightsIpc"

export const CHECK_LABELS: Record<string, string> = {
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
  failures: ChecksCategoryPayload[]
  wins: ChecksCategoryPayload[]
  unavailable: ChecksCategoryPayload[]
  refreshUnavailable: boolean
  estimate: ChecksEstimate
}

function estimateOrder(category: ChecksCategoryPayload): number {
  return category.estimatedTokenBurnBasisPoints ?? -1
}

export function checksPresentation(
  report: ChecksReportPayload,
  refreshUnavailable = false,
): ChecksPresentation {
  return {
    failures: report.categories
      .filter((category) => category.finding > 0)
      .sort((left, right) => estimateOrder(right) - estimateOrder(left)),
    wins: report.categories.filter((category) => category.finding === 0 && category.clean > 0),
    unavailable: report.categories.filter(
      (category) => category.finding === 0 && category.clean === 0,
    ),
    refreshUnavailable,
    estimate: {
      tokenBurnBasisPoints: report.estimatedTokenBurnBasisPoints,
    },
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
