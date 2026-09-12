import {
  Bot,
  Brain,
  Database,
  Gauge,
  History,
  Layers3,
  Server,
  Wrench,
  type LucideIcon,
} from "lucide-react"

import type { BurnCheckDetectorId, ChecksCategoryPayload } from "../../lib/insightsIpc"
import {
  CHECK_LABELS,
  formatTokenBurnPercent,
  tokenBurnTone,
} from "../../lib/presentation/checks"

interface CheckUiMetadata {
  icon: LucideIcon
  recommendation: string
}

export const CHECK_UI: Record<BurnCheckDetectorId, CheckUiMetadata> = {
  sessionsOverDepth: {
    icon: Layers3,
    recommendation:
      "Start a new session after completed work or a major scope change to avoid carrying unrelated context.",
  },
  modelOverthinking: {
    icon: Brain,
    recommendation:
      "Use a lower reasoning level by default to reduce unnecessary reasoning, and raise it for difficult work.",
  },
  overpoweredSubagents: {
    icon: Bot,
    recommendation:
      "Assign bounded work to a reviewed lighter worker model to reduce avoidable model cost.",
  },
  unusedMcpServers: {
    icon: Server,
    recommendation: "Disable this server where it is not needed to avoid loading unused tools.",
  },
  unusedBuiltInTools: {
    icon: Wrench,
    recommendation:
      "Load only the built-in tools needed for this work to avoid repeated unused definitions.",
  },
  unusedSkills: {
    icon: Wrench,
    recommendation: "Load this skill only when the request needs it to avoid unused context.",
  },
  oldModelUsage: {
    icon: History,
    recommendation:
      "Use the reviewed replacement for new sessions to support the same work at a lower API-equivalent cost.",
  },
  overuseOfFastMode: {
    icon: Gauge,
    recommendation:
      "Use the standard tier unless the task needs faster results to avoid the fast-tier price premium.",
  },
  cacheChurn: {
    icon: Database,
    recommendation: "Keep stable context reusable to avoid paid cache rehydration.",
  },
}

const CHECK_ICONS: Record<BurnCheckDetectorId, LucideIcon> = {
  sessionsOverDepth: CHECK_UI.sessionsOverDepth.icon,
  modelOverthinking: CHECK_UI.modelOverthinking.icon,
  overpoweredSubagents: CHECK_UI.overpoweredSubagents.icon,
  unusedMcpServers: CHECK_UI.unusedMcpServers.icon,
  unusedBuiltInTools: CHECK_UI.unusedBuiltInTools.icon,
  unusedSkills: CHECK_UI.unusedSkills.icon,
  oldModelUsage: CHECK_UI.oldModelUsage.icon,
  overuseOfFastMode: CHECK_UI.overuseOfFastMode.icon,
  cacheChurn: CHECK_UI.cacheChurn.icon,
}

function failedSessionSummary(category: ChecksCategoryPayload): string {
  const assessed = category.finding + category.clean
  return `${category.finding}/${assessed} session${assessed === 1 ? "" : "s"} failed`
}

function tokenBurnLabel(category: ChecksCategoryPayload): string | null {
  return category.estimatedTokenBurnBasisPoints == null
    ? null
    : `${formatTokenBurnPercent(category.estimatedTokenBurnBasisPoints)} token burn`
}

export function checkRowPresentation(category: ChecksCategoryPayload) {
  const failed = category.finding > 0
  const metric = failed ? tokenBurnLabel(category) : null
  return {
    Icon: CHECK_ICONS[category.id],
    label: CHECK_LABELS[category.id],
    summary: failed ? failedSessionSummary(category) : `${category.clean} passed`,
    metric,
    iconTone: failed
      ? "bg-system-red/10 text-system-red-text"
      : "bg-system-green/10 text-system-green",
    metricTone:
      metric && category.estimatedTokenBurnBasisPoints != null
        ? tokenBurnTone(category.estimatedTokenBurnBasisPoints)
        : null,
  }
}
