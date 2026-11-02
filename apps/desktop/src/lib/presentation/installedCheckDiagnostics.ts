import type { InstalledCheckDiagnosticPayload } from "../insightsIpc"

type DiagnosticDetailKind = "code" | "background"

// Detail labels come from the native report; IPC does not provide field kinds.
const diagnosticDetailKinds: Readonly<Record<string, DiagnosticDetailKind>> = {
  Decision: "code",
  Field: "code",
  "Input path": "code",
  Directory: "code",
  Folder: "code",
  Filter: "code",
  Model: "code",
  "Runtime stage": "code",
  "Failure category": "code",
  "SQLite code": "code",
  "Stored analysis": "background",
  "Diagnostic code": "background",
}

export interface InstalledCheckDiagnosticDetailPresentation {
  label: string
  value: string
  code: boolean
}

function normalizedProse(value: string): string {
  return value
    .toLowerCase()
    .replace(/[^\p{L}\p{N}]+/gu, " ")
    .trim()
}

export function installedCheckDiagnosticDetails(
  diagnostic: InstalledCheckDiagnosticPayload,
): InstalledCheckDiagnosticDetailPresentation[] {
  const primary = ` ${normalizedProse(`${diagnostic.title}. ${diagnostic.message}`)} `
  const details = new Map<string, InstalledCheckDiagnosticDetailPresentation>()
  for (const detail of diagnostic.details) {
    const kind = diagnosticDetailKinds[detail.label] ?? "text"
    if (kind === "background") continue
    let value = detail.value.trim()
    if (detail.label === "Reason" && diagnostic.code === "opencode-v2-capture-gap") {
      // The first sentence gives the cause. Later sentences describe saved history.
      const sentenceEnd = value.search(/[.!?]\s/u)
      if (sentenceEnd >= 0) value = value.slice(0, sentenceEnd + 1)
    }
    if (!value || (kind !== "code" && primary.includes(` ${normalizedProse(value)} `))) continue
    const presented = { label: detail.label, value, code: kind === "code" }
    details.set(JSON.stringify([presented.label, presented.value]), presented)
  }
  return [...details.values()]
}

export function installedCheckDiagnosticKey(
  diagnostic: InstalledCheckDiagnosticPayload,
): string {
  return JSON.stringify([
    diagnostic.code,
    diagnostic.title,
    diagnostic.message,
    diagnostic.source && [
      diagnostic.source.path,
      diagnostic.source.line,
      diagnostic.source.column,
    ],
    diagnostic.details
      .filter((detail) => detail.label !== "Diagnostic code")
      .map((detail): [string, string] => [detail.label, detail.value])
      .sort(
        ([leftLabel, leftValue], [rightLabel, rightValue]) =>
          leftLabel.localeCompare(rightLabel) || leftValue.localeCompare(rightValue),
      ),
  ])
}

export function installedCheckDiagnostics(
  diagnostics: readonly InstalledCheckDiagnosticPayload[],
): InstalledCheckDiagnosticPayload[] {
  const issues = new Map<string, InstalledCheckDiagnosticPayload>()
  for (const diagnostic of diagnostics) {
    const key = installedCheckDiagnosticKey(diagnostic)
    const previous = issues.get(key)
    if (!previous || (previous.severity === "info" && diagnostic.severity === "error"))
      issues.set(key, diagnostic)
  }
  return [...issues.values()].sort(
    (left, right) => Number(left.severity === "info") - Number(right.severity === "info"),
  )
}
