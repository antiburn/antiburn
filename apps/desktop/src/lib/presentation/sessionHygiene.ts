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
  ink: SessionHygieneInk
}

interface HygieneCheckDefinition {
  id: SessionHygieneBadgeId
  cleanTitle: string
  findingTitle: string
  notAssessedTitle: string
}

const CHECKS: readonly HygieneCheckDefinition[] = [
  {
    id: "reasoningOverkill",
    cleanTitle: "No reasoning overkill",
    findingTitle: "Reasoning overkill",
    notAssessedTitle: "Reasoning overkill not assessed",
  },
  {
    id: "excessCacheRehydration",
    cleanTitle: "No excess cache rehydration",
    findingTitle: "Excess cache rehydration",
    notAssessedTitle: "Excess cache rehydration not assessed",
  },
  {
    id: "bloatedInitialContext",
    cleanTitle: "No bloated initial context",
    findingTitle: "Bloated initial context",
    notAssessedTitle: "Bloated initial context not assessed",
  },
]

const NOT_ASSESSED: SessionHygieneBadgePayload = {
  id: "reasoningOverkill",
  status: "notAssessed",
  notAssessedReason: "incompleteEvidence",
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
    if (badge.status === "finding") {
      return {
        ...badge,
        title: definition.findingTitle,
        name: definition.findingTitle,
        ink: "system-red-text" as const,
      }
    }
    if (badge.status === "clean") {
      return {
        ...badge,
        title: definition.cleanTitle,
        name: definition.findingTitle,
        ink: "system-green" as const,
      }
    }
    return {
      ...badge,
      title: definition.notAssessedTitle,
      name: definition.findingTitle,
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
    case "noSessionsInWindow":
      return "no sessions in the window"
  }
}
