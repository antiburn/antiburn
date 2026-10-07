import { AgentsStepSettings } from "./AgentsStepSettings"
import { ChecksStepSettings } from "./ChecksStepSettings"
import { LimitsStepSettings } from "./LimitsStepSettings"
import { SessionsStepSettings } from "./SessionsStepSettings"

export type StepSettingsStep = "agents" | "limits" | "sessions" | "checks" | "fixes"

export function StepSettings({ step }: { step: StepSettingsStep }) {
  switch (step) {
    case "agents":
      return <AgentsStepSettings />
    case "limits":
      return <LimitsStepSettings />
    case "sessions":
      return <SessionsStepSettings />
    case "checks":
      return <ChecksStepSettings />
    case "fixes":
      return null
  }
}
