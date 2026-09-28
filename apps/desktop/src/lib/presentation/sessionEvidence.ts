import type { SessionEvidenceKind } from "../sessionEvidenceIpc"

export const EVIDENCE_KIND_LABELS: Record<SessionEvidenceKind, string> = {
  user: "Your message",
  assistant: "Assistant message",
  thinking: "Reasoning excerpt",
  tool_input: "Recorded tool input",
  tool_result: "Recorded tool output",
  tool_error: "Recorded error",
}
