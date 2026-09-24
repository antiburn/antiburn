import type {
  BurnCheckDetectorId,
  BurnCheckTargetListPayload,
  BurnCheckTargetPayload,
  ChecksCategoryPayload,
  SessionHygieneBadgeId,
} from "../../../lib/insightsIpc"
import { agentDisplayName } from "../../../lib/presentation/agents"
import { CHECK_LABELS } from "../../../lib/presentation/checkDefinitions"
import { modelShortName } from "../../../lib/presentation/models"

// The check behind each session hygiene badge.
const BADGE_CHECKS: Record<SessionHygieneBadgeId, BurnCheckDetectorId> = {
  sessionOverdepth: "sessionsOverDepth",
  modelOverthinking: "modelOverthinking",
  overpoweredSubagents: "overpoweredSubagents",
  obsoleteModel: "oldModelUsage",
  fastModeOveruse: "overuseOfFastMode",
  excessCacheRehydration: "cacheChurn",
}

/** Checks about the agent setup. They fail in most sessions, so the chart
 *  shows each one once at the weekly reset, not as a pin per session. */
export const CONFIG_CHECKS: ReadonlySet<BurnCheckDetectorId> = new Set<BurnCheckDetectorId>([
  "overpoweredSubagents",
  "unusedMcpServers",
  "unusedBuiltInTools",
  "unusedSkills",
])

/** One wasteful session on the allowance chart. */
export interface WastePin {
  detector: BurnCheckDetectorId
  label: string
  atEpoch: number
  title: string
  navigationHandle: string
  repo: string
  agent: string
  /** Short model names, in the order the session used them. */
  models: string[]
  costUsd: number | null
  /** The other failed checks of the same session. */
  alsoFailed: string[]
}

/** What the chart can say about one failing check. */
export interface CheckFacts {
  detector: BurnCheckDetectorId
  /** Estimated avoidable tokens over used tokens, in basis points. */
  burnBasisPoints: number | null
  /** The most common suggested change, for example "Opus → Sonnet". */
  change: string | null
}

/** A config check and the share of checked sessions that fail it. */
export interface ConfigShare {
  detector: BurnCheckDetectorId
  label: string
  share: number
  /** The sessions that fail the check. */
  finding: number
  /** The sessions the check could read. */
  sessions: number
}

export interface WasteMarks {
  pins: WastePin[]
  config: ConfigShare[]
  checks?: CheckFacts[]
  onOpen?: (pin: WastePin) => void
}

/** The failing checks that get a pin per session. */
export function pinnedDetectors(
  failures: readonly ChecksCategoryPayload[],
): BurnCheckDetectorId[] {
  return failures.map((check) => check.id).filter((id) => !CONFIG_CHECKS.has(id))
}

/** The most common change that the targets suggest, or null. */
function commonChange(targets: readonly BurnCheckTargetPayload[]): string | null {
  const counts = new Map<string, number>()
  for (const { display } of targets) {
    if (!display.currentValue || !display.replacementValue) continue
    const name = (value: string) =>
      display.resourceKind === "model" || display.resourceKind === "worker"
        ? modelShortName(value)
        : value
    const change = `${name(display.currentValue)} → ${name(display.replacementValue)}`
    counts.set(change, (counts.get(change) ?? 0) + 1)
  }
  return [...counts.entries()].sort((left, right) => right[1] - left[1])[0]?.[0] ?? null
}

/** The pins and config shares for the failing checks. A pin needs the check's
 *  sample sessions, so a check without loaded targets has no pins yet. */
export function wasteMarks(
  failures: readonly ChecksCategoryPayload[],
  targets: Partial<Record<BurnCheckDetectorId, { data: BurnCheckTargetListPayload | null }>>,
): Omit<WasteMarks, "onOpen"> {
  const config = failures
    .filter((check) => CONFIG_CHECKS.has(check.id) && check.finding + check.clean > 0)
    .map((check) => ({
      detector: check.id,
      label: CHECK_LABELS[check.id],
      share: check.finding / (check.finding + check.clean),
      finding: check.finding,
      sessions: check.finding + check.clean,
    }))
    .sort((left, right) => right.share - left.share)
  const pins = pinnedDetectors(failures).flatMap((detector) =>
    (targets[detector]?.data?.samples ?? []).map((sample) => ({
      detector,
      label: CHECK_LABELS[detector],
      atEpoch: sample.observedAtMs / 1000,
      title: sample.title,
      navigationHandle: sample.navigationHandle,
      repo: sample.repo,
      agent: agentDisplayName(sample.agent),
      models: [...new Set(sample.models.map(modelShortName))],
      costUsd: sample.cost?.totalUsd ?? null,
      alsoFailed: sample.hygiene.badges
        .filter((badge) => badge.status === "finding" && BADGE_CHECKS[badge.id] !== detector)
        .map((badge) => CHECK_LABELS[BADGE_CHECKS[badge.id]]),
    })),
  )
  const checks = failures.map((check) => ({
    detector: check.id,
    burnBasisPoints: check.estimatedTokenBurnBasisPoints,
    change: commonChange(targets[check.id]?.data?.targets ?? []),
  }))
  return { pins, config, checks }
}
