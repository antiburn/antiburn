import { describe, expect, it } from "vitest"
import { installedCheckDiagnosticFixture as diagnostic } from "../../views/main-window/burn-checks/tests/installedCheckFixture"
import {
  installedCheckDiagnosticDetails,
  installedCheckDiagnostics,
} from "./installedCheckDiagnostics"

describe("installed diagnostic grouping", () => {
  it.each([false, true])(
    "keeps one error for the same issue with error first=%s",
    (errorFirst) => {
      const error = diagnostic
      const info = {
        ...diagnostic,
        severity: "info" as const,
        details: [...diagnostic.details].reverse(),
      }
      expect(installedCheckDiagnostics(errorFirst ? [error, info] : [info, error])).toEqual([
        error,
      ])
    },
  )

  it("does not merge different files, lines, columns, fields, inputs, decisions, or source issues", () => {
    const diagnostics = [
      diagnostic,
      { ...diagnostic, source: { ...diagnostic.source!, path: "/workspace/other/check.yaml" } },
      { ...diagnostic, source: { ...diagnostic.source!, line: 30 } },
      { ...diagnostic, source: { ...diagnostic.source!, column: 12 } },
      { ...diagnostic, details: [{ label: "Field", value: "decisions.retry.input" }] },
      {
        ...diagnostic,
        details: [{ label: "Input path", value: "activity[*].tool.arguments" }],
      },
      { ...diagnostic, details: [{ label: "Decision", value: "recovery" }] },
      { ...diagnostic, code: "source-unavailable" },
    ]
    expect(installedCheckDiagnostics(diagnostics)).toEqual(diagnostics)
  })

  it("keeps errors before unrelated notices", () => {
    const notice = { ...diagnostic, severity: "info" as const, code: "input-shortened" }
    expect(installedCheckDiagnostics([notice, diagnostic])).toEqual([diagnostic, notice])
  })

  it.each([
    { title: "Session journal unavailable", message: "Rescan sessions to restore the source." },
    { title: "Session journal unavailable", message: "Restore access to the session journal." },
    { title: "Session source unavailable", message: "Restore access to the session source." },
    {
      title: "Session source unavailable",
      message: "The session source is no longer available.",
    },
  ])("does not merge distinct source-unavailable copy: $title, $message", (copy) => {
    const first = {
      ...diagnostic,
      code: "source-unavailable",
      title: "Session source unavailable",
      message: "Rescan sessions to restore the source.",
    }
    const second = { ...first, ...copy }
    expect(installedCheckDiagnostics([first, second])).toEqual([first, second])
  })
})

describe("installed diagnostic detail presentation", () => {
  it.each([
    ["yaml-syntax", "Field", "decisions.retry.input"],
    ["compile-invalid-expression", "Field", "actions[0].if"],
    ["discovery-directory-unavailable", "Folder", "/workspace/demo/.antiburn/checks"],
    ["discovery-read-failed", "Directory", "/workspace/demo/.antiburn/checks"],
    ["input-unavailable", "Input path", "activity[*].tool.result.text"],
    ["provider-invalid-response", "Decision", "retry"],
    ["provider-invalid-request", "Model", "synthetic-model"],
    ["filter-evidence-unavailable", "Filter", "paths-ignore"],
    ["storage-failed", "Runtime stage", "assessment_settlement"],
    ["storage-failed", "Failure category", "sqlite"],
    ["storage-failed", "SQLite code", "13"],
  ])("keeps the %s %s as a technical identifier", (code, label, value) => {
    expect(
      installedCheckDiagnosticDetails({ ...diagnostic, code, details: [{ label, value }] }),
    ).toEqual([{ label, value, code: true }])
  })

  it.each([
    ["Reason", "The provider rejected the request."],
    ["Fitting result", "All permitted reductions were exhausted."],
    ["Provider", "Cloudflare"],
    ["Original input size", "8192 bytes; local limit 4096 bytes"],
    ["Input values", "200 nodes; local limit 100 nodes"],
    ["Input nesting", "12 levels; local limit 10 levels"],
    ["Fitting rules", "20 rules; local limit 16 rules"],
    ["Fitting path depth", "12 segments; local limit 8 segments"],
    ["Fitting path length", "1024 bytes; local limit 512 bytes"],
    ["Expanded fitting targets", "200 targets; local limit 100 targets"],
    ["Fitting traversal", "2000 node visits; local limit 1000 node visits"],
    ["Input reductions", "20 turns; local limit 16 turns"],
    ["Cumulative preparation work", "8192 bytes; local limit 4096 bytes"],
    ["Serialized request size", "8192 bytes; local limit 4096 bytes"],
    ["Estimated request size", "8192 estimated tokens; local limit 4096 estimated tokens"],
    ["Resident request size", "8192 bytes; local limit 4096 bytes"],
    ["Question criteria", "20 criteria; local limit 16 criteria"],
    ["activity[0].tool.result.text", "truncate: retained 1024 of 8192 UTF-8 bytes"],
    ["activity", "recent: retained 10 of 20 records"],
    ["Additional context", "A new detail stays readable without an IPC change."],
  ])("keeps %s readable without monospace", (label, value) => {
    expect(
      installedCheckDiagnosticDetails({ ...diagnostic, details: [{ label, value }] }),
    ).toEqual([{ label, value, code: false }])
  })

  it("removes repeated copy and background fields without changing the payload", () => {
    const payload = {
      ...diagnostic,
      details: [
        { label: "Reason", value: `  ${diagnostic.message}  ` },
        { label: "Reason", value: diagnostic.title },
        { label: "Additional context", value: diagnostic.message },
        {
          label: "Stored analysis",
          value: "Stored-session analysis needs no live connection.",
        },
        { label: "Diagnostic code", value: diagnostic.code },
        { label: "Field", value: "decisions.retry.input" },
        { label: "Field", value: "decisions.retry.input" },
        { label: "Reason", value: " " },
      ],
    }
    const original = structuredClone(payload)
    expect(installedCheckDiagnosticDetails(payload)).toEqual([
      { label: "Field", value: "decisions.retry.input", code: true },
    ])
    expect(payload).toEqual(original)
  })

  it.each([
    ["Live capture is not active for this session.", " "],
    ["Live capture stopped!", "\t"],
    ["Did live capture stop?", "\n\n"],
    ["Live capture is unavailable", ""],
    ["Live capture stopped.", ""],
  ])("keeps the capture cause %s without saved-history boilerplate", (cause, separator) => {
    expect(
      installedCheckDiagnosticDetails({
        ...diagnostic,
        code: "opencode-v2-capture-gap",
        message:
          "Live activity is not available for this session. Keep OpenCode open, then rescan sessions. Live checks cannot recover missed activity. Saved results cover only their recorded input.",
        details: [
          {
            label: "Reason",
            value: separator
              ? `${cause}${separator}Live checks cannot replay earlier events. Earlier checks cover only their saved input.`
              : cause,
          },
        ],
      }),
    ).toEqual([{ label: "Reason", value: cause, code: false }])
  })
})
