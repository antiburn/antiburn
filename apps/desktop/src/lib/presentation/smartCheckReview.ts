import type { ChecksCategoryPayload } from "../insightsIpc"
import { CHECK_DEFINITIONS } from "./checkDefinitions"

export function smartCheckReviewPresentation(check: ChecksCategoryPayload) {
  const coverage = check.reviewCoverage
  if (CHECK_DEFINITIONS[check.id].kind !== "smart" || !coverage) {
    return { terminal: false, terminalLabel: null, description: null, progress: null }
  }
  const stopped = !coverage.continuing
  const empty =
    stopped &&
    coverage.total === 0 &&
    coverage.reviewed === 0 &&
    coverage.pending === 0 &&
    (coverage.skipped === undefined || coverage.skipped === 0) &&
    (coverage.contextBlocked === undefined || coverage.contextBlocked === 0)
  const allRemainingSkipped =
    coverage.pending != null &&
    coverage.pending > 0 &&
    coverage.skipped != null &&
    coverage.skipped >= coverage.pending
  const blocked =
    stopped &&
    (check.checking !== true || allRemainingSkipped) &&
    (coverage.contextBlocked ?? 0) > 0
  const noResult = check.finding === 0 && check.clean === 0
  const terminalLabel =
    noResult && empty ? "No matching work" : noResult && blocked ? "Review stopped" : null
  const description =
    terminalLabel === "No matching work"
      ? check.id === "skillOpportunities"
        ? "No matching work and skills were found to review."
        : check.id === "ignoredInstructions"
          ? "No instruction and action pairs were found to review."
          : "No matching work was found in the recorded task context."
      : blocked
        ? "Some items were too large for the selected model to review."
        : null
  const progress = empty
    ? null
    : [
        coverage.total == null
          ? `${coverage.reviewed} reviewed`
          : `${coverage.reviewed} of ${coverage.total} reviewed`,
        ...((coverage.skipped ?? 0) > 0 ? [`${coverage.skipped} not reviewed`] : []),
        ...((coverage.contextBlocked ?? 0) > 0
          ? [`${coverage.contextBlocked} too large for this model`]
          : []),
        ...((coverage.uncertain ?? 0) > 0 ? [`${coverage.uncertain} unclear`] : []),
        ...((coverage.pendingCompletion ?? 0) > 0
          ? [`${coverage.pendingCompletion} waiting for the task to finish`]
          : []),
      ].join(" · ")
  return {
    terminal: empty || (stopped && allRemainingSkipped),
    terminalLabel,
    description,
    progress,
  }
}
