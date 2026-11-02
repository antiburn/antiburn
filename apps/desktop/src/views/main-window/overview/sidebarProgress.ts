import type { SidebarNavItem } from "../../../components/ui/SidebarNav"
import type { MainViewId } from "../../../lib/navigation/mainViews"
import type { AllowanceUsageSummaryPayload } from "../../../lib/providerUsageIpc"
import {
  LIVE_LIMITS_TRANSITION_NAME,
  progressStepTransitionName,
  stepDocked,
  type OverviewProgress,
} from "./overviewProgressStore"

export function sidebarProgress(
  view: MainViewId,
  progress: OverviewProgress,
  allowance: AllowanceUsageSummaryPayload | null,
): Pick<SidebarNavItem, "status" | "transitionName"> {
  if (progress.mode === "pending") return {}
  const { flow } = progress
  const limitsDocked = ["sessions", "checks", "fixes", "done"].includes(flow)
  switch (view) {
    case "agents":
      return stepDocked(flow, "agents")
        ? {
            status: String(progress.agents.rows.filter((row) => row.sessions > 0).length),
            transitionName: progressStepTransitionName("agents"),
          }
        : {}
    case "quota":
      return limitsDocked
        ? {
            status: (allowance?.accounts ?? [])
              .flatMap((account) =>
                account.utilization == null
                  ? []
                  : [`${Math.round(account.utilization.utilizationPercent)}%`],
              )
              .join(" / "),
            transitionName: LIVE_LIMITS_TRANSITION_NAME,
          }
        : {}
    case "activity":
      return stepDocked(flow, "sessions")
        ? {
            status: progress.sessions.displayCompleted.toLocaleString(),
            transitionName: progressStepTransitionName("sessions"),
          }
        : {}
    case "burnChecks":
      return stepDocked(flow, "checks")
        ? {
            status: `${progress.failingCount} to fix`,
            transitionName: progressStepTransitionName(
              stepDocked(flow, "fixes") ? "fixes" : "checks",
            ),
          }
        : {}
    default:
      return {}
  }
}
