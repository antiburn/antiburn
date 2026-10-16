import type { InstalledCheckDiagnosticPayload } from "../insightsIpc"

export function installedCheckErrorStatus(
  diagnostics: readonly InstalledCheckDiagnosticPayload[],
): string | null {
  const diagnostic = diagnostics.find((item) => item.severity === "error")
  if (!diagnostic) return null
  switch (diagnostic.code) {
    case "input-too-large":
    case "provider-context-rejected":
      return "Input too large"
    case "fit-config-invalid":
    case "input-invalid":
    case "provider-invalid-request":
    case "action-failed":
    case "evaluation-limit-exceeded":
      return "Check needs fixing"
    case "discovery-read-failed":
    case "discovery-directory-unavailable":
      return "Cannot read check"
    case "discovery-duplicate-id":
      return "Duplicate check"
    case "compile-unsupported-feature":
      return "Not supported"
    case "input-unavailable":
    case "source-unavailable":
    case "runtime-invalid-context":
      return "Missing information"
    case "provider-authentication":
    case "provider-unavailable":
    case "provider-not-configured":
      return "Connection needed"
    default:
      return diagnostic.code.startsWith("yaml-") ||
        diagnostic.code.startsWith("config-") ||
        diagnostic.code.startsWith("compile-") ||
        diagnostic.code === "discovery-invalid-id"
        ? "Check needs fixing"
        : "Could not check"
  }
}
