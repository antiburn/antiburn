import { ArrowRight } from "lucide-react"

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

/** The round Optimise button. It sits in the middle of the allowance chart
 *  and opens the Optimise wizard. */
export function EnhanceOrb({
  state,
  onOpen,
}: {
  state: EnhanceButtonState
  onOpen: () => void
}) {
  const face = pillFace(state)
  return (
    <button
      type="button"
      aria-label={face.label}
      onClick={onOpen}
      data-quiet={face.calm ? "" : undefined}
      className="enhance-pill enhance-orb group flex flex-col items-center justify-center gap-(--space-xs) type-title-3 font-semibold"
    >
      {face.action}
      <ArrowRight
        aria-hidden="true"
        size={17}
        className="transition-transform duration-(--duration-medium) group-hover:translate-x-0.5"
      />
    </button>
  )
}
