import type {
  BurnCheckDetectorId,
  BurnCheckTargetPayload,
  ChecksReportPayload,
} from "../../../lib/insightsIpc"
import { detectorMask } from "../../../lib/snoozedBurnChecks"
import type { OverviewViewPrefs } from "./overviewViewPrefs"

export type EnhanceButtonState =
  | { kind: "loading" }
  | { kind: "new"; count: number }
  | { kind: "resume"; step: number }
  | { kind: "watching"; count: number }
  | { kind: "fresh"; count: number }
  | { kind: "clear" }

/**
 * The state of the Overview Enhance button. When more than one state
 * applies, the first match wins: resume, new, watching, fresh, clear. A
 * resume does not need the report, so it shows while the report loads.
 *
 * `failing` holds the failing checks that are not snoozed, or null while the
 * report loads. `awaiting` counts the checks that wait for verification.
 */
export function enhanceButtonState(
  failing: readonly string[] | null,
  awaiting: number,
  prefs: OverviewViewPrefs,
): EnhanceButtonState {
  const completedAt = prefs.enhanceCompletedAt
  const startedAt = prefs.enhanceStartedAt
  if (startedAt != null && (completedAt == null || startedAt > completedAt))
    return { kind: "resume", step: prefs.enhanceStep ?? 1 }
  if (failing == null) return { kind: "loading" }
  if (completedAt != null) {
    const seen = new Set(prefs.enhanceSeenFailing ?? [])
    const unseen = failing.filter((id) => !seen.has(id)).length
    if (unseen > 0) return { kind: "new", count: unseen }
  }
  // Before the first finished run, a check can wait for verification without
  // any fix from the wizard. The button then offers a fresh run.
  if (completedAt != null && awaiting > 0) return { kind: "watching", count: awaiting }
  if (failing.length > 0) return { kind: "fresh", count: failing.length }
  return { kind: "clear" }
}

/** The projection horizon of the Done step. */
export const SAVINGS_MONTHS = 3

const TOKEN_UNITS = new Set(["literalInputTokens", "assumedOutputTokens", "cacheClassTokens"])

export interface ProjectedSavings {
  /** Tokens over the horizon, summed only from token estimates. */
  tokens: number
  /** API-equivalent USD over the horizon, summed only from dollar estimates. */
  usd: number
  /** Targets that gave an estimate in either unit. */
  estimated: number
}

/**
 * Projects each target's 30-day estimate over `SAVINGS_MONTHS`. Tokens and
 * dollars stay separate, because no reviewed rate converts one to the other.
 */
export function projectSavings(targets: readonly BurnCheckTargetPayload[]): ProjectedSavings {
  let tokens = 0
  let usd = 0
  let estimated = 0
  for (const target of targets) {
    const opportunity = target.display.estimatedOpportunity
    if (!opportunity) continue
    if (TOKEN_UNITS.has(opportunity.unit)) tokens += opportunity.value
    else if (opportunity.unit === "apiEquivalentUsd") usd += opportunity.value
    else continue
    estimated += 1
  }
  return { tokens: tokens * SAVINGS_MONTHS, usd: usd * SAVINGS_MONTHS, estimated }
}

export interface PredictedSavings {
  /** The most tokens the fixes can save over the horizon. */
  tokens: number
  /** The checks' combined share of all used tokens, in basis points. */
  basisPoints: number
}

/**
 * Predicts the most that fixes to `detectors` can save over `SAVINGS_MONTHS`.
 * It uses the combined burn of the checks in the report window, so no token
 * counts twice. The result is an upper bound, because a fix can remove only
 * part of the burn of a check. It returns null when the report cannot
 * measure token burn.
 */
export function predictSavings(
  report: ChecksReportPayload,
  detectors: ReadonlySet<BurnCheckDetectorId>,
): PredictedSavings | null {
  if (detectors.size === 0) return null
  const total = report.tokenBurnDenominator
  const basisPoints =
    report.estimatedTokenBurnBasisPointsByDetectorMask?.[detectorMask(detectors)]
  if (total == null || basisPoints == null || basisPoints === 0) return null
  return { tokens: Math.round((total * basisPoints) / 10_000) * SAVINGS_MONTHS, basisPoints }
}

/** True when the reader applied a fix to this target and it did not return. */
export function isAppliedFix(target: BurnCheckTargetPayload): boolean {
  return target.watch?.origin === "action" && target.watch.lifecycle !== "recurred"
}
