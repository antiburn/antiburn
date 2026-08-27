import { useSyncExternalStore } from "react"

import { createExternalStore } from "../../../lib/externalStore"

/**
 * Whether the Cost card's "N sub-agents" row shows its roster.
 *
 * One flag for the whole app, not one per session: a reader who opens the
 * roster expects it to stay open while they move between sessions, so the
 * choice lives here instead of in `SubagentsSplitRow`'s own state, which a
 * remount of the detail panel would otherwise reset.
 *
 * In-memory only. The choice does not need to survive a restart, so there is
 * nothing to load and nothing to subscribe to.
 */
export const subagentsExpandedStore = createExternalStore<boolean>({
  initial: false,
})

/** Read the current expanded state, and re-render when it changes. */
export function useSubagentsExpanded(): boolean {
  return useSyncExternalStore(
    subagentsExpandedStore.subscribe,
    subagentsExpandedStore.getSnapshot,
  )
}

/** Flip the expanded state. */
export function toggleSubagentsExpanded(): void {
  subagentsExpandedStore.set(!subagentsExpandedStore.getSnapshot())
}
