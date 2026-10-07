// Shared copy for the scan-status rows the Sessions step settings show: how
// large the local index is, and what the historical pass did.

import type { ScanHistoryProgress } from "../ipc"

/** A byte count at a readable scale. Two significant places is enough for a
 *  settings row: the question being answered is "is this large?". */
export function byteLabel(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 KB"
  const units = ["KB", "MB", "GB"]
  let value = bytes / 1024
  let unit = 0
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024
    unit += 1
  }
  return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`
}

/**
 * What the "Older sessions" row says about the historical pass. `completed`
 * also counts sessions that failed or were unsupported, so the copy says
 * "processed", not "read".
 */
export function olderSessionsStatus(
  history: ScanHistoryProgress | undefined,
  monitoringPaused: boolean,
): string {
  if (!history) return ""
  switch (history.state) {
    case "none":
      return "Keep session data covers only the last 30 days"
    case "pending":
      return monitoringPaused ? "Starts when monitoring resumes" : "Starts once checks finish"
    case "running":
      return history.passRunning
        ? "Looking for older sessions…"
        : `${history.completed} of ${history.total} processed`
    case "done":
      return history.total > 0
        ? `${history.total} older ${history.total === 1 ? "session" : "sessions"} processed`
        : "No older sessions found"
  }
}
