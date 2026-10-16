import { isInstalledCheck, type CheckRow } from "./checkRows"
import { CHECK_DEFINITIONS } from "./checkDefinitions"

export function smartCheckReviewPresentation(check: CheckRow) {
  const coverage = check.reviewCoverage
  if (isInstalledCheck(check) || CHECK_DEFINITIONS[check.id].kind !== "smart" || !coverage) {
    return { terminal: false, terminalLabel: null, description: null, progress: null }
  }
  const empty = check.reviewState === "noMatchingWork"
  const stopped = check.reviewState === "stopped"
  const blocked = stopped && (coverage.contextBlocked ?? 0) > 0
  const noResult = check.finding === 0 && check.clean === 0
  const terminalLabel =
    noResult && empty ? "No matching work" : noResult && blocked ? "Check stopped" : null
  const description =
    terminalLabel === "No matching work"
      ? check.id === "skillOpportunities"
        ? "No matching work and skills were found to check."
        : check.id === "ignoredInstructions"
          ? "No instruction and action pairs were found to check."
          : "No matching work was found in the recorded task context."
      : blocked
        ? "Some items were too large for the selected model to check."
        : null
  const progress = empty
    ? null
    : [
        coverage.total == null
          ? `${coverage.reviewed} checked`
          : `${coverage.reviewed} of ${coverage.total} checked`,
        ...((coverage.skipped ?? 0) > 0 ? [`${coverage.skipped} unchecked`] : []),
        ...((coverage.contextBlocked ?? 0) > 0
          ? [`${coverage.contextBlocked} too large for this model`]
          : []),
        ...((coverage.uncertain ?? 0) > 0 ? [`${coverage.uncertain} unclear`] : []),
        ...((coverage.pendingCompletion ?? 0) > 0
          ? [`${coverage.pendingCompletion} waiting for the task to finish`]
          : []),
      ].join(" · ")
  return {
    terminal: empty || stopped,
    terminalLabel,
    description,
    progress,
  }
}
