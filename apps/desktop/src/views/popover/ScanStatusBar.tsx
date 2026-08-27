import { AlertTriangle, Pause, Play, RefreshCw } from "lucide-react"

import { relativeTime } from "../../lib/presentation/relativeTime"
import type { ScanStatus } from "../../lib/ipc"

/**
 * What the local index knows and when it last looked.
 *
 * A local-first app whose whole value is "these are the sessions on your
 * machine" has to answer two questions on sight: is it looking, and how old is
 * what I am reading? Without this line a stale list and a fresh one are
 * indistinguishable, and the first-run wait — the longest scan the app ever
 * runs — looks like an empty app.
 *
 * The three controls next to it are the ones that line implies: run a pass now,
 * stop the one running, and stop antiburn scanning on its own.
 */

export interface ScanStatusBarProps {
  /** The shell's latest status, or null before the first read resolves. */
  status: ScanStatus | null
  /** Whether background discovery is paused. */
  paused: boolean
  onRescan: () => void
  onCancel: () => void
  onTogglePaused: (paused: boolean) => void
}

/** The sentence the bar leads with. */
export function scanStatusLabel(status: ScanStatus | null, paused: boolean): string {
  if (status?.running) {
    const { completedAgents, totalAgents } = status
    // Before the first agent reports there is no ratio worth printing.
    if (totalAgents > 0 && completedAgents > 0) {
      return `Scanning… ${completedAgents} of ${totalAgents} agents`
    }
    return "Scanning…"
  }
  if (status?.error) return "Last scan did not finish"
  if (paused) return "Scanning paused"
  if (status?.cancelled) return "Last scan was stopped"
  if (status?.finishedAt) return `Scanned ${relativeTime(status.finishedAt)}`
  return "Not scanned yet"
}

export function ScanStatusBar({
  status,
  paused,
  onRescan,
  onCancel,
  onTogglePaused,
}: ScanStatusBarProps) {
  const running = status?.running ?? false
  const failed = !running && !!status?.error
  const label = scanStatusLabel(status, paused)

  return (
    <div
      // Polite rather than assertive: a scan finishing is worth knowing, not
      // worth interrupting whatever is being read.
      aria-live="polite"
      data-testid="scan-status"
      className="flex h-7 shrink-0 items-center gap-1 px-4 pb-1"
    >
      {failed && (
        <AlertTriangle
          size={11}
          strokeWidth={2}
          aria-hidden="true"
          className="shrink-0 text-system-orange"
        />
      )}
      <p className="min-w-0 flex-1 truncate type-caption text-label-tertiary">{label}</p>

      {running ? (
        <button
          type="button"
          onClick={onCancel}
          className="shrink-0 rounded-control px-1.5 py-0.5 type-caption text-label-secondary hover:bg-surface-hover"
        >
          Stop
        </button>
      ) : (
        <button
          type="button"
          onClick={onRescan}
          aria-label="Scan now"
          className="inline-flex h-5 shrink-0 items-center gap-1 rounded-control px-1.5 type-caption text-label-secondary hover:bg-surface-hover"
        >
          <RefreshCw size={10} strokeWidth={2} aria-hidden="true" />
          Rescan
        </button>
      )}

      {/* Pause is a statement about background work only: an explicit rescan
          still runs, and everything already indexed stays readable. The label
          says which of the two states pressing it produces. */}
      <button
        type="button"
        onClick={() => onTogglePaused(!paused)}
        aria-label={paused ? "Resume background scanning" : "Pause background scanning"}
        className="inline-flex h-5 w-5 shrink-0 items-center justify-center rounded-control text-label-tertiary hover:bg-surface-hover hover:text-label-secondary"
      >
        {paused ? (
          <Play size={10} strokeWidth={2.5} aria-hidden="true" />
        ) : (
          <Pause size={10} strokeWidth={2.5} aria-hidden="true" />
        )}
      </button>
    </div>
  )
}
