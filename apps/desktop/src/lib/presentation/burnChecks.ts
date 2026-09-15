import type { ChecksReportPayload, SessionHygieneEvidenceState } from "../insightsIpc"
import type { SessionHygieneCheck } from "./sessionHygiene"

type BurnCheckLifecycle =
  | "pending"
  | "processing"
  | "ready"
  | "stale"
  | "activelyGrowing"
  | "unsupported"
  | "unavailable"

type BurnCheckScope = "sessionChecks" | "reportCategories"
type BurnCheckOutcome = "failed" | "passed" | "unassessed"
type BurnCheckState =
  | "allPassed"
  | "assessedPassed"
  | "allFailed"
  | "mixed"
  | "incomplete"
  | "running"
  | "refreshing"
  | "awaitingSessionData"
  | "unsupported"
  | "unavailable"
  | "notAssessed"

interface BurnCheckCounts {
  failed: number
  passed: number
  unassessed: number
}

interface BurnCheckPhrase {
  outcome: BurnCheckOutcome | "status"
  text: string
}

type BurnCheckIndicatorSpec =
  | { kind: "pass" }
  | { kind: "fail" }
  | { kind: "segments"; segments: Array<{ outcome: BurnCheckOutcome; value: number }> }
  | { kind: "running" }
  | { kind: "neutral" }

export interface BurnCheckPresentation {
  state: BurnCheckState
  scope: BurnCheckScope
  counts: BurnCheckCounts
  evidenceComplete: boolean
  lifecycle: BurnCheckLifecycle
  refreshFailed: boolean
  headline: string
  headlineTone: "failure" | "neutral"
  compactPhrases: BurnCheckPhrase[]
  breakdownPhrases: BurnCheckPhrase[]
  contextPhrases: string[]
  accessibleDescription: string
  indicator: BurnCheckIndicatorSpec
}

interface PresentationFacts {
  scope: BurnCheckScope
  counts: BurnCheckCounts
  evidenceComplete: boolean
  lifecycle: BurnCheckLifecycle
  refreshFailed: boolean
}

function compactPhrases(counts: BurnCheckCounts): BurnCheckPhrase[] {
  const phrases: BurnCheckPhrase[] = []
  if (counts.failed > 0) {
    phrases.push({
      outcome: "failed",
      text: `${counts.failed} failed`,
    })
  }
  if (counts.passed > 0) {
    phrases.push({
      outcome: "passed",
      text: `${counts.passed} passed`,
    })
  }
  if (counts.unassessed > 0) {
    phrases.push({
      outcome: "unassessed",
      text: `${counts.unassessed} not assessed`,
    })
  }
  return phrases
}

function breakdownPhrases(counts: BurnCheckCounts): BurnCheckPhrase[] {
  const phrases: BurnCheckPhrase[] = []
  if (counts.failed > 0) phrases.push({ outcome: "failed", text: `${counts.failed} failed` })
  if (counts.passed > 0) phrases.push({ outcome: "passed", text: `${counts.passed} passed` })
  if (counts.unassessed > 0) {
    phrases.push({ outcome: "unassessed", text: `${counts.unassessed} not assessed` })
  }
  return phrases
}

function lifecycleState(lifecycle: BurnCheckLifecycle): {
  state: BurnCheckState
  headline: string
} {
  switch (lifecycle) {
    case "pending":
    case "processing":
      return { state: "running", headline: "Running Burn Checks…" }
    case "stale":
      return { state: "refreshing", headline: "Refreshing Burn Checks…" }
    case "activelyGrowing":
      return {
        state: "awaitingSessionData",
        headline: "Burn Checks awaiting session data",
      }
    case "unsupported":
      return { state: "unsupported", headline: "Burn Checks not supported" }
    case "unavailable":
      return { state: "unavailable", headline: "Burn Checks unavailable" }
    case "ready":
      return { state: "notAssessed", headline: "Burn Checks not assessed" }
  }
}

function indicatorFor(state: BurnCheckState, counts: BurnCheckCounts): BurnCheckIndicatorSpec {
  if (state === "allPassed" || state === "assessedPassed") return { kind: "pass" }
  if (state === "allFailed") return { kind: "fail" }
  if (state === "running" || state === "refreshing") return { kind: "running" }
  if (counts.failed + counts.passed > 0) {
    const segments = (["failed", "passed", "unassessed"] as const).flatMap((outcome) =>
      counts[outcome] > 0 ? [{ outcome, value: counts[outcome] }] : [],
    )
    return { kind: "segments", segments }
  }
  return { kind: "neutral" }
}

