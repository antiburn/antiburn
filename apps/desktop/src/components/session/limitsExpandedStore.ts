import { useSyncExternalStore } from "react"

import { createExternalStore } from "../../lib/externalStore"

/**
 * Whether the Cost tab's Limits section shows its rows.
 *
 * One flag for the whole app, not one per session: a reader who opens this
 * section expects it to stay open while they move between sessions, so the
 * choice lives here instead of in `SessionQuotaSection`'s own state, which a
 * remount of the detail panel would otherwise reset.
 *
 * In-memory only. The choice does not need to survive a restart, so there is
 * nothing to load and nothing to subscribe to.
 */
export const limitsExpandedStore = createExternalStore<boolean>({
  initial: false,
})

/** Read the current expanded state, and re-render when it changes. */
export function useLimitsExpanded(): boolean {
  return useSyncExternalStore(limitsExpandedStore.subscribe, limitsExpandedStore.getSnapshot)
}

/** Flip the expanded state. */
export function toggleLimitsExpanded(): void {
  limitsExpandedStore.set(!limitsExpandedStore.getSnapshot())
}
