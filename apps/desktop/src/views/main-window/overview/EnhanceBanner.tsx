import { ArrowRight } from "lucide-react"
import { useId } from "react"

import { startSmoke } from "./enhanceSmoke"
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

/** Draws LED smoke on the canvas while it is mounted. */
function mountSmoke(canvas: HTMLCanvasElement | null) {
  if (!canvas) return
  return startSmoke(canvas) ?? undefined
}

/** The Optimise band across the top of the Overview. It gives the reader one
 *  clear next step: open the Optimise wizard. A calm state drops the brand
 *  fill, so the band steps back once the work is done. */
export function EnhanceBanner({
  failingChecks,
  state,
  onOpen,
}: {
  /** Failing checks that are not snoozed, or null while the report loads. */
  failingChecks: number | null
  state: EnhanceButtonState
  onOpen: () => void
}) {
  const face = pillFace(state)
  const statusId = useId()
  return (
    <section
      aria-label="Optimise"
      data-calm={face.calm ? "" : undefined}
      className="enhance-banner flex shrink-0 items-center justify-between gap-(--space-2xl) rounded-(--radius-popover) px-(--space-2xl) py-(--space-2xl)"
    >
      {!face.calm && (
        <canvas ref={mountSmoke} aria-hidden="true" className="enhance-banner-smoke" />
      )}
      <div className="flex min-w-0 flex-col gap-(--space-xs)">
        <h2 className="type-title-2 font-semibold">Optimise my AI setup</h2>
        <p id={statusId} className="enhance-banner-status type-body">
          {failingChecks == null ? (
            "Checking your setup…"
          ) : failingChecks === 0 ? (
            <>
              <span className="font-semibold">No fixes needed</span> in your last 30 days.
            </>
          ) : (
            <>
              <span className="font-semibold">
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
        data-invert={face.calm ? undefined : ""}
        className="enhance-pill group flex shrink-0 items-center gap-(--space-sm) px-(--space-2xl) type-title-3 font-semibold whitespace-nowrap"
      >
        {face.action}
        <ArrowRight
          aria-hidden="true"
          size={17}
          className="transition-transform duration-(--duration-medium) group-hover:translate-x-0.5"
        />
      </button>
    </section>
  )
}
