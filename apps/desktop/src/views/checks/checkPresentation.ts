import {
  Bot,
  BookOpen,
  BookOpenCheck,
  Brain,
  Database,
  Gauge,
  History,
  Layers3,
  Search,
  Server,
  Sparkles,
  Target,
  Wrench,
  type LucideIcon,
} from "lucide-react"

import type {
  BurnCheckDetectorId,
  BurnCheckTargetPayload,
  ChecksCategoryPayload,
} from "../../lib/insightsIpc"
import { checkHasProvisionalResult } from "../../lib/presentation/checkStatus"
import {
  CHECK_LABELS,
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
    icon: BookOpen,
    recommendation: "Load this skill only when the request needs it to avoid unused context.",
  },
  skillOpportunities: {
    icon: Sparkles,
    recommendation: "Use this current skill for similar future work.",
  },
  overExploring: {
    icon: Search,
    recommendation: "Read only the files and sections needed for future work.",
  },
  scopeCreep: {
    icon: Target,
    recommendation:
      "Keep future work within the agreed task. Ask for approval before adding work.",
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
  ignoredInstructions: {
    icon: BookOpenCheck,
    recommendation: "Follow the cited instruction and correct the affected work.",
  },
}

const CHECK_ICONS: Record<BurnCheckDetectorId, LucideIcon> = {
  sessionsOverDepth: CHECK_UI.sessionsOverDepth.icon,
  modelOverthinking: CHECK_UI.modelOverthinking.icon,
  overpoweredSubagents: CHECK_UI.overpoweredSubagents.icon,
  unusedMcpServers: CHECK_UI.unusedMcpServers.icon,
  unusedBuiltInTools: CHECK_UI.unusedBuiltInTools.icon,
  unusedSkills: CHECK_UI.unusedSkills.icon,
  skillOpportunities: CHECK_UI.skillOpportunities.icon,
  overExploring: CHECK_UI.overExploring.icon,
  scopeCreep: CHECK_UI.scopeCreep.icon,
  oldModelUsage: CHECK_UI.oldModelUsage.icon,
  overuseOfFastMode: CHECK_UI.overuseOfFastMode.icon,
  cacheChurn: CHECK_UI.cacheChurn.icon,
  ignoredInstructions: CHECK_UI.ignoredInstructions.icon,
}

function failedSessionSummary(category: ChecksCategoryPayload): string {
  if (category.lifecycle === "awaitingVerification") return "Awaiting verification"
  if (category.lifecycle === "passing") return "Passed"
  const assessed = category.finding + category.clean
  return assessed > 0
    ? `${category.finding}/${assessed} session${assessed === 1 ? "" : "s"} failed`
    : "Check failed"
}

function tokenBurnLabel(category: ChecksCategoryPayload): string | null {
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
  category: ChecksCategoryPayload,
  targets?: readonly BurnCheckTargetPayload[],
) {
  const failed = category.lifecycle === "failing"
  const provisional = checkHasProvisionalResult(category)
  const metric =
    failed && category.id !== "ignoredInstructions" ? tokenBurnLabel(category) : null
  return {
    Icon: CHECK_ICONS[category.id],
    label: CHECK_LABELS[category.id],
    provisional,
    checking: category.checking === true && category.lifecycle !== "awaitingVerification",
    coverage: category.reviewCoverage
      ? [
          category.reviewCoverage.total == null
            ? `${category.reviewCoverage.reviewed} reviewed`
            : `${category.reviewCoverage.reviewed} of ${category.reviewCoverage.total} reviewed`,
          `${category.reviewCoverage.uncertain} uncertain`,
          `${category.reviewCoverage.pending} pending`,
        ].join(" · ")
      : null,
    evidenceLimits: [
      ...(category.sampled
        ? [
            {
              label: "This check has been sampled",
              details: [
                "This check assesses selected evidence. No finding in a sample does not establish that all work has been assessed.",
                ...(category.checking && category.reviewCoverage?.continuing
                  ? ["Review is continuing."]
                  : []),
              ],
            },
          ]
        : []),
      ...(category.partialContext
        ? [
            {
              label: "This check used partial context",
              details: [
                "This check used incomplete context. Missing context can limit the assessment.",
              ],
            },
          ]
        : []),
    ],
    summary: provisional
      ? "No issues found yet"
      : failed
        ? failedSessionSummary(category)
        : category.lifecycle === "passing"
          ? "Passed"
          : category.lifecycle === "awaitingVerification"
            ? "Awaiting verification"
            : "Not assessed",
    metric,
    costLine: failed ? summedCostLine(targets) : null,
    iconTone:
      category.lifecycle === "failing"
        ? "bg-system-red/10 text-system-red-text"
        : category.lifecycle === "passing" || provisional
          ? "bg-system-green/10 text-system-green"
          : "bg-surface-card text-label-secondary",
    metricTone:
      metric && category.estimatedTokenBurnBasisPoints != null
        ? tokenBurnTone(category.estimatedTokenBurnBasisPoints)
        : null,
  }
}
