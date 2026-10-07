import { noteInteraction } from "../../../../lib/ipc"
import { StepSettings, type StepSettingsStep } from "./StepSettings"

/**
 * The first-run takeover's "More info" toggle. It is link-style text inline
 * at the end of the step's body. The caller keeps the open state and shows
 * `StepSettingsPanel` under the step. Not for `"fixes"`, which has no
 * settings to show.
 */
export function MoreInfoToggle({
  step,
  open,
  onToggle,
}: {
  step: StepSettingsStep
  open: boolean
  onToggle: () => void
}) {
  return (
    <button
      type="button"
      aria-expanded={open}
      onClick={() => {
        if (!open && step !== "fixes") {
          noteInteraction({ kind: "stepSettingsViewed", label: step, detail: "first_run" })
        }
        onToggle()
      }}
      className="text-accent underline decoration-accent/40 underline-offset-[3px] hover:decoration-accent"
    >
      {open ? "Less info" : "More info"}
    </button>
  )
}

/** The settings that `MoreInfoToggle` opens for one step. */
export function StepSettingsPanel({ step }: { step: StepSettingsStep }) {
  return (
    <div className="flex w-full flex-col gap-(--space-md)">
      <StepSettings step={step} />
    </div>
  )
}
