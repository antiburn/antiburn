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
 * Until the snoozes are known, a failing check can be a snoozed one, so each
 * failing check shows as not checked and the failing count is zero.
 */
export function withSnoozes(
  progress: OverviewProgress,
  snoozed: ReadonlySet<BurnCheckDetectorId>,
  snoozesKnown = true,
): OverviewProgress {
  if (!snoozesKnown) {
    if (progress.failingCount === 0) return progress
    return {
      ...progress,
      categories: progress.categories.map((category) =>
        category.status === "needsFix"
          ? { ...category, status: "notChecked" as const }
          : category,
      ),
      failingCount: 0,
    }
  }
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
  // A failed refresh keeps the last records, so records also count as known.
  const snoozesKnown = snoozes.status === "ready" || snoozes.records.length > 0
  return withSnoozes(progress, snoozedDetectorIds(snoozes.records), snoozesKnown)
}
