import { ArrowRight } from "lucide-react"
import { useId, type ReactNode } from "react"

import type { EnhanceButtonState } from "./enhanceState"

function pillFace(state: EnhanceButtonState): {
  /** The accessible name of the pill. */
  label: string
  action: string
  calm: boolean
} {
  switch (state.kind) {
    case "new":
      return {
        label: `Optimise again · ${state.count} new`,
        action: `Optimise again · ${state.count} new`,
        calm: false,
      }
    case "resume":
      return {
        label: `Continue setup · step ${state.step} of 5`,
        action: `Continue · step ${state.step} of 5`,
        calm: true,
      }
    case "watching":
      return { label: "Check your fixes", action: "Check your fixes", calm: true }
    case "clear":
      return { label: "All set · run again", action: "Run again", calm: true }
    case "loading":
    case "fresh":
      return { label: "Optimise my AI setup", action: "Optimise", calm: false }
  }
}

/** The Optimise card on the Overview. A plain card with the call to action
 *  on top and the usage chart under it. The pill opens the Optimise wizard. */
export function EnhanceBanner({
  failingChecks,
  state,
  onOpen,
  children,
}: {
  /** Failing checks that are not snoozed, or null while the report loads. */
  failingChecks: number | null
  state: EnhanceButtonState
  onOpen: () => void
  /** The content under the call to action. */
  children?: ReactNode
}) {
  const face = pillFace(state)
  const statusId = useId()
  return (
    <section
      aria-label="Optimise"
      className="enhance-banner my-auto flex shrink-0 flex-col gap-(--space-2xl) rounded-(--radius-popover) p-(--space-2xl)"
    >
      <div className="flex items-center justify-between gap-(--space-2xl)">
        <div className="flex min-w-0 flex-col gap-(--space-xs)">
          <h2 className="type-title-2 font-semibold text-label">Optimise my AI setup</h2>
          <p id={statusId} className="type-body text-label-secondary">
            {failingChecks == null ? (
              "Checking your setup…"
            ) : failingChecks === 0 ? (
              <>
                <span className="font-semibold text-label">No fixes needed</span> in your last
                30 days.
              </>
            ) : (
              <>
                <span className="font-semibold text-label">
                  {failingChecks} {failingChecks === 1 ? "fix" : "fixes"} found
                </span>{" "}
                in your last 30 days.
              </>
            )}
          </p>
        </div>
        <button
          type="button"
          aria-label={face.label}
          aria-describedby={statusId}
          onClick={onOpen}
          data-quiet={face.calm ? "" : undefined}
          className="enhance-pill group flex shrink-0 items-center gap-(--space-sm) px-(--space-2xl) type-title-3 font-semibold whitespace-nowrap"
        >
          {face.action}
          <ArrowRight
            aria-hidden="true"
            size={17}
            className="transition-transform duration-(--duration-medium) group-hover:translate-x-0.5"
          />
        </button>
      </div>
      {children}
    </section>
  )
}
