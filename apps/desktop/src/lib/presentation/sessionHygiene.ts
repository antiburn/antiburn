import type {
  InsightsNotAssessedReason,
  SessionHygieneBadgeId,
  SessionHygieneBadgePayload,
  SessionHygieneEvidenceState,
  SessionHygienePayload,
} from "../insightsIpc"

type SessionHygieneInk = "system-green" | "system-red-text" | "label-tertiary"

/** Reader copy for one check's explanatory tooltip. */
interface SessionHygieneTooltip {
  heading: string
  /** What the check examined in the transcript. */
  body: string
  /** The token-burn mechanism the check protects against. */
  burn: string
}

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
  tooltip: SessionHygieneTooltip
}

interface HygieneCheckDefinition {
  id: SessionHygieneBadgeId
  cleanTitle: string
  findingTitle: string
  notAssessedTitle: string
  checkingTitle: string
  tooltip: SessionHygieneTooltip
}

const CHECKS: readonly HygieneCheckDefinition[] = [
  {
    id: "sessionOverdepth",
    cleanTitle: "No session overdepth detected",
    findingTitle: "Session overdepth detected",
    notAssessedTitle: "Session overdepth not assessed",
    checkingTitle: "Checking context depth…",
    tooltip: {
      heading: "Session overdepth",
      body: "Flags any single request that carried more than 160k tokens of context.",
      burn: "Every turn re-reads the whole context, so a deep session pays for it again and again. Handing off to a fresh session resets the cost.",
    },
  },
  {
    id: "modelOverthinking",
    cleanTitle: "No model overthinking detected",
    findingTitle: "Model overthinking detected",
    notAssessedTitle: "Model overthinking not assessed",
    checkingTitle: "Checking reasoning effort…",
    tooltip: {
      heading: "Model overthinking",
      body: "A turn ran at a top effort tier (max, ultrathink, or xhigh). Only tiers the transcript records count — nothing is guessed from prompts.",
      burn: "Top-tier reasoning multiplies thinking tokens on every turn it touches. Most tasks do fine one tier down.",
    },
  },
  {
    id: "overpoweredSubagents",
    cleanTitle: "No overpowered subagents detected",
    findingTitle: "Overpowered subagents detected",
    notAssessedTitle: "Overpowered subagents not assessed",
    checkingTitle: "Checking subagent models…",
    tooltip: {
      heading: "Overpowered subagents",
      body: "A premium main-loop model spawned subagents that ran on a premium model too, such as Opus delegating to Opus.",
      burn: "Every delegated turn bills at the top tier. Fetch-and-carry work — searching, reading files, running commands — lands the same on a cheaper model.",
    },
  },
  {
    id: "obsoleteModel",
    cleanTitle: "No obsolete model detected",
    findingTitle: "Obsolete model detected",
    notAssessedTitle: "Obsolete model not assessed",
    checkingTitle: "Checking model versions…",
    tooltip: {
      heading: "Obsolete model",
      body: "Turns ran on a superseded model after its reviewed replacement was already available.",
      burn: "An older model usually needs more turns to reach the same answer, and you pay for every one of them.",
    },
  },
  {
    id: "fastModeOveruse",
    cleanTitle: "No fast mode overuse detected",
    findingTitle: "Fast mode overuse detected",
    notAssessedTitle: "Fast mode overuse not assessed",
    checkingTitle: "Checking fast mode use…",
    tooltip: {
      heading: "Fast mode overuse",
      body: "Delegated subagent turns ran in fast mode. Only turns the transcript marks as fast tier count.",
      burn: "Fast mode trades cost for lower latency. A subagent runs unattended, so nobody is waiting on that speed.",
    },
  },
  {
    id: "excessCacheRehydration",
    cleanTitle: "No excess cache rehydration detected",
    findingTitle: "Excess cache rehydration detected",
    notAssessedTitle: "Excess cache rehydration not assessed",
    checkingTitle: "Checking cache rehydration…",
    tooltip: {
      heading: "Cache rehydration",
      body: "Looks for paid cache writes after something invalidated the prompt cache: a model switch, an idle gap past cache expiry, or a manual compaction.",
      burn: "Each rehydration re-writes the whole context prefix at cache-write rates — you pay for the same tokens again.",
    },
  },
]

/** Explain one not-assessed reason without implying a pass. */
export const SESSION_HYGIENE_NOT_ASSESSED_NOTE: Record<InsightsNotAssessedReason, string> = {
  incompleteEvidence:
    "Part of this transcript could not be read, and a gap never counts as a pass.",
  capabilityMissing: "This agent's transcripts do not record the evidence this check needs.",
  evidenceContractIncomplete:
    "The stored evidence predates this check, so there is nothing to assess yet.",
  noSessionsInWindow: "No evidence is stored for this session.",
}

/** The row labels shown while the engine computes the checks. */
export const SESSION_HYGIENE_CHECKING_ROWS: readonly string[] = CHECKS.map(
  (check) => check.checkingTitle,
)

/** What the hygiene block shows for one evidence state. */
export type SessionHygieneDisplay =
  { kind: "results" } | { kind: "checking" } | { kind: "notice"; text: string }

/** Map the evidence state to the hygiene block content. */
export function sessionHygieneDisplay(
  state: SessionHygieneEvidenceState,
): SessionHygieneDisplay {
  switch (state) {
    case "ready":
    case "stale":
      return { kind: "results" }
    case "pending":
    case "processing":
      return { kind: "checking" }
    case "activelyGrowing":
      return { kind: "notice", text: "Hygiene checks wait for the session to go quiet…" }
    case "unsupported":
      return { kind: "notice", text: "Hygiene checks are not supported for this agent." }
    case "failed":
      return { kind: "notice", text: "Hygiene checks are unavailable for this session." }
  }
}

const NOT_ASSESSED: SessionHygieneBadgePayload = {
  id: "sessionOverdepth",
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
        tooltip: definition.tooltip,
      }
    }
    if (badge.status === "clean") {
      return {
        ...badge,
        title: definition.cleanTitle,
        name: definition.findingTitle,
        ink: "system-green" as const,
        tooltip: definition.tooltip,
      }
    }
    return {
      ...badge,
      title: definition.notAssessedTitle,
      name: definition.findingTitle,
      ink: "label-tertiary" as const,
      tooltip: definition.tooltip,
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
