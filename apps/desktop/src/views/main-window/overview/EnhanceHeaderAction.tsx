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

/** The Enhance call to action in the Recent sessions card header. A short
 *  status and a compact pill: the sessions under it are the evidence, and
 *  the pill opens the Enhance wizard. */
export function EnhanceHeaderAction({
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
    <span className="flex min-w-0 items-center gap-(--space-md)">
      <span id={statusId} className="truncate type-caption text-label-secondary">
        {failingChecks == null ? (
          "Checking your setup…"
        ) : failingChecks === 0 ? (
          <>
            <span className="font-semibold text-label">No fixes needed</span>
            <span className="@max-[720px]:hidden"> in your last 30 days.</span>
          </>
        ) : (
          <>
            <span className="font-semibold text-label">
              {failingChecks} {failingChecks === 1 ? "fix" : "fixes"} found
            </span>
            <span className="@max-[720px]:hidden"> in your last 30 days.</span>
          </>
        )}
      </span>
      <button
        type="button"
        aria-label={face.label}
        aria-describedby={statusId}
        onClick={onOpen}
        data-quiet={face.calm ? "" : undefined}
        data-compact=""
        className="enhance-pill group flex shrink-0 items-center gap-(--space-xs) px-(--space-md) type-caption font-semibold whitespace-nowrap"
      >
        {face.action}
        <ArrowRight
          aria-hidden="true"
          size={12}
          strokeWidth={2}
          className="transition-transform duration-(--duration-medium) group-hover:translate-x-0.5"
        />
      </button>
    </span>
  )
}
