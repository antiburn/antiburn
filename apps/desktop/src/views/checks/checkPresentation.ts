import {
  Bot,
  BookOpen,
  Brain,
  Database,
  Gauge,
  History,
  Layers3,
  Server,
  Wrench,
  CircleCheck,
  type LucideIcon,
} from "lucide-react"

import type { BurnCheckDetectorId, BurnCheckTargetPayload } from "../../lib/insightsIpc"
import { isInstalledCheck, checkLabel, type CheckRow } from "../../lib/presentation/checkRows"
import { checkRowStatus, checkStatusFacts } from "../../lib/presentation/checkRowStatus"
import {
  formatApiEquivalentUsd,
  formatTokenBurnPercent,
  tokenBurnTone,
} from "../../lib/presentation/checkReport"

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
      "Give helper models smaller tasks and use a lower-cost model when it can do the work.",
  },
  unusedMcpServers: {
    icon: Server,
    recommendation: "Turn off this tool server where you do not need it.",
  },
  unusedBuiltInTools: {
    icon: Wrench,
    recommendation: "Load only the built-in tools needed for the task.",
  },
  unusedSkills: {
    icon: BookOpen,
    recommendation: "Load this skill only when the request needs it to avoid unused context.",
  },
  oldModelUsage: {
    icon: History,
    recommendation:
      "Use the reviewed replacement for new sessions to do similar work at a lower estimated API cost.",
  },
  overuseOfFastMode: {
    icon: Gauge,
    recommendation: "Use standard mode unless the task needs speed. Fast mode costs more.",
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

function tokenBurnLabel(category: CheckRow): string | null {
  return category.estimatedTokenBurnBasisPoints == null
    ? null
    : `${formatTokenBurnPercent(category.estimatedTokenBurnBasisPoints)} estimated burn`
}

/** Sums `estimatedOpportunity` across every loaded target, the way
 * `display_opportunity` sums per target: all-or-nothing across the list, and
 * only when every present figure shares the same unit. Returns null when the
 * target list has not loaded yet, is empty, or does not price to dollars. */
function summedCostLine(targets: readonly BurnCheckTargetPayload[] | undefined): string | null {
  if (!targets || targets.length === 0) return null
  let total = 0
  for (const target of targets) {
    const opportunity = target.display.estimatedOpportunity
    if (!opportunity || opportunity.unit !== "apiEquivalentUsd") return null
    total += opportunity.value
  }
  return formatApiEquivalentUsd(total)
}

export function checkRowPresentation(
  category: CheckRow,
  targets?: readonly BurnCheckTargetPayload[],
) {
  const failed = category.lifecycle === "failing"
  const status = checkStatusFacts(category)
  const metric = failed ? tokenBurnLabel(category) : null
  return {
    Icon: isInstalledCheck(category) ? CircleCheck : CHECK_ICONS[category.id],
    label: checkLabel(category),
    checking: status.checking === true,
    summary: checkRowStatus(category).text,
    metric,
    costLine: failed ? summedCostLine(targets) : null,
    iconTone:
      category.lifecycle === "failing"
        ? "bg-system-red/10 text-system-red-text"
        : status.lifecycle === "passing"
          ? "bg-system-green/10 text-system-green"
          : "bg-surface-card text-label-secondary",
    metricTone:
      metric && category.estimatedTokenBurnBasisPoints != null
        ? tokenBurnTone(category.estimatedTokenBurnBasisPoints)
        : null,
  }
}
