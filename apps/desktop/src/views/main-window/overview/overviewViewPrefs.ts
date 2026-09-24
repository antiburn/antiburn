/**
 * The Overview's own view controls a reader last chose: the Cost/Subscription
 * unit and the selected provider tab. Persisted to `localStorage`, the same
 * pattern `quotaViewPrefs.ts` uses for the Quota screen, so the Overview
 * reopens the way the reader left it and the choice survives an app restart.
 *
 * It also remembers one thing the reader never chose: whether they turned out
 * to have a subscription plan. That answer decides which unit the page opens
 * on, and it cannot be worked out until the first read returns, so without a
 * memory of it the page has to guess and then correct itself in front of the
 * reader.
 */

const OVERVIEW_VIEW_PREFS_KEY = "antiburn.overview.view.v1"

export type OverviewMetric = "cost" | "allowance"

export interface OverviewViewPrefs {
  metric?: OverviewMetric
  accountTabKey?: string
  /** Whether the last run found a subscription plan on any account. */
  hadSubscriptionPlan?: boolean
}

/** The saved Overview view controls, or an empty object with no saved value,
 *  no storage, or a stored value that is not a plain object. */
export function readOverviewViewPrefs(): OverviewViewPrefs {
  try {
    const raw = localStorage.getItem(OVERVIEW_VIEW_PREFS_KEY)
    if (!raw) return {}
    const parsed: unknown = JSON.parse(raw)
    return parsed != null && typeof parsed === "object" && !Array.isArray(parsed)
      ? (parsed as OverviewViewPrefs)
      : {}
  } catch {
    return {}
  }
}

/** Merge `partial` into the saved Overview view controls. */
export function writeOverviewViewPrefs(partial: OverviewViewPrefs): void {
  try {
    localStorage.setItem(
      OVERVIEW_VIEW_PREFS_KEY,
      JSON.stringify({ ...readOverviewViewPrefs(), ...partial }),
    )
  } catch {
    // The screen still works when preference storage is unavailable.
  }
}
