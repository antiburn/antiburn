import { CircleCheck, Hourglass, Sparkles, type LucideIcon } from "lucide-react"

import { cn } from "../../../lib/cn"
import type { EnhanceButtonState } from "./enhanceState"

function buttonFace(state: EnhanceButtonState): {
  label: string
  Icon: LucideIcon
  calm: boolean
  ring: boolean
} {
  switch (state.kind) {
    case "new":
      return {
        label: `Enhance again · ${state.count} new`,
        Icon: Sparkles,
        calm: false,
        ring: false,
      }
    case "resume":
      return {
        label: `Continue setup · step ${state.step} of 5`,
        Icon: Sparkles,
        calm: true,
        ring: true,
      }
    case "watching":
      return { label: "Checking your fixes", Icon: Hourglass, calm: true, ring: false }
    case "clear":
      return { label: "All set · run again", Icon: CircleCheck, calm: true, ring: false }
    case "loading":
    case "fresh":
      return { label: "Enhance", Icon: Sparkles, calm: false, ring: false }
  }
}

/** The bar under the Overview content. It gives the reader one clear next
 *  step: open the Enhance wizard. */
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
  const face = buttonFace(state)
  return (
    <footer
      aria-label="Next step"
      className="overview-enhance-bar flex items-center gap-(--space-md) border-t border-separator pt-(--space-lg)"
    >
      <p role="status" className="grow type-caption text-label-secondary">
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
      </p>

      <button
        type="button"
        onClick={onOpen}
        data-calm={face.calm ? "" : undefined}
        data-ring={face.ring ? "" : undefined}
        className="overview-enhance shrink-0"
      >
        <span aria-hidden="true" className="overview-enhance-glow" />
        <span
          className={cn(
            "overview-enhance-face flex items-center gap-(--space-sm) rounded-(--radius-popover) px-(--space-lg) py-(--space-sm) type-body font-semibold whitespace-nowrap",
            face.calm ? "text-label" : "text-white",
          )}
        >
          <face.Icon
            aria-hidden="true"
            size={15}
            strokeWidth={2}
            className="overview-enhance-icon shrink-0"
          />
          {face.label}
        </span>
      </button>
    </footer>
  )
}
