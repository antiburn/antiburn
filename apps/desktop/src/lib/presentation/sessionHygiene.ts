import type {
  InsightsNotAssessedReason,
  SessionHygieneBadgeId,
  SessionHygieneBadgePayload,
  SessionHygieneEvidenceState,
  SessionHygienePayload,
} from "../insightsIpc"

type SessionHygieneInk = "system-green" | "system-red-text" | "label-tertiary"

export interface SessionHygieneCheck {
  id: SessionHygieneBadgeId
  status: SessionHygieneBadgePayload["status"]
  notAssessedReason: InsightsNotAssessedReason | null
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
}

const CHECKS: readonly HygieneCheckDefinition[] = [
  {
    id: "sessionOverdepth",
    name: "Session overdepth",
    cleanTitle: "No session overdepth detected",
    findingTitle: "Session overdepth detected",
    notAssessedTitle: "Session overdepth not assessed",
  },
  {
    id: "modelOverthinking",
    name: "Model overthinking",
    cleanTitle: "No model overthinking detected",
    findingTitle: "Model overthinking detected",
    notAssessedTitle: "Model overthinking not assessed",
  },
  {
    id: "overpoweredSubagents",
    name: "Overpowered subagents",
    cleanTitle: "No overpowered subagents detected",
    findingTitle: "Overpowered subagents detected",
    notAssessedTitle: "Overpowered subagents not assessed",
  },
  {
    id: "obsoleteModel",
    name: "Obsolete model",
    cleanTitle: "No obsolete model detected",
    findingTitle: "Obsolete model detected",
    notAssessedTitle: "Obsolete model not assessed",
  },
  {
    id: "fastModeOveruse",
    name: "Fast-mode overuse",
    cleanTitle: "No fast mode overuse detected",
    findingTitle: "Fast mode overuse detected",
    notAssessedTitle: "Fast mode overuse not assessed",
  },
  {
    id: "excessCacheRehydration",
    // This name diverges from the title wording on purpose. "Excess cache
    // rehydration" names the mechanism; "Excess context reprocessing"
    // names the check in reader terms.
    name: "Excess context reprocessing",
    cleanTitle: "No excess cache rehydration detected",
    findingTitle: "Excess cache rehydration detected",
    notAssessedTitle: "Excess cache rehydration not assessed",
  },
]

const NOT_ASSESSED: SessionHygieneBadgePayload = {
  id: "sessionOverdepth",
  status: "notAssessed",
  notAssessedReason: "incompleteEvidence",
}

/** Reader copy for an `excessCacheRehydration` finding, keyed by the
 *  vendor billing mechanism the badge payload names. */
const ACCOUNTING_DETAIL: Record<NonNullable<SessionHygieneBadgePayload["accounting"]>, string> = {
  cacheWrite: "Reduce repeated cache writes",
  uncachedInput: "Reduce full-price context re-reads",
}

export const INITIAL_SESSION_HYGIENE: SessionHygienePayload = {
  badges: CHECKS.map((check) => ({ ...NOT_ASSESSED, id: check.id })),
  evidenceState: "pending",
}

/** Add reader copy and semantic ink to the engine badge identifiers. */
export function sessionHygieneChecks(payload: SessionHygienePayload): SessionHygieneCheck[] {
  return CHECKS.map((definition) => {
    const badge = payload.badges.find((candidate) => candidate.id === definition.id) ?? {
      ...NOT_ASSESSED,
      id: definition.id,
    }
    const detail =
      badge.status === "finding" && badge.accounting ? ACCOUNTING_DETAIL[badge.accounting] : null
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
