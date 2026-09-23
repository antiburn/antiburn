import { ArrowRight, CircleCheck, Crosshair, Hourglass, type LucideIcon } from "lucide-react"
import { useCallback, useId, useRef } from "react"

import { cn } from "../../../lib/cn"
import { startFire, type FireHandle } from "./enhanceFire"
import type { EnhanceButtonState } from "./enhanceState"

function cardFace(state: EnhanceButtonState): {
  /** The accessible name of the card. */
  label: string
  action: string
  Icon: LucideIcon
  calm: boolean
} {
  switch (state.kind) {
    case "new":
      return {
        label: `Enhance again · ${state.count} new`,
        action: `Enhance again · ${state.count} new`,
        Icon: Crosshair,
        calm: false,
      }
    case "resume":
      return {
        label: `Continue setup · step ${state.step} of 5`,
        action: `Continue · step ${state.step} of 5`,
        Icon: Crosshair,
        calm: true,
      }
    case "watching":
      return {
        label: "Check your fixes",
        action: "Check your fixes",
        Icon: Hourglass,
        calm: true,
      }
    case "clear":
      return {
        label: "All set · run again",
        action: "Run again",
        Icon: CircleCheck,
        calm: true,
      }
    case "loading":
    case "fresh":
      return { label: "Enhance my AI setup", action: "Enhance", Icon: Crosshair, calm: false }
  }
}

/** The card under Recent sessions. It gives the reader one clear next step:
 *  open the Enhance wizard. The install banner's fire burns behind it, and
 *  moves only while the pointer is on the card. */
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
      className="enhance-fire-card group flex w-full shrink-0 items-center gap-(--space-xl) rounded-(--radius-popover) px-(--space-2xl) py-(--space-xl) text-left"
    >
      <canvas
        ref={mountFire}
        aria-hidden="true"
        data-fuel={face.calm ? 0.55 : 1}
        className="enhance-fire-canvas"
      />
      <span
        aria-hidden="true"
        className={cn(
          "enhance-fire-icon grid size-14 shrink-0 place-items-center rounded-full",
          face.calm ? "bg-surface-sidebar text-brand-tint" : "bg-brand-tint text-white",
        )}
      >
        <face.Icon size={28} strokeWidth={2} />
      </span>
      <span className="flex min-w-0 flex-1 flex-col gap-(--space-xs)">
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
        className={cn(
          "enhance-fire-action flex shrink-0 items-center gap-(--space-sm) rounded-full px-(--space-lg) py-(--space-sm) type-headline whitespace-nowrap",
          !face.calm && "text-white",
        )}
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
