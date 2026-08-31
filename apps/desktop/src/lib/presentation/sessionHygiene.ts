import type {
  InsightsNotAssessedReason,
  SessionHygieneBadgeId,
  SessionHygieneBadgePayload,
  SessionHygieneEvidenceState,
  SessionHygienePayload,
} from "../insightsIpc"
import { modelShortName } from "./models"

type SessionHygieneInk = "system-green" | "system-red-text" | "label-tertiary"

export interface SessionHygieneCheck {
  id: SessionHygieneBadgeId
  status: SessionHygieneBadgePayload["status"]
  notAssessedReason: InsightsNotAssessedReason | null
  findingEvidence?: SessionHygieneBadgePayload["findingEvidence"]
  title: string
  /**
   * The check name alone, with no verdict. Use it where a separate line
   * carries the verdict, so the two do not repeat each other.
   */
  name: string
  /**
   * Accounting-specific remediation copy for a finding, when the badge
   * payload names the mechanism. Null for every other status, and for a
   * finding with no `accounting` (old evidence).
   */
  detail: string | null
  ink: SessionHygieneInk
}

interface HygieneCheckDefinition {
  id: SessionHygieneBadgeId
  /** The check name alone, with no verdict. Feeds `SessionHygieneCheck.name`. */
  name: string
  cleanTitle: string
  findingTitle: string
  notAssessedTitle: string
  summary: string
  guidance: readonly string[]
}

const CHECKS: readonly HygieneCheckDefinition[] = [
  {
    id: "sessionOverdepth",
    name: "Session overdepth",
    cleanTitle: "Session didn't get too deep",
    findingTitle: "Session went too deep",
    notAssessedTitle: "Session depth not assessed",
    summary:
      "Deep context burns more tokens with each request, directly affecting cost and quality.",
    guidance: [
      "Compaction works now, use it over about 200k tokens.",
      "Use subagents to preserve parent context.",
    ],
  },
  {
    id: "modelOverthinking",
    name: "Model overthinking",
    cleanTitle: "Thinking/reasoning modes ok",
    findingTitle: "Thinking/reasoning modes too high",
    notAssessedTitle: "Thinking/reasoning modes not assessed",
    summary: "Higher thinking modes burn more tokens without giving better quality.",
    guidance: [
      "Keep thinking/reasoning/effort below xhigh.",
      "Default to high for most tasks.",
    ],
  },
  {
    id: "overpoweredSubagents",
    name: "Overpowered subagents",
    cleanTitle: "Subagent models ok",
    findingTitle: "Subagent models too powerful",
    notAssessedTitle: "Subagent models not assessed",
    summary:
      "Subagents have to reorient themselves. Using premium subagents gets expensive fast.",
    guidance: ["Get your premium main agent to delegate to cheaper subagents."],
  },
  {
    id: "obsoleteModel",
    name: "Obsolete model",
    cleanTitle: "All models up to date",
    findingTitle: "Old model usage detected",
    notAssessedTitle: "Model obsolescence not assessed",
    summary: "Newer models usually give better output at the same or cheaper cost.",
    guidance: [
      "Manually switch to the current replacement.",
      "Update the agent's default model in config.",
    ],
  },
  {
    id: "fastModeOveruse",
    name: "Fast mode overuse",
    cleanTitle: "Fast mode not overused",
    findingTitle: "Fast mode overused",
    notAssessedTitle: "Fast mode not assessed",
    summary: "Fast mode costs a lot more for a little extra speed.",
    guidance: ["Use standard speed by default.", "Rarely use fast mode for subagents."],
  },
  {
    id: "excessCacheRehydration",
    name: "Excess cache rehydration",
    cleanTitle: "Cache rehydration under control",
    findingTitle: "Cache rehydration out of control",
    notAssessedTitle: "Cache rehydration not assessed",
    summary: "Spending tokens to refresh the server-side cache is a waste of money/quota.",
    guidance: [
      "Avoid long breaks in sessions.",
      "If you have a long break, compact before or even after it.",
      "Avoid switching models with a large context accumulated.",
    ],
  },
]

export interface SessionHygieneDocumentation {
  summary: string
  findingDetails: readonly string[]
  guidance: readonly string[]
}

const NOT_ASSESSED: SessionHygieneBadgePayload = {
  id: "sessionOverdepth",
  status: "notAssessed",
  notAssessedReason: "incompleteEvidence",
}

/** Reader copy for an `excessCacheRehydration` finding, keyed by the
 *  vendor billing mechanism the badge payload names. */
const ACCOUNTING_DETAIL: Record<
  NonNullable<SessionHygieneBadgePayload["accounting"]>,
  string
> = {
  cacheWrite: "Reduce repeated cache writes",
  uncachedInput: "Reduce full-price context re-reads",
}

export const INITIAL_SESSION_HYGIENE: SessionHygienePayload = {
  badges: CHECKS.map((check) => ({ ...NOT_ASSESSED, id: check.id })),
  evidenceState: "pending",
}

/** The reader-facing name of one check, with no verdict attached. */
export function sessionHygieneCheckName(id: SessionHygieneBadgeId): string {
  return CHECKS.find((check) => check.id === id)?.name ?? id
}

