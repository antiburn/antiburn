import type {
  InstalledCheckDiagnosticPayload,
  InstalledCheckReportPayload,
  BurnCheckTargetEvidencePayload,
} from "../../../../lib/insightsIpc"
import report from "./fixtures/installed-check-report.json"

// The Rust report test checks this fixture against the production serializer.
export const installedCheckFixture: InstalledCheckReportPayload = {
  ...report,
  effectiveId: "installed:failed-retry-loop",
  availability: { status: "available" },
  waitingForEvents: { ...report.waitingForEvents, event: "toolResult" },
}

export const installedCheckDiagnosticFixture: InstalledCheckDiagnosticPayload = {
  code: "input-too-large",
  severity: "error",
  title: "Input too large",
  message:
    "This check could not fit the selected input into the decision model. Open the check file and allow more input to be shortened, or select less input.",
  source: {
    reference: "installed-definition-reference",
    path: "/workspace/demo/.antiburn/checks/retries.yaml",
    line: 18,
    column: 5,
  },
  details: [
    { label: "Decision", value: "retry" },
    { label: "Input path", value: "activity[*].tool.result.text" },
  ],
  affectedSessionCount: 3,
}

const retainedResultHead = "Parser attempt failed. ".repeat(6)
const retainedResultTail = "Expected two records. ".repeat(6)

export const installedCheckAssessedInputFixture = {
  requests: [{ id: "request-1", text: "Fix the parser and run its tests." }],
  request_history_complete: false,
  source_coverage: "partial",
  activity: [
    {
      tool: {
        name: "bash",
        arguments: { command: "cargo test parser" },
        result: {
          content: `${retainedResultHead}\n[... omitted ...]\n${retainedResultTail}`,
          status: "error",
        },
      },
      quality: { source_complete: false },
    },
  ],
}

export const installedCheckFitReportFixture = {
  omissions: [
    {
      path: "activity[0].tool.result.content",
      strategy: "truncate",
      original_count: retainedResultHead.length + retainedResultTail.length + 1024,
      retained_count: retainedResultHead.length + retainedResultTail.length,
    },
  ],
}

export const installedCheckEvidenceFixture: BurnCheckTargetEvidencePayload = {
  status: "available",
  items: [
    {
      label: "context",
      sourceLabel: "Assessed input",
      reference: "assessed-input",
      observedAtMs: null,
      startLine: null,
      endLine: null,
      excerpt: JSON.stringify(installedCheckAssessedInputFixture),
      explanation: "These are the decision input values used for this assessment.",
      limitation:
        "Input was shortened under the check's fitting rules. Omitted records and text were not assessed.",
    },
    {
      label: "context",
      sourceLabel: "Input omissions",
      reference: "input-omissions",
      observedAtMs: null,
      startLine: null,
      endLine: null,
      excerpt: JSON.stringify(installedCheckFitReportFixture),
      explanation:
        "Counts use records for recent and original UTF-8 bytes for truncate. Retained byte counts exclude the omission marker.",
      limitation: null,
    },
  ],
}
