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
  subscribe: (set) => onScanEvent((status) => set(status)),
})
