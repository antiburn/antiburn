import { invoke, isTauri } from "@tauri-apps/api/core"
import { listen, type UnlistenFn } from "@tauri-apps/api/event"

const noShellUnlisten: UnlistenFn = () => undefined

/* -------------------------------------------------------------------------
 * Session lifecycle bus — mirrors `src-tauri/src/session_lifecycle.rs`
 *
 * The shell publishes one event for every transition of a session it watches,
 * and answers a snapshot of the sessions inside the active window. A surface
 * reads the snapshot once, then follows the events.
 * ---------------------------------------------------------------------- */

/** One session's identity on the lifecycle bus. */
export interface SessionRefPayload {
  environmentKey: string
  agent: string
  sessionId: string
}

/**
 * One transition on the shell's session lifecycle bus. Mirrors
 * `session_lifecycle::SessionEvent` in `src-tauri/src/session_lifecycle.rs`.
 * `at` is epoch seconds.
 */
export type SessionLifecycleEvent =
  | { kind: "started"; session: SessionRefPayload; agent: string; at: number }
  | {
      kind: "activity"
      /** `null` for a write under an agent root the store has not indexed yet. */
      session: SessionRefPayload | null
      agent: string
      at: number
    }
  /** 30 seconds after the session's last write. The session is still active. */
  | { kind: "quiet"; session: SessionRefPayload; agent: string; at: number }
  | { kind: "idle"; session: SessionRefPayload; agent: string; at: number }

/** One session inside the active window, as the snapshot command returns it. */
export interface LiveSessionPayload {
  session: SessionRefPayload
  agent: string
  /** Epoch seconds. */
  lastActivityAt: number
  /**
   * The model of the session's newest analyzed turn, as the provider names
   * it in a transcript. `null` until an analysis pass publishes a turn. A
   * meter scoped to one model sweeps from this.
   */
  model: string | null
}

/**
 * Return every session inside the active window, most recent first. A
 * surface reads this once, then follows `onSessionLifecycle` for changes.
 */
export async function getLiveSessions(): Promise<LiveSessionPayload[]> {
  if (!isTauri()) return []
  return (await invoke<LiveSessionPayload[] | null>("get_live_sessions")) ?? []
}

/**
 * Event the shell emits for every transition on the session lifecycle bus.
 * Mirrors `SESSION_LIFECYCLE_EVENT` in `src-tauri/src/commands.rs`.
 */
export const SESSION_LIFECYCLE_EVENT = "session:lifecycle"

/** Subscribe to session lifecycle transitions. The result unsubscribes. */
export async function onSessionLifecycle(
  handler: (event: SessionLifecycleEvent) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) return noShellUnlisten
  return listen<SessionLifecycleEvent>(SESSION_LIFECYCLE_EVENT, (event) =>
    handler(event.payload),
  )
}
