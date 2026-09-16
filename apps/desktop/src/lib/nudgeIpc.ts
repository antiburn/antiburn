import { invoke, isTauri } from "@tauri-apps/api/core"
import { listen, type UnlistenFn } from "@tauri-apps/api/event"

const noShellUnlisten: UnlistenFn = () => undefined

/* -------------------------------------------------------------------------
 * Nudge payloads — mirrors `src-tauri/crates/nudge/src/model.rs`
 *
 * The nudge crate is the *mechanism* behind the floating notification window:
 * it owns that window and its placement, and knows nothing about why a nudge
 * fires. These shapes are the whole contract between it and `NudgeView`.
 * ---------------------------------------------------------------------- */

/** Window label the shell gives the notification window. Mirrors `NUDGE_LABEL`. */
export const NUDGE_WINDOW_LABEL = "nudge"

/**
 * What surfaced a nudge. Mirrors Rust `NudgeKind`.
 *
 * The view is deliberately kind-agnostic — it draws whatever fields arrived —
 * so a new trigger is a new variant here and a new payload builder in Rust,
 * with no change to the notification UI.
 */
export type NudgeKind =
  | "updateAvailable"
  | "scanFailure"
  | "diskSpaceLow"
  | "usageMilestone"
  | "menuBarLocation"
  | "test"

/** Visual tone — informational, positive, or attention. Mirrors Rust `NudgeTone`. */
export type NudgeTone = "info" | "success" | "warning"

/**
 * Optional structured target carried by a CTA and echoed back to the shell when
 * it is clicked, so the handler acts on what the nudge was actually about.
 */
export type NudgeActionTarget =
  | { type: "update"; expectedVersion: string }
  | { type: "providerUsage"; provider: string; accountKey: string | null }
  | { type: "session"; agent: string; sessionId: string; environment: string | null }

/** One actionable CTA on the notification. Mirrors Rust `NudgeAction`. */
export interface NudgeAction {
  /** Stable identifier routed back to the shell on click. */
  id: string
  label: string
  /** Rendered as the emphasized button, and always last (macOS convention). */
  primary: boolean
  target?: NudgeActionTarget
}

/**
 * Payload of the `nudge:show` event. Mirrors Rust `Nudge`.
 *
 * Empty optionals are omitted on the wire (`skip_serializing_if` in Rust), so
 * `recommendations` arrives absent rather than as `[]`.
 */
export interface Nudge {
  id: string
  kind: NudgeKind
  tone: NudgeTone
  title: string
  /** Short summary that stays visible in collapsed and expanded states. */
  subtitle: string
  /** Detailed copy revealed when the notification expands. */
  description: string
  /** Who or what this is about, when it is about one. Never drawn; the shell acts on it. */
  actor?: string
  /** Suggested steps, revealed when the notification expands on hover. */
  recommendations?: string[]
  actions: NudgeAction[]
  /** Auto-dismiss timeout in milliseconds; absent means sticky until acted on. */
  timeoutMs?: number
}

/* -------------------------------------------------------------------------
 * Nudge commands
 *
 * Registered by the shell from `antiburn_nudge::commands::`. Every one of them
 * is called from the notification window only, and every one is a request the
 * crate may decline — a nudge that has already been dismissed answers none of
 * them, which is why they all resolve to nothing.
 * ---------------------------------------------------------------------- */

/**
 * Report a clicked CTA back to the crate.
 *
 * The crate hands `(kind, actionId, target)` to the shell's `on_action`
 * callback and then dismisses the notification, so this both acts and closes.
 */
export async function nudgeAction(
  kind: NudgeKind,
  actionId: string,
  target?: NudgeActionTarget,
): Promise<void> {
  if (!isTauri()) return
  await invoke("nudge_action", { kind, actionId, target })
}

/** Hide the notification without acting (close button, or the auto-dismiss timeout). */
export async function dismissNudge(): Promise<void> {
  if (!isTauri()) return
  await invoke("nudge_dismiss")
}

/**
 * Reveal the notification at its measured content `height`.
 *
 * The crate keeps the window hidden until this arrives, then sizes, places, and
 * shows it in one step — which is what stops the notification from visibly
 * resizing on screen as it appears.
 */
export async function revealNudge(height: number): Promise<void> {
  if (!isTauri()) return
  await invoke("nudge_reveal", { height })
}

/**
 * Resize the *already visible* notification to a new measured `height` (it
 * expanded or collapsed on hover). On macOS the native frame animates with the
 * system's resize ease, anchored at its top edge; elsewhere it snaps.
 */
export async function resizeNudge(height: number): Promise<void> {
  if (!isTauri()) return
  await invoke("nudge_resize", { height })
}

/**
 * Signal that this window's `nudge:show` listener is attached.
 *
 * The window is prewarmed, so a nudge can be emitted before the webview is
 * listening. The crate retains the pending payload and re-delivers it here,
 * rather than the first nudge after a launch being silently lost.
 */
export async function nudgeReady(): Promise<void> {
  if (!isTauri()) return
  await invoke("nudge_ready")
}

/**
 * Report the notification's hover state (macOS only; a no-op elsewhere).
 *
 * An unprompted nudge never takes key-window status on its own, so hover-driven
 * CSS only receives the mouse-moved events it needs once the cursor has
 * genuinely entered the notification — at which point taking key is safe.
 * Deliberately *not* called for a hover the crate detected by sampling the
 * cursor (`NUDGE_HOVER_EVENT`): that path exists precisely because the window
 * is receiving no mouse events, so there is no `:hover` to feed.
 */
export async function setNudgeHovered(hovered: boolean): Promise<void> {
  if (!isTauri()) return
  await invoke("nudge_set_hovered", { hovered })
}

/**
 * Event the nudge crate emits to the notification window, carrying the
 * {@link Nudge} to draw. Mirrors `NUDGE_SHOW_EVENT` in
 * `src-tauri/crates/nudge/src/lib.rs`.
 */
export const NUDGE_SHOW_EVENT = "nudge:show"

/** Subscribe to incoming nudges. The returned function unsubscribes. */
export async function onNudgeShow(handler: (nudge: Nudge) => void): Promise<UnlistenFn> {
  if (!isTauri()) return noShellUnlisten
  return listen<Nudge>(NUDGE_SHOW_EVENT, (event) => handler(event.payload))
}

/**
 * Event the nudge crate emits carrying whether the cursor is over the
 * notification, sampled natively. Mirrors `NUDGE_HOVER_EVENT` in the crate.
 *
 * macOS only. It backs up — never replaces — the window's own
 * `mouseenter`/`mouseleave`, which stop firing whenever another antiburn window
 * (the settings window, or the popover) holds macOS key-window status, because
 * AppKit routes mouse-moved events there instead.
 */
export const NUDGE_HOVER_EVENT = "nudge:hover"

/** Subscribe to the native hover signal. The returned function unsubscribes. */
export async function onNudgeHover(handler: (hovered: boolean) => void): Promise<UnlistenFn> {
  if (!isTauri()) return noShellUnlisten
  return listen<boolean>(NUDGE_HOVER_EVENT, (event) => handler(event.payload === true))
}
