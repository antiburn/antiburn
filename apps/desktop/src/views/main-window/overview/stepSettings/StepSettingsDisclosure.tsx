import { useState } from "react"

import { noteInteraction } from "../../../../lib/ipc"
import { SKIP_BUTTON } from "../ProgressSteps"
import { StepSettings, type StepSettingsStep } from "./StepSettings"

/**
 * The first-run takeover's "Show settings" link: collapsed by default,
 * expands `StepSettings` for this step in place. Not for `"fixes"`, which
 * has no settings to show; its caller skips this component for that step.
 */
export function StepSettingsDisclosure({ step }: { step: StepSettingsStep }) {
  const [open, setOpen] = useState(false)
  return (
    <div className="flex w-full flex-col items-center gap-(--space-sm)">
      <button
        type="button"
        aria-expanded={open}
        onClick={() => {
          const next = !open
          setOpen(next)
          if (next && step !== "fixes") {
            noteInteraction({ kind: "stepSettingsViewed", label: step, detail: "first_run" })
          }
        }}
        className={SKIP_BUTTON}
      >
        {open ? "Hide settings" : "Show settings"}
      </button>
      {open && (
        <div className="flex w-full flex-col gap-(--space-md)">
          <StepSettings step={step} />
        </div>
      )}
    </div>
  )
}
