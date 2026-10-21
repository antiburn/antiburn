import { createClaudePluginStore } from "../../lib/claudePluginStore"
import {
  overviewProgress,
  subscribeOverviewProgress,
  type OverviewProgress,
} from "./overview/overviewProgressStore"

/** Claude Code sessions the discovery pass found in local logs. */
export function claudeSessionsFrom(progress: OverviewProgress): number {
  return progress.agents.rows.find((row) => row.agent === "claude-code")?.sessions ?? 0
}

/**
 * The main window's one plugin store. Its sidebar card and Agents panel share
 * it, so a dismissal or a status refresh reaches both at once. This window
 * alone reports `installed_detected`.
 */
export const mainClaudePluginStore = createClaudePluginStore({
  reportInstalled: true,
  sessions: {
    read: () => claudeSessionsFrom(overviewProgress()),
    subscribe: subscribeOverviewProgress,
  },
})