/** Add reader copy and semantic ink to the engine badge identifiers. */
export function sessionHygieneChecks(payload: SessionHygienePayload): SessionHygieneCheck[] {
  return CHECKS.map((definition) => {
    const badge = payload.badges.find((candidate) => candidate.id === definition.id) ?? {
      ...NOT_ASSESSED,
      id: definition.id,
    }
    const detail =
      badge.status === "finding" && badge.accounting
        ? ACCOUNTING_DETAIL[badge.accounting]
        : null
    if (badge.status === "finding") {
      return {
        ...badge,
        title: definition.findingTitle,
        name: definition.name,
        detail,
        ink: "system-red-text" as const,
      }
    }
    if (badge.status === "clean") {
      return {
        ...badge,
        title: definition.cleanTitle,
        name: definition.name,
        detail,
        ink: "system-green" as const,
      }
    }
    return {
      ...badge,
      title: definition.notAssessedTitle,
      name: definition.name,
      detail,
      ink: "label-tertiary" as const,
    }
  })
}

/** Return the explanation and guidance for one hygiene check. */
export function sessionHygieneDocumentation(
  check: SessionHygieneCheck,
): SessionHygieneDocumentation {
  const definition = CHECKS.find((candidate) => candidate.id === check.id)!
  const guidance = check.detail ? [check.detail, ...definition.guidance] : definition.guidance
  return {
    summary: definition.summary,
    findingDetails: sessionHygieneFindingDetails(check),
    guidance,
  }
}

function readableCount(value: number, noun: string): string {
  return `${value.toLocaleString()} ${value === 1 ? noun : `${noun}s`}`
}

function readableModels(models: readonly string[]): string {
  const shortNames = [...new Set(models.map(modelShortName))]
  return new Intl.ListFormat(undefined, { style: "long", type: "conjunction" }).format(
    shortNames,
  )
}

/** Describe the stored facts that caused one finding. */
function sessionHygieneFindingDetails(check: SessionHygieneCheck): string[] {
  const evidence = check.findingEvidence
  if (check.status !== "finding" || !evidence) return []

  switch (evidence.kind) {
    case "sessionOverdepth":
      return [
        `The deepest request carried ${evidence.maxRequestContextTokens.toLocaleString()} tokens. The reviewed limit is ${evidence.depthCapTokens.toLocaleString()}.`,
      ]
    case "modelOverthinking":
      return evidence.tiers.map(({ tier, mainLoopTurns, delegatedTurns }) => {
        const turns = mainLoopTurns + delegatedTurns
        return `The session used ${tier} reasoning for ${readableCount(turns, "turn")}.`
      })
    case "overpoweredSubagents":
      return [
        `${readableModels(evidence.mainModels)} used premium subagents (${readableModels(evidence.delegatedModels)}).`,
      ]
    case "obsoleteModel":
      return evidence.models.map(
        ({ model, replacement }) =>
          `${modelShortName(model)} was still in use after ${modelShortName(replacement)} became available.`,
      )
    case "fastModeOveruse":
      return [
        `Fast mode was used for ${readableCount(evidence.delegatedTurns, "delegated turn")}.`,
      ]
    case "excessCacheRehydration": {
      const uniqueTokens = evidence.paidTokens - evidence.repeatedTokens
      if (uniqueTokens <= 0) {
        return [
          `All ${evidence.paidTokens.toLocaleString()} paid context tokens repeated. The finding threshold is ${evidence.thresholdMultiple.toLocaleString()}×.`,
        ]
      }
      const observedMultiple = evidence.paidTokens / uniqueTokens
      return [
        `${evidence.repeatedTokens.toLocaleString()} of ${evidence.paidTokens.toLocaleString()} paid context tokens repeated. This raised paid context to ${observedMultiple.toLocaleString(undefined, { maximumFractionDigits: 2 })}× the unique context; the finding threshold is ${evidence.thresholdMultiple.toLocaleString()}×.`,
      ]
    }
  }
}

/** Name a non-ready evidence state without implying a clean result. */
export function sessionHygieneStateLabel(state: SessionHygieneEvidenceState): string | null {
  switch (state) {
    case "pending":
    case "processing":
      return "Computing"
    case "stale":
      return "Refreshing"
    case "activelyGrowing":
      return "Still writing"
    case "unsupported":
      return "Unsupported"
    case "failed":
      return "Unavailable"
    case "ready":
      return null
  }
}

/** True while the engine still works and the verdict can change on its own. */
export function sessionHygieneStateIsTransient(state: SessionHygieneEvidenceState): boolean {
  switch (state) {
    case "pending":
    case "processing":
    case "stale":
    case "activelyGrowing":
      return true
    case "unsupported":
    case "failed":
    case "ready":
      return false
  }
}

/** Reader wording for why one check was not assessed. */
export function notAssessedReasonLabel(reason: InsightsNotAssessedReason): string {
  switch (reason) {
    case "capabilityMissing":
      return "this agent's logs don't record what this check needs"
    case "incompleteEvidence":
      return "couldn't read the whole session log"
    case "evidenceContractIncomplete":
      return "the log is missing data this check needs"
    case "signalMissing":
      return "this session did not record the setting this check needs"
    case "noSessionsInWindow":
      return "no sessions in the window"
  }
}
