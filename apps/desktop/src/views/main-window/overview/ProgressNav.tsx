import { useId, useRef, useState, useSyncExternalStore, type RefObject } from "react"
import { createPortal } from "react-dom"

import { CountUp } from "../../../components/ui/CountUp"
import { checksConfiguredStore } from "../../../lib/checkAvailability"
import { cn } from "../../../lib/cn"
import type { BurnCheckDetectorId } from "../../../lib/insightsIpc"
import { enabledCheckCount } from "../../../lib/presentation/checkDefinitions"
import { SettingsTargetFocus } from "../../settings/SettingsTargetFocus"
import { PRIMARY_BUTTON, ProgressStepCard } from "./ProgressSteps"
import {
  closeProgressStep,
  firstFailingCheck,
  fixesFound,
  openProgressStep,
  type OverviewProgress,
  type ProgressStepKey,
  rewindTo,
  stepDocked,
} from "./overviewProgressStore"
import { useOverviewProgress } from "./useOverviewProgress"
import { StepSettings } from "./stepSettings/StepSettings"

const STEPS: readonly ProgressStepKey[] = ["agents", "sessions", "checks", "fixes"]

const STEP_LABELS: Record<ProgressStepKey, string> = {
  agents: "Agents",
  sessions: "Sessions",
  checks: "Checks",
  fixes: "Fixes",
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
  triggerRefs,
}: {
  step: ProgressStepKey
  progress: OverviewProgress
  triggerRefs: RefObject<Map<ProgressStepKey, HTMLButtonElement>>
}) {
  const configured = useSyncExternalStore(
    checksConfiguredStore.subscribe,
    checksConfiguredStore.getSnapshot,
  )
  const label = STEP_LABELS[step]
  const { value, pulsing } = rowContent(step, progress, enabledCheckCount(configured))
  const open = progress.openStep === step
  // During the first run a row takes the reader back to its step. Once the
  // first run is done, it opens the step's modal.
  const rewinds = progress.mode === "firstRun" && progress.flow !== "done"
  return (
    <button
      ref={(node) => {
        if (node) triggerRefs.current.set(step, node)
        else triggerRefs.current.delete(step)
      }}
      type="button"
      aria-haspopup={rewinds ? undefined : "dialog"}
      aria-expanded={rewinds ? undefined : open}
      onClick={() => (rewinds ? rewindTo(step) : openProgressStep(step))}
      className={cn(
        "type-body flex h-9 w-full cursor-pointer! items-center justify-between gap-3 rounded-control px-3 transition-colors duration-[var(--duration-fast)] ease-out",
        open ? "bg-surface-selected text-label" : "text-label hover:bg-surface-hover",
      )}
    >
      <span className="truncate">{label}</span>
      <span
        className={cn(
          "shrink-0 font-mono type-caption tabular-nums text-label-secondary",
          pulsing && "animate-pulse",
        )}
      >
        <CountUp value={value} />
      </span>
    </button>
  )
}

/**
 * One step's modal: the same step card the takeover shows, with steady
 * values and no Next or Done button. Follows the burn-checks dialog
 * pattern: focus moves in and is trapped, Esc and a backdrop click close
 * it, and focus returns to the row that opened it.
 */
function ProgressStepModal({
  step,
  progress,
  returnFocus,
  onOpenChecks,
}: {
  step: ProgressStepKey
  progress: OverviewProgress
  returnFocus: () => void
  onOpenChecks: (check: BurnCheckDetectorId | undefined) => void
}) {
  const titleId = useId()
  const [targetFocus] = useState(() => new SettingsTargetFocus())
  const close = () => {
    closeProgressStep()
    queueMicrotask(returnFocus)
  }
  return createPortal(
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-surface-window/80 p-6 backdrop-blur-sm"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) close()
      }}
    >
      <section
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        onKeyDown={(event) => {
          if (event.key === "Escape") return close()
          if (event.key !== "Tab") return
          const controls = Array.from(
            event.currentTarget.querySelectorAll<HTMLElement>("button:not([disabled])"),
          )
          const first = controls[0]
          const last = controls.at(-1)
          if (event.shiftKey && document.activeElement === first) {
            event.preventDefault()
            last?.focus()
          } else if (!event.shiftKey && document.activeElement === last) {
            event.preventDefault()
            first?.focus()
          }
        }}
        className="flex max-h-[calc(100vh-3rem)] w-full max-w-2xl flex-col overflow-hidden rounded-control border border-separator bg-surface-card text-label shadow-raised"
      >
        <h4 id={titleId} className="sr-only">
          {STEP_LABELS[step]}
        </h4>
        {/* Only this area scrolls: the summary card, then the step's own
            settings, open by default. The footer below stays put. */}
        <div className="flex min-h-0 flex-1 flex-col items-center gap-(--space-lg) overflow-y-auto p-5">
          <ProgressStepCard
            step={step}
            surface="modal"
            progress={progress}
            isSteady={progress.mode === "steady"}
            transitionName={undefined}
          />
          <div
            ref={(node) =>
              node
                ? targetFocus.attach(
                    node,
                    step,
                    progress.openStepControl,
                    progress.openStepControlRevision,
                  )
                : undefined
            }
            className="flex w-full flex-col gap-(--space-md)"
          >
            <StepSettings step={step} />
          </div>
        </div>
        <div className="flex shrink-0 items-center justify-center gap-(--space-sm) border-t border-separator p-3">
          {step === "fixes" && fixesFound(progress) && (
            <button
              type="button"
              onClick={() => {
                const check = firstFailingCheck(progress)
                closeProgressStep()
                onOpenChecks(check)
              }}
              className={PRIMARY_BUTTON}
            >
              Enhance
            </button>
          )}
          <button type="button" autoFocus onClick={close} className="ui-push-button">
            Close
          </button>
        </div>
      </section>
    </div>,
    document.body,
  )
}

/** Renders in the `SidebarNav` footer, above Settings. Not part of the
 *  navigation registry or search: these rows are status, not views. */
export function ProgressNav({
  onOpenChecks,
}: {
  onOpenChecks: (check: BurnCheckDetectorId | undefined) => void
}) {
  const progress = useOverviewProgress()
  const triggerRefs = useRef(new Map<ProgressStepKey, HTMLButtonElement>())
  if (progress.mode === "pending") return null

  const docked = STEPS.filter((step) => stepDocked(progress.flow, step))
  if (docked.length === 0) return null

  const openStep = progress.openStep

  return (
    <>
      {docked.length > 0 && (
        <>
          <div className="flex flex-col gap-2">
            {docked.map((step) => (
              <ProgressRow
                key={step}
                step={step}
                progress={progress}
                triggerRefs={triggerRefs}
              />
            ))}
          </div>

          <div className="my-2 h-px bg-separator" />
        </>
      )}

      {openStep && (
        <ProgressStepModal
          step={openStep}
          progress={progress}
          returnFocus={() => triggerRefs.current.get(openStep)?.focus()}
          onOpenChecks={onOpenChecks}
        />
      )}
    </>
  )
}
