import { invoke, isTauri } from "@tauri-apps/api/core"
import { listen, type UnlistenFn } from "@tauri-apps/api/event"

export interface MainWindowSessionIdentity {
  agent: string
  sessionId: string
  wslDistro: string | null
}

/** One revisioned request to show a session in the retained main window. */
export interface MainWindowSessionRequest {
  revision: number
  target: MainWindowSessionIdentity
}

export interface MainWindowHealthCheckRequest {
  requestId: number
  generation: number
}

export type MainWindowRendererStatus = "healthy" | "fallback" | "degraded"
export type MainWindowRenderFailureKind =
  "render_fallback" | "window_error" | "unhandled_rejection" | "responder_install_failed"
export type MainWindowRenderFailureCategory =
  | "type_error"
  | "reference_error"
  | "range_error"
  | "syntax_error"
  | "dom_exception"
  | "invoke_error"
  | "non_error_value"
  | "unknown"
export type MainWindowRenderErrorName =
  | "Error"
  | "TypeError"
  | "ReferenceError"
  | "RangeError"
  | "SyntaxError"
  | "EvalError"
  | "URIError"
  | "AggregateError"
  | "DOMException"
  | "Other"
export interface MainWindowRenderFailureReport {
  kind: MainWindowRenderFailureKind
  category: MainWindowRenderFailureCategory
  errorName: MainWindowRenderErrorName
}

const noShellUnlisten: UnlistenFn = () => undefined

/** Tell the shell that the retained main window committed this renderer generation. */
export async function mainWindowReady(generation: number): Promise<void> {
  if (!isTauri()) return
  await invoke("main_window_ready", { generation })
}

/** Whether the retained main renderer can present work. */
export async function getMainWindowVisible(): Promise<boolean> {
  if (!isTauri()) return true
  return invoke<boolean>("get_main_window_visible")
}

/** Open or focus the main window and select one exact local session. */
export async function openMainWindowSession(target: MainWindowSessionIdentity): Promise<void> {
  if (!isTauri()) return
  await invoke("open_main_window_session", { target })
}

/** Peek at the latest target that this renderer generation can apply. */
export async function peekMainWindowSessionTarget(
  generation: number,
): Promise<MainWindowSessionRequest | null> {
  if (!isTauri()) return null
  return invoke<MainWindowSessionRequest | null>("peek_main_window_session_target", {
    generation,
  })
}

/** Acknowledge that this renderer applied the latest session target. */
export async function acknowledgeMainWindowSessionTarget(
  generation: number,
  revision: number,
): Promise<void> {
  if (!isTauri()) return
  await invoke("acknowledge_main_window_session_target", { generation, revision })
}

/** Answer one generation-scoped hidden-window health check. */
export async function mainWindowHealthAck(
  requestId: number,
  generation: number,
  healthy: boolean,
): Promise<void> {
  if (!isTauri()) return
  await invoke("main_window_health_ack", { requestId, generation, healthy })
}

/** Read a health request emitted before this renderer installed its listener. */
export async function mainWindowPendingHealthCheck(
  generation: number,
): Promise<MainWindowHealthCheckRequest | null> {
  if (!isTauri()) return null
  return invoke<MainWindowHealthCheckRequest | null>("main_window_pending_health_check", {
    generation,
  })
}

/** Report the committed renderer tree without claiming that pixels were presented. */
export async function reportMainWindowRenderStatus(
  generation: number,
  status: MainWindowRendererStatus,
): Promise<void> {
  if (!isTauri()) return
  await invoke("report_main_window_render_status", { generation, status })
}

/** Report one closed-schema renderer diagnostic. */
export async function reportMainWindowRenderFailure(
  generation: number,
  report: MainWindowRenderFailureReport,
): Promise<void> {
  if (!isTauri()) return
  await invoke("report_main_window_render_failure", { generation, report })
}

/** Start an informed replacement of the current fallback renderer. */
export async function requestMainWindowRecovery(generation: number): Promise<void> {
  if (!isTauri()) return
  await invoke("request_main_window_recovery", { generation })
}

/** Event emitted when the main renderer can start or stop presenting work. */
export const MAIN_WINDOW_VISIBILITY_CHANGED_EVENT = "main:visibility-changed"

/** Event carrying a revisioned session target to an existing main renderer. */
export const MAIN_WINDOW_SESSION_TARGET_EVENT = "main:session-target"

/** Event asking a hidden retained renderer for its committed health. */
export const MAIN_WINDOW_HEALTH_CHECK_EVENT = "main:health-check"

/** Subscribe to main-window presentation visibility. */
export async function onMainWindowVisibilityChanged(
  handler: (visible: boolean) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) return noShellUnlisten
  return listen<boolean>(MAIN_WINDOW_VISIBILITY_CHANGED_EVENT, (event) =>
    handler(event.payload),
  )
}

/** Subscribe to session targets sent to the retained main renderer. */
export async function onMainWindowSessionTarget(
  handler: (request: MainWindowSessionRequest) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) return noShellUnlisten
  return listen<MainWindowSessionRequest>(MAIN_WINDOW_SESSION_TARGET_EVENT, (event) =>
    handler(event.payload),
  )
}

/** Subscribe to hidden-window health requests. */
export async function onMainWindowHealthCheck(
  handler: (request: MainWindowHealthCheckRequest) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) return noShellUnlisten
  return listen<MainWindowHealthCheckRequest>(MAIN_WINDOW_HEALTH_CHECK_EVENT, (event) =>
    handler(event.payload),
  )
}