function contextPhrases(facts: PresentationFacts, hasResults: boolean): string[] {
  if (!hasResults) return []
  const phrases: string[] = []
  if (facts.lifecycle === "pending" || facts.lifecycle === "processing") {
    phrases.push("Running")
  }
  if (facts.lifecycle === "stale") phrases.push("Refreshing")
  if (facts.lifecycle === "activelyGrowing") phrases.push("Session still growing")
  if (facts.lifecycle === "unavailable" || facts.refreshFailed) {
    phrases.push("Refresh unavailable")
  }
  if (!facts.evidenceComplete) phrases.push("Evidence incomplete")
  return [...new Set(phrases)]
}

function presentation(facts: PresentationFacts): BurnCheckPresentation {
  const resultCount = facts.counts.failed + facts.counts.passed
  const hasResults = resultCount > 0
  const terminal = facts.lifecycle === "ready" && facts.evidenceComplete && hasResults
  let state: BurnCheckState
  let headline: string

  if (terminal && facts.counts.failed === 0) {
    state = "allPassed"
    headline = "All Burn Checks passed"
  } else if (terminal && facts.counts.passed === 0) {
    state = "allFailed"
    headline = "All Burn Checks failed"
  } else if (terminal) {
    state = "mixed"
    headline = "Some Burn Checks failed"
  } else if (hasResults && facts.counts.failed === 0) {
    state = "assessedPassed"
    headline = "All assessed Burn Checks passed"
  } else if (hasResults) {
    state = "incomplete"
    headline = "Burn Checks incomplete"
  } else {
    ;({ state, headline } = lifecycleState(facts.lifecycle))
  }

  const compact = hasResults
    ? compactPhrases(facts.counts)
    : [{ outcome: "status" as const, text: headline }]
  const breakdown = breakdownPhrases(facts.counts)
  const context = contextPhrases(facts, hasResults)
  const total = facts.counts.failed + facts.counts.passed + facts.counts.unassessed
  const scopeDescription =
    total === 0
      ? null
      : facts.scope === "sessionChecks"
        ? `${total} session ${total === 1 ? "check" : "checks"}`
        : `${total} report ${total === 1 ? "category" : "categories"}`
  const details = [
    scopeDescription,
    breakdown.length > 0 ? breakdown.map((phrase) => phrase.text).join(", ") : null,
    ...context,
  ].filter((detail): detail is string => detail !== null)
  const accessibleDescription =
    details.length === 0 ? headline : `${headline}. ${details.join(". ")}.`

  return {
    ...facts,
    state,
    headline,
    headlineTone:
      state === "allFailed" || state === "mixed" || facts.counts.failed > 0
        ? "failure"
        : "neutral",
    compactPhrases: compact,
    breakdownPhrases: breakdown,
    contextPhrases: context,
    accessibleDescription,
    indicator: indicatorFor(state, facts.counts),
  }
}

function sessionLifecycle(state: SessionHygieneEvidenceState): BurnCheckLifecycle {
  return state === "failed" ? "unavailable" : state
}

export function sessionBurnCheckPresentation(
  checks: readonly Pick<SessionHygieneCheck, "status">[],
  evidenceState: SessionHygieneEvidenceState,
): BurnCheckPresentation {
  const counts = checks.reduce<BurnCheckCounts>(
    (result, check) => {
      if (check.status === "finding") result.failed += 1
      else if (check.status === "clean") result.passed += 1
      else result.unassessed += 1
      return result
    },
    { failed: 0, passed: 0, unassessed: 0 },
  )
  return presentation({
    scope: "sessionChecks",
    counts,
    evidenceComplete: evidenceState === "ready" && counts.unassessed === 0,
    lifecycle: sessionLifecycle(evidenceState),
    refreshFailed: evidenceState === "failed" && counts.failed + counts.passed > 0,
  })
}

export function aggregateBurnCheckPresentation(
  report: ChecksReportPayload,
  refreshFailed = false,
): BurnCheckPresentation {
  const counts = report.categories.reduce<BurnCheckCounts>(
    (result, category) => {
      if (category.finding > 0) result.failed += 1
      else if (category.clean > 0) result.passed += 1
      else result.unassessed += 1
      return result
    },
    { failed: 0, passed: 0, unassessed: 0 },
  )
  const evidenceComplete =
    report.evidenceSettled && report.categories.every((category) => category.unavailable === 0)
  return presentation({
    scope: "reportCategories",
    counts,
    evidenceComplete,
    lifecycle: report.evidenceSettled ? "ready" : "processing",
    refreshFailed,
  })
}

export function emptyBurnCheckPresentation(
  lifecycle: "pending" | "unavailable",
): BurnCheckPresentation {
  return presentation({
    scope: "reportCategories",
    counts: { failed: 0, passed: 0, unassessed: 0 },
    evidenceComplete: false,
    lifecycle,
    refreshFailed: false,
  })
}
