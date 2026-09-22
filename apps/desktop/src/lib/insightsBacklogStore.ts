/**
 * Whether the insights worker pool has a backlog to drain right now.
 *
 * The Overview reads this to throttle its event-driven reads while the
 * worker pool is busy (see `OVERVIEW_BACKLOG_THROTTLE_MS` in
 * `MainOverviewSession.ts`).
 */

import { createExternalStore } from "./externalStore"
import { getInsightsBacklog, onInsightsBacklogChanged, type InsightsBacklog } from "./ipc"

export const insightsBacklogStore = createExternalStore<InsightsBacklog | null>({
  initial: null,
  load: () => getInsightsBacklog().catch(() => null),
  subscribe: (set) => onInsightsBacklogChanged(set),
})
