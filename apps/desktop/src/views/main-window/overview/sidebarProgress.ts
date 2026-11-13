import type { SidebarNavItem } from "../../../components/ui/SidebarNav"
import type { MainViewId } from "../../../lib/navigation/mainViews"
import type { AllowanceUsageSummaryPayload } from "../../../lib/providerUsageIpc"
import type { OverviewProgress } from "./overviewProgressStore"

export function sidebarProgress(
  view: MainViewId,
  progress: OverviewProgress,
  allowance: AllowanceUsageSummaryPayload | null,
): Pick<SidebarNavItem, "status"> {
  switch (view) {
    case "agents":
      return { status: String(progress.agents.rows.filter((row) => row.sessions > 0).length) }
    case "quota":
      return {
        status: (allowance?.accounts ?? [])
          .flatMap((account) =>
            account.utilization == null
              ? []
              : [`${Math.round(account.utilization.utilizationPercent)}%`],
          )
          .join(" / "),
      }
    case "activity":
      return { status: progress.sessions.displayCompleted.toLocaleString() }
    case "burnChecks":
      return { status: `${progress.failingCount} to fix` }
    default:
      return {}
  }
}
