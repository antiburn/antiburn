import { ArrowRight } from "lucide-react"
import { useCallback, useId, useRef } from "react"

import { startFire, type FireHandle } from "./enhanceFire"
import type { EnhanceButtonState } from "./enhanceState"

function cardFace(state: EnhanceButtonState): {
  /** The accessible name of the card. */
  label: string
  action: string
  calm: boolean
} {
  switch (state.kind) {
    case "new":
      return {
        label: `Enhance again · ${state.count} new`,
        action: `Enhance again · ${state.count} new`,
        calm: false,
      }
    case "resume":
      return {
        label: `Continue setup · step ${state.step} of 5`,
        action: `Continue · step ${state.step} of 5`,
        calm: true,
      }
    case "watching":
      return {
        label: "Check your fixes",
        action: "Check your fixes",
        calm: true,
      }
    case "clear":
      return {
        label: "All set · run again",
        action: "Run again",
        calm: true,
      }
    case "loading":
    case "fresh":
      return { label: "Enhance my AI setup", action: "Enhance", calm: false }
  }
}

/** The call to action under Recent sessions. It gives the reader one clear
 *  next step: open the Enhance wizard. It is not a card: the install banner's
 *  fire burns behind centred text and a pill, and moves only while the
 *  pointer is on it. */
export function EnhanceActionBar({
  failingChecks,
  state,
  onOpen,
}: {
  /** Failing checks that are not snoozed, or null while the report loads. */
  failingChecks: number | null
  state: EnhanceButtonState
  onOpen: () => void
}) {
  const face = cardFace(state)
  const fire = useRef<FireHandle | null>(null)
  const statusId = useId()
  const mountFire = useCallback((canvas: HTMLCanvasElement | null) => {
    if (!canvas) return
    const handle = startFire(canvas)
    if (!handle) return
    fire.current = handle
    return () => {
      handle.dispose()
      fire.current = null
    }
  }, [])
  return (
    <button
      type="button"
      aria-label={face.label}
      aria-describedby={statusId}
      onClick={onOpen}
      onPointerEnter={() => fire.current?.play()}
      onPointerLeave={() => fire.current?.pause()}
      onFocus={() => fire.current?.play()}
      onBlur={() => fire.current?.pause()}
      data-calm={face.calm ? "" : undefined}
      // Auto margins float it to the middle of the space under Recent sessions.
      className="enhance-fire-card group my-auto flex w-full shrink-0 flex-col items-center gap-(--space-lg) rounded-(--radius-popover) px-(--space-2xl) py-(--space-2xl) text-center"
    >
      <canvas
        ref={mountFire}
        aria-hidden="true"
        data-fuel={face.calm ? 0.55 : 1}
        className="enhance-fire-canvas"
      />
      <span className="flex min-w-0 flex-col gap-(--space-xs)">
        <span aria-hidden="true" className="type-title-3 font-semibold text-label">
          Enhance my AI setup
        </span>
        <span id={statusId} className="type-callout text-label-secondary">
          {failingChecks == null ? (
            "Checking your setup…"
          ) : failingChecks === 0 ? (
            <>
              <span className="font-semibold text-label">No fixes needed</span> in your last 30
              days.
            </>
          ) : (
            <>
              <span className="font-semibold text-label">
                {failingChecks} {failingChecks === 1 ? "fix" : "fixes"} found
              </span>{" "}
              in your last 30 days.
            </>
          )}
        </span>
      </span>
      <span
        data-quiet={face.calm ? "" : undefined}
        className="enhance-pill flex shrink-0 items-center gap-(--space-sm) px-(--space-2xl) type-headline whitespace-nowrap"
      >
        {face.action}
        <ArrowRight
          aria-hidden="true"
          size={15}
          className="transition-transform duration-(--duration-medium) group-hover:translate-x-0.5"
        />
      </span>
    </button>
  )
}
