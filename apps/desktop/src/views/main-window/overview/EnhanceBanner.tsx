import { ArrowRight } from "lucide-react"
import { useId } from "react"

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

/** The Optimise card. It sits at the top of the blur scrim over the config checks. The pill
 *  is centred at the bottom. It
 *  names the failing checks. The pill opens the Optimise wizard. */
export function EnhanceBanner({
  failingLabels,
  state,
  onOpen,
}: {
  /** Labels of failing checks that are not snoozed, or null while the report loads. */
  failingLabels: readonly string[] | null
  state: EnhanceButtonState
  onOpen: () => void
}) {
  const face = pillFace(state)
  const count = failingLabels?.length ?? 0
  const statusId = useId()
  return (
    <section
      aria-label="Optimise"
      className="enhance-banner flex w-full flex-col items-center gap-(--space-lg) rounded-(--radius-popover) p-(--space-xl) text-center"
    >
      <div className="flex w-full min-w-0 flex-col gap-(--space-xs)">
        <h2 className="type-body font-semibold text-label">
          {failingLabels == null
            ? "Checking your config…"
            : count === 0
              ? "No fixes needed in your config"
              : `${count} ${count === 1 ? "fix" : "fixes"} found in your config`}
        </h2>
        {failingLabels != null && count > 0 && (
          <p id={statusId} className="truncate type-footnote text-label-secondary">
            {failingLabels.slice(0, 3).join(", ")}
            {count > 3 ? `, +${count - 3} more` : ""}
          </p>
        )}
      </div>
      <button
        type="button"
        aria-label={face.label}
        aria-describedby={failingLabels != null && count > 0 ? statusId : undefined}
        onClick={onOpen}
        data-quiet={face.calm ? "" : undefined}
        className="enhance-pill group flex shrink-0 items-center gap-(--space-sm) px-(--space-xl) type-body font-semibold whitespace-nowrap"
      >
        {face.action}
        <ArrowRight
          aria-hidden="true"
          size={15}
          className="transition-transform duration-(--duration-medium) group-hover:translate-x-0.5"
        />
      </button>
    </section>
  )
}
