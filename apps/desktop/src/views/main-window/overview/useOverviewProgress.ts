import { useSyncExternalStore } from "react"

import type { BurnCheckDetectorId } from "../../../lib/insightsIpc"
import { snoozedDetectorIds, useSnoozedBurnChecks } from "../../../lib/snoozedBurnChecks"
import {
  overviewProgress,
  subscribeOverviewProgress,
  type OverviewProgress,
} from "./overviewProgressStore"

/**
 * Marks each snoozed check as `"snoozed"` and removes it from the failing
 * count, as the Checks page does. The progress store does not read snoozes.
 */
export function withSnoozes(
  progress: OverviewProgress,
  snoozed: ReadonlySet<BurnCheckDetectorId>,
): OverviewProgress {
  if (snoozed.size === 0) return progress
  const categories = progress.categories.map((category) =>
    snoozed.has(category.id) ? { ...category, status: "snoozed" as const } : category,
  )
  return {
    ...progress,
    categories,
    failingCount: categories.filter((category) => category.status === "needsFix").length,
  }
}

/** The Overview's progress, with snoozed checks applied. */
export function useOverviewProgress(): OverviewProgress {
  const progress = useSyncExternalStore(
    subscribeOverviewProgress,
    overviewProgress,
    overviewProgress,
  )
  const snoozes = useSnoozedBurnChecks()
  return withSnoozes(progress, snoozedDetectorIds(snoozes.records))
}
