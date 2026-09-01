/**
 * The scan's current status, shared by every settings pane that reports on
 * it (General, Sources).
 *
 * A single module-level store rather than one per pane: the two panes read
 * the identical shell state and never mount at the same time (`SettingsView`
 * renders one pane at a time), so there is nothing to keep separate and one
 * subscription is one fewer place to drift from the other.
 */

import { createExternalStore } from "./externalStore"
import { getScanStatus, onScanEvent, type ScanStatus } from "./ipc"

export const scanStatusStore = createExternalStore<ScanStatus | null>({
  initial: null,
  load: () => getScanStatus().catch(() => null),
  subscribe: (set) => onScanEvent((status) => set(withKnownAgents(status))),
})

/**
 * Keep the per-agent list across updates that do not carry one. Scan events
 * and `scan_now` return an empty `agents` array by design; only
 * `get_scan_status` fills it. Use for every write into `scanStatusStore`.
 */
export function withKnownAgents(status: ScanStatus): ScanStatus {
  const previous = scanStatusStore.getSnapshot()
  if (status.agents.length > 0 || !previous || previous.agents.length === 0) return status
  return { ...status, agents: previous.agents }
}
