/**
 * The Quota screen's own view controls a reader last chose: the account,
 * lane, range preset, axis mode, and pace-line switch. Persisted to
 * `localStorage`, the same pattern `overlayWindow.ts` uses for the floating
 * HUD preference, so the screen reopens the way the reader left it and the
 * choice survives an app restart.
 *
 * A custom range from a session-detail link never lands here: only a fixed
 * `QuotaRangePreset` is worth restoring on a later visit.
 */

import type { QuotaChartAxisMode } from "./QuotaBurnupChart"
import type { QuotaRangePreset } from "./quotaSeries"

const QUOTA_VIEW_PREFS_KEY = "antiburn.quota.view.v1"

export interface QuotaViewPrefs {
  provider?: string
  accountKey?: string
  lane?: string
  rangePreset?: QuotaRangePreset
  axisMode?: QuotaChartAxisMode
  showPace?: boolean
}

/** The saved Quota view controls, or an empty object with no saved value, no
 *  storage, or a stored value that is not a plain object. */
export function readQuotaViewPrefs(): QuotaViewPrefs {
  try {
    const raw = localStorage.getItem(QUOTA_VIEW_PREFS_KEY)
    if (!raw) return {}
    const parsed: unknown = JSON.parse(raw)
    return parsed != null && typeof parsed === "object" && !Array.isArray(parsed)
      ? (parsed as QuotaViewPrefs)
      : {}
  } catch {
    return {}
  }
}

/** Merge `partial` into the saved Quota view controls. */
export function writeQuotaViewPrefs(partial: QuotaViewPrefs): void {
  try {
    localStorage.setItem(
      QUOTA_VIEW_PREFS_KEY,
      JSON.stringify({ ...readQuotaViewPrefs(), ...partial }),
    )
  } catch {
    // The screen still works when preference storage is unavailable.
  }
}
