import type {
  InsightsNotAssessedReason,
  SessionHygieneBadgeId,
  SessionHygieneBadgePayload,
  SessionHygieneEvidenceState,
  SessionHygienePayload,
} from "../insightsIpc"
import { modelShortName } from "./models"
import { overExploringDetail } from "./checkDefinitions"

type SessionHygieneInk = "system-green" | "system-red-text" | "label-tertiary"

export interface SessionHygieneCheck {
  id: SessionHygieneBadgeId
  status: SessionHygieneBadgePayload["status"]
  notAssessedReason: InsightsNotAssessedReason | null
  checkReason?: string
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
  serverOnly?: boolean
  /** The check name alone, with no verdict. Feeds `SessionHygieneCheck.name`. */
  name: string
  cleanTitle: string
  findingTitle: string
  notAssessedTitle: string
  summary: string
  guidance: readonly string[]
  /**
   * One or two short sentences for the checks tab's footer: what the check
   * tests and what failing it costs in tokens. Kept apart from `summary`,
   * which opens inside a row and can assume the row's verdict as context.
   */
  explainer: string
}

function defineHygieneCheck(
  id: SessionHygieneBadgeId,
  name: string,
  cleanTitle: string,
  findingTitle: string,
  notAssessedTitle: string,
  summary: string,
  guidance: readonly string[],
  explainer: string,
  serverOnly = false,
): HygieneCheckDefinition {
  return {
    id,
    name,
    cleanTitle,
    findingTitle,
    notAssessedTitle,
    summary,
    guidance,
    explainer,
    ...(serverOnly ? { serverOnly } : {}),
  }
}

const CHECKS: readonly HygieneCheckDefinition[] = [
  defineHygieneCheck(
    "scopeCreep",
    "Scope creep",
    "No scope creep in assessed work",
    "Scope creep found",
    "Scope creep not assessed",
    "Selected proposed or attempted work adds a separate objective.",
    ["Keep future work within the agreed task. Ask for approval before adding work."],
    "Finds extra work outside the agreed task using recorded scope and approval evidence from accepted OpenCode, Codex, Claude Code, and Pi sessions. Incomplete or unproven scope cannot establish approval.",
    true,
  ),
  defineHygieneCheck(
    "sessionOverdepth",
    "Session overdepth",
    "Session didn't get too deep",
    "Session went too deep",
    "Session depth not assessed",
    "Deep context burns more tokens with each request, directly affecting cost and quality.",
    [
      "Compaction works now, use it over about 200k tokens.",
      "Use subagents to preserve parent context.",
    ],
    "Past about 200k tokens, every turn resends the whole history as cache reads. Deep sessions cost more per message than fresh ones.",
  ),
  defineHygieneCheck(
    "modelOverthinking",
    "Model overthinking",
    "Thinking/reasoning modes ok",
    "Thinking/reasoning modes too high",
    "Thinking/reasoning modes not assessed",
    "Higher thinking modes burn more tokens without giving better quality.",
    ["Keep thinking/reasoning/effort below xhigh.", "Default to high for most tasks."],
    "High reasoning effort spends extra output tokens on every reply. Most tasks do fine on a lower setting.",
  ),
  defineHygieneCheck(
    "overpoweredSubagents",
    "Overpowered subagents",
    "Subagent models ok",
    "Subagent models too powerful",
    "Subagent models not assessed",
    "Subagents have to reorient themselves. Using premium subagents gets expensive fast.",
    ["Get your premium main agent to delegate to cheaper subagents."],
    "Subagents inherit the big model for fetch-and-carry work. Routine lookups on a smaller model cost a fraction.",
  ),
  defineHygieneCheck(
    "obsoleteModel",
    "Obsolete model",
    "All models up to date",
    "Old model usage detected",
    "Model obsolescence not assessed",
    "Newer models usually give better output at the same or cheaper cost.",
    [
      "Manually switch to the current replacement.",
      "Update the agent's default model in config.",
    ],
    "Newer models do the same work better, usually at the same or lower price.",
  ),
  defineHygieneCheck(
    "fastModeOveruse",
    "Fast mode overuse",
    "Fast mode not overused",
    "Fast mode overused",
    "Fast mode not assessed",
    "Fast mode costs a lot more for a little extra speed.",
    ["Use standard speed by default.", "Rarely use fast mode for subagents."],
    "Fast mode trades a higher token rate for speed. Keep it for bursts, not as the default.",
  ),
  defineHygieneCheck(
    "excessCacheRehydration",
    "Excess cache rehydration",
    "Cache rehydration under control",
    "Cache rehydration out of control",
    "Cache rehydration not assessed",
    "Repeated full-price context processing can increase cost and limit use.",
    [
      "Avoid long breaks in sessions.",
      "If you have a long break, compact before or even after it.",
      "Avoid switching models with a large context accumulated.",
    ],
    "This estimates paid context beyond context growth. Cache expiry, context changes, and provider evictions can contribute; the estimate does not establish the cause.",
  ),
  defineHygieneCheck(
    "ignoredInstructions",
    "Ignored instructions",
    "No instruction conflict in assessed work",
    "Instruction conflict found",
    "Instructions not assessed",
    "A selected action conflicts with an instruction.",
    ["Follow the cited instruction and correct the affected work."],
    "Compares selected actions with current or recorded instruction text.",
    true,
  ),
  defineHygieneCheck(
    "skillOpportunities",
    "Skill opportunities",
    "No skill opportunity in assessed work",
    "Skill opportunity found",
    "Skill opportunities not assessed",
    "Assessed work matches a skill in your current inventory.",
    ["Use this current skill for similar future work."],
    "Compares selected work with current skills. Current inventory does not prove past availability.",
    true,
  ),
  defineHygieneCheck(
    "overExploring",
    "Over-exploring",
    "No over-exploring in assessed work",
    "Over-exploring found",
    "Over-exploring not assessed",
    "Some assessed reads went beyond what the work needed.",
    ["Read only the files and sections needed for future work."],
    "Checks selected reads for unrelated files, excess file breadth, and excess reading within a file.",
    true,
  ),
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
  badges: CHECKS.filter((check) => !check.serverOnly).map((check) => ({
    ...NOT_ASSESSED,
    id: check.id,
  })),
  evidenceState: "pending",
  unusedResources: null,
}

