import type { InstalledCheckEvent } from "../insightsIpc"
import { pluralize } from "./text"

const eventLabels = {
  toolResult: "tool result",
  userMessage: "user message",
  assistantMessage: "assistant message",
  toolCall: "tool call",
  message: "message",
  turnEnd: "response cycle",
  sessionStart: "session-start observation",
  sessionIdle: "idle observation",
  sessionEvent: "session observation",
} satisfies Record<InstalledCheckEvent, string>

export function installedCheckEventLabel(event: InstalledCheckEvent, count: number): string {
  const label = eventLabels[event]
  return pluralize(count, label, `${label}s`)
}

export function installedCheckEventLimit(event: InstalledCheckEvent): string | null {
  switch (event) {
    case "sessionStart":
      return "Session-start observations mark when monitoring begins, not when the agent starts or resumes."
    case "sessionIdle":
      return "Idle observations record at least 120 seconds without observed activity. They do not prove that the agent finished."
    case "sessionEvent":
      return "Session observations record attachment or observed inactivity. They do not prove that the agent started, resumed, or finished."
    default:
      return null
  }
}
