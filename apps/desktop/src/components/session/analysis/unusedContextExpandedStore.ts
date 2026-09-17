import { useSyncExternalStore } from "react"

import { createExternalStore } from "../../../lib/externalStore"

/**
 * Whether the Cost tab's unused skills, MCP servers, and built-in tools
 * section shows its rows.
 *
 * One flag for the whole app, not one per session: a reader who opens this
 * section expects it to stay open while they move between sessions, so the
 * choice lives here instead of in `UnusedContext`'s own state, which a
 * remount of the detail panel would otherwise reset.
 *
 * In-memory only. The choice does not need to survive a restart, so there is
 * nothing to load and nothing to subscribe to.
 */
export const unusedContextExpandedStore = createExternalStore<boolean>({
  initial: false,
})

/** Read the current expanded state, and re-render when it changes. */
export function useUnusedContextExpanded(): boolean {
  return useSyncExternalStore(
    unusedContextExpandedStore.subscribe,
    unusedContextExpandedStore.getSnapshot,
  )
}

/** Flip the expanded state. */
export function toggleUnusedContextExpanded(): void {
  unusedContextExpandedStore.set(!unusedContextExpandedStore.getSnapshot())
}