/** Every check's name and footer explainer, for the checks tab. */
export function sessionHygieneExplainers(): Array<{
  id: SessionHygieneBadgeId
  name: string
  explainer: string
}> {
  return CHECKS.map(({ id, name, explainer }) => ({ id, name, explainer }))
}

/** Add reader copy and semantic ink to the engine badge identifiers. */
export function sessionHygieneChecks(payload: SessionHygienePayload): SessionHygieneCheck[] {
  return CHECKS.filter(
    (definition) =>
      !definition.serverOnly || payload.badges.some((badge) => badge.id === definition.id),
  ).map((definition) => {
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
    if (badge.status === "noCandidates") {
      return {
        ...badge,
        title: `${definition.name} · No matching work`,
        name: definition.name,
        detail: null,
        ink: "label-tertiary" as const,
      }
    }
    if (badge.status === "checking" || badge.status === "couldntCheck") {
      return {
        ...badge,
        title:
          badge.status === "checking"
            ? definition.id === "ignoredInstructions"
              ? "Checking instructions"
              : `Checking ${definition.name.toLowerCase()}`
            : definition.id === "scopeCreep" && badge.checkReason === "scope_context_too_large"
              ? "Scope creep · Task context exceeds the model limit."
              : definition.id === "ignoredInstructions"
                ? "Couldn't check instructions"
                : `Couldn't check ${definition.name.toLowerCase()}`,
        name: definition.name,
        detail,
        ink: "label-tertiary" as const,
      }
    }
    return {
      ...badge,
      title:
        definition.id === "scopeCreep" && badge.checkReason === "scope_context_too_large"
          ? "Scope creep · Task context exceeds the model limit."
          : definition.notAssessedTitle,
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
  if (check.status === "noCandidates") {
    return {
      summary: "No comparisons are available for this check in the recorded work.",
      findingDetails: [],
      guidance: [],
    }
  }
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
  if (check.status === "finding" && check.id === "overExploring") {
    const detail = overExploringDetail(check.checkReason)
    return detail ? [detail] : []
  }
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
