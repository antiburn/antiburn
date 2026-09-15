import {
  reportMainWindowRenderFailure,
  type MainWindowRenderErrorName,
  type MainWindowRenderFailureCategory,
  type MainWindowRenderFailureKind,
  type MainWindowRenderFailureReport,
} from "./ipc"

const ERROR_NAMES = new Set<MainWindowRenderErrorName>([
  "Error",
  "TypeError",
  "ReferenceError",
  "RangeError",
  "SyntaxError",
  "EvalError",
  "URIError",
  "AggregateError",
  "DOMException",
])

function generation(): number | null {
  const value = window.__ANTIBURN_WINDOW_GENERATION__
  return typeof value === "number" && Number.isSafeInteger(value) ? value : null
}

function inspect(value: unknown): {
  category: MainWindowRenderFailureCategory
  errorName: MainWindowRenderErrorName
} {
  try {
    const rawName =
      typeof value === "object" && value !== null ? Reflect.get(value, "name") : null
    const errorName =
      typeof rawName === "string" && ERROR_NAMES.has(rawName as MainWindowRenderErrorName)
        ? (rawName as MainWindowRenderErrorName)
        : "Other"
    if (errorName === "TypeError") return { category: "type_error", errorName }
    if (errorName === "ReferenceError") return { category: "reference_error", errorName }
    if (errorName === "RangeError") return { category: "range_error", errorName }
    if (errorName === "SyntaxError") return { category: "syntax_error", errorName }
    if (errorName === "DOMException") return { category: "dom_exception", errorName }
    if (value instanceof Error) return { category: "unknown", errorName }
    return {
      category: value === undefined ? "unknown" : "non_error_value",
      errorName,
    }
  } catch {
    return { category: "unknown", errorName: "Other" }
  }
}

/** Build a diagnostic that cannot contain messages, paths, URLs, or rejection values. */
export function classifyMainWindowFailure(
  kind: MainWindowRenderFailureKind,
  value: unknown,
): MainWindowRenderFailureReport {
  const inspected = inspect(value)
  return {
    kind,
    category: kind === "responder_install_failed" ? "invoke_error" : inspected.category,
    errorName: inspected.errorName,
  }
}

/** Send one closed diagnostic for this renderer generation. */
export function reportBootstrapFailure(
  kind: MainWindowRenderFailureKind,
  value: unknown,
): void {
  const currentGeneration = generation()
  if (currentGeneration === null) return
  void reportMainWindowRenderFailure(
    currentGeneration,
    classifyMainWindowFailure(kind, value),
  ).catch(() => undefined)
}

/** Install diagnostic-only global handlers. They never mark the renderer fatal. */
export function installBootstrapDiagnostics(): () => void {
  const onError = (event: ErrorEvent) => reportBootstrapFailure("window_error", event.error)
  const onRejection = (event: PromiseRejectionEvent) =>
    reportBootstrapFailure("unhandled_rejection", event.reason)
  window.addEventListener("error", onError)
  window.addEventListener("unhandledrejection", onRejection)
  return () => {
    window.removeEventListener("error", onError)
    window.removeEventListener("unhandledrejection", onRejection)
  }
}
