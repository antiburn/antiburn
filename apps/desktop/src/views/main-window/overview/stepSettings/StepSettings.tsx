import { AgentsStepSettings } from "./AgentsStepSettings"
import { ChecksStepSettings } from "./ChecksStepSettings"
import { LimitsStepSettings } from "./LimitsStepSettings"
import { SessionsStepSettings } from "./SessionsStepSettings"

export type StepSettingsStep = "agents" | "limits" | "sessions" | "checks" | "fixes"

export function StepSettings({
  step,
  control,
  targetRevision,
}: {
  step: StepSettingsStep
  control?: string | null | undefined
  targetRevision?: number | undefined
}) {
  switch (step) {
    case "agents":
      return <AgentsStepSettings />
    case "limits":
      return <LimitsStepSettings />
    case "sessions":
      return <SessionsStepSettings />
    case "checks":
      return <ChecksStepSettings control={control} targetRevision={targetRevision} />
    case "fixes":
      return null
  }
}
