import { ArrowRight } from "lucide-react"

import { cn } from "../../../lib/cn"
import type { EnhanceButtonState } from "./enhanceState"

function pillFace(state: EnhanceButtonState): {
  /** The accessible name of the button. */
  label: string
  /** The short word in the round button. */
  action: string
  calm: boolean
} {
  switch (state.kind) {
    case "new":
      return {
        label: `Optimise again · ${state.count} new`,
        action: "Optimise",
        calm: false,
      }
    case "resume":
      return {
        label: `Continue setup · step ${state.step} of 5`,
        action: "Continue",
        calm: true,
      }
    case "watching":
      return { label: "Check your fixes", action: "Check", calm: true }
    case "clear":
      return { label: "All set · run again", action: "Run again", calm: true }
    case "loading":
    case "fresh":
      return { label: "Optimise my AI setup", action: "Optimise", calm: false }
  }
}

/** The Optimise card. It sits at the top of the blur scrim over the config
 *  checks. It names the failing checks. The round button in the chart opens
 *  the wizard. */
export function EnhanceBanner({
  failingLabels,
}: {
  /** Labels of failing checks that are not snoozed, or null while the report loads. */
  failingLabels: readonly string[] | null
}) {
  const count = failingLabels?.length ?? 0
  return (
    <section
      aria-label="Optimise"
      className="enhance-banner flex w-full flex-col gap-(--space-xs) rounded-(--radius-popover) p-(--space-xl) text-center"
    >
      <h2 className="type-body font-semibold text-label">
        {failingLabels == null
          ? "Checking your config…"
          : count === 0
            ? "No fixes needed in your config"
            : `${count} ${count === 1 ? "fix" : "fixes"} found in your config`}
      </h2>
      {failingLabels != null && count > 0 && (
        <p className="truncate type-footnote text-label-secondary">
          {failingLabels.slice(0, 3).join(", ")}
          {count > 3 ? `, +${count - 3} more` : ""}
        </p>
      )}
    </section>
  )
}

/** The Optimise button. It opens the Optimise wizard. The round shape sits
 *  in the middle of the week flower. The bar shape, a rounded rectangle,
 *  sits under the week chart. */
export function EnhanceOrb({
  state,
  onOpen,
  shape = "round",
}: {
  state: EnhanceButtonState
  onOpen: () => void
  shape?: "round" | "bar"
}) {
  const face = pillFace(state)
  return (
    <button
      type="button"
      aria-label={face.label}
      onClick={onOpen}
      data-quiet={face.calm ? "" : undefined}
      className={cn(
        "enhance-pill group flex items-center justify-center gap-(--space-xs) font-semibold",
        shape === "round"
          ? "enhance-orb flex-col type-title-3"
          : "enhance-bar px-(--space-xl) type-body",
      )}
    >
      {face.action}
      <ArrowRight
        aria-hidden="true"
        size={shape === "round" ? 17 : 15}
        className="transition-transform duration-(--duration-medium) group-hover:translate-x-0.5"
      />
    </button>
  )
}
