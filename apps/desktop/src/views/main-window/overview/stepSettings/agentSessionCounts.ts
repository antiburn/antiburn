import type { AgentFoundCount } from "../../../../lib/ipc"
import { scanStatusStore } from "../../../../lib/scanStatusStore"
import { advanceLastPass, INITIAL_LAST_PASS, type LastPass } from "../overviewProgressStore"

/**
 * Session counts per agent, for the agent list in the first run and in
 * Settings → Agents. Reads only the shared scan status, so the Settings
 * window does not start the Overview progress store, its main-window
 * commands, or its first-run analytics.
 */
const NO_COUNTS: readonly AgentFoundCount[] = []

let lastPass: LastPass = INITIAL_LAST_PASS
let lastStatus = scanStatusStore.getSnapshot()

export const subscribeAgentSessionCounts = scanStatusStore.subscribe

// Keep the last completed pass, so a routine scan does not reset the counts
// to zero while it runs.
export function agentSessionCounts(): readonly AgentFoundCount[] {
  const status = scanStatusStore.getSnapshot()
  if (status !== lastStatus) {
    lastStatus = status
    lastPass = advanceLastPass(lastPass, status)
  }
  return lastPass.lastFound ?? status?.foundByAgent ?? NO_COUNTS
}
