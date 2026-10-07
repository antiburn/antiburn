import { useSyncExternalStore } from "react"

import { CountUp } from "../../../components/ui/CountUp"
import { checksConfiguredStore } from "../../../lib/checkAvailability"
import { cn } from "../../../lib/cn"
import { enabledCheckCount } from "../../../lib/presentation/checkDefinitions"
import type { BurnCheckDetectorId } from "../../../lib/insightsIpc"
import {
  firstFailingCheck,
  type OverviewProgress,
  type ProgressStepKey,
  rewindTo,
  stepDocked,
} from "./overviewProgressStore"
import { useOverviewProgress } from "./useOverviewProgress"

const STEPS: readonly ProgressStepKey[] = ["agents", "sessions", "checks", "fixes"]

const STEP_LABELS: Record<ProgressStepKey, string> = {
  agents: "Agents",
  sessions: "Sessions",
  checks: "Checks",
  fixes: "To fix",
}

function rowContent(
  step: ProgressStepKey,
  progress: OverviewProgress,
  enabledChecks: number,
): { value: number; pulsing: boolean } {
  switch (step) {
    case "agents": {
      const total = progress.agents.rows.reduce(
        (sum, row) => sum + (row.sessions > 0 ? 1 : 0),
        0,
      )
      return { value: total, pulsing: !progress.agents.done }
    }
    case "sessions": {
      const { done, displayCompleted, total } = progress.sessions
      // Before the 30-day read is done, the row shows that read's own total,
      // which climbs as discovery finds sessions. Once it's done, the row
      // shows the combined figure instead, which climbs as the background
      // history pass reads sessions older than the 30-day window.
      const historyActive =
        progress.history?.state === "looking" || progress.history?.state === "reading"
      return {
        value: done ? displayCompleted : total,
        pulsing: !done || historyActive,
      }
    }
    case "checks":
      return { value: enabledChecks, pulsing: !progress.checks.done }
    case "fixes": {
      return { value: progress.failingCount, pulsing: false }
    }
  }
}

function ProgressRow({
  step,
  progress,
  onOpen,
}: {
  step: ProgressStepKey
  progress: OverviewProgress
  onOpen: () => void
}) {
  const configured = useSyncExternalStore(
    checksConfiguredStore.subscribe,
    checksConfiguredStore.getSnapshot,
  )
  const label = STEP_LABELS[step]
  const { value, pulsing } = rowContent(step, progress, enabledCheckCount(configured))
  // During the first run a row takes the reader back to its step. Once the
  // first run is done, it opens the step's destination.
  const rewinds = progress.mode === "firstRun" && progress.flow !== "done"
  return (
    <button
      type="button"
      onClick={() => (rewinds ? rewindTo(step) : onOpen())}
      className="type-caption flex h-7 cursor-pointer! items-center justify-between gap-1.5 rounded-full bg-surface-card px-2.5 text-label transition-colors duration-fast ease-out hover:bg-surface-secondary"
    >
      <span className="truncate">{label}</span>
      <span
        className={cn(
          "shrink-0 font-mono tabular-nums text-label-secondary",
          pulsing && "animate-pulse",
        )}
      >
        <CountUp value={value} />
      </span>
    </button>
  )
}

/** Renders in the `SidebarNav` footer, below Settings. Not part of the
 *  navigation registry or search: these pills are status, not views. Agents,
 *  Sessions, and Checks open their Settings pane. To fix opens Checks at the
 *  first check that needs a fix. */
export function ProgressNav({
  onOpenSettings,
  onOpenFixes,
}: {
  onOpenSettings: (step: Exclude<ProgressStepKey, "fixes">) => void
  onOpenFixes: (check: BurnCheckDetectorId | undefined) => void
}) {
  const progress = useOverviewProgress()
  if (progress.mode === "pending") return null

  const docked = STEPS.filter((step) => stepDocked(progress.flow, step))
  if (docked.length === 0) return null

  return (
    <>
      {/* Shortcut pills in two equal columns under Settings. */}
      <div className="mt-2 grid grid-cols-2 gap-1.5 px-1">
        {docked.map((step) => (
          <ProgressRow
            key={step}
            step={step}
            progress={progress}
            onOpen={() =>
              step === "fixes" ? onOpenFixes(firstFailingCheck(progress)) : onOpenSettings(step)
            }
          />
        ))}
      </div>
    </>
  )
}
