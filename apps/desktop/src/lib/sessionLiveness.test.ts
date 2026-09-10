import { describe, expect, it } from "vitest"

import type { SessionLifecycleEvent } from "./ipc"
import {
  IDLE_LIVENESS,
  LIVE_WINDOW_MS,
  applyLifecycleEvent,
  isLive,
  liveProviders,
  livenessExpiry,
  livenessFromSnapshot,
} from "./sessionLiveness"

const NOW = Date.parse("2026-09-08T00:00:00Z")
const NOW_SECS = Math.floor(NOW / 1000)

function ref(agent: string, sessionId = "session-1") {
  return { environmentKey: "native", agent, sessionId }
}

function event(
  kind: SessionLifecycleEvent["kind"],
  agent: string,
  sessionId: string | null = "session-1",
  at = NOW_SECS,
): SessionLifecycleEvent {
  if (kind === "activity") {
    return { kind, session: sessionId == null ? null : ref(agent, sessionId), agent, at }
  }
  return { kind, session: ref(agent, sessionId ?? "session-1"), agent, at }
}

describe("sessionLiveness", () => {
  it("counts a session written inside the window, and maps its agent to a provider", () => {
    const live = livenessFromSnapshot([
      { session: ref("claude-code"), agent: "claude-code", lastActivityAt: NOW_SECS - 10 },
      { session: ref("codex", "session-2"), agent: "codex", lastActivityAt: NOW_SECS - 20 },
    ])
    expect(isLive(live, NOW)).toBe(true)
    expect(liveProviders(live, NOW)).toEqual(["anthropic", "openai"])
    // The earlier write closes first.
    expect(livenessExpiry(live, NOW)).toBe(NOW + LIVE_WINDOW_MS - 20_000)
  })

  it("leaves a session written 45 seconds ago out of the sweep", () => {
    // The session is inside the 180 s active window, so the snapshot lists
    // it, but its tokens stopped flowing 45 s ago.
    const live = livenessFromSnapshot([
      { session: ref("claude-code"), agent: "claude-code", lastActivityAt: NOW_SECS - 45 },
    ])
    expect(isLive(live, NOW)).toBe(false)
    expect(liveProviders(live, NOW)).toEqual([])
    expect(livenessExpiry(live, NOW)).toBeNull()
  })

  it("counts a live agent with no provider on screen as live, with nothing to sweep", () => {
    const live = applyLifecycleEvent(IDLE_LIVENESS, event("activity", "cursor"))
    expect(isLive(live, NOW)).toBe(true)
    expect(liveProviders(live, NOW)).toEqual([])
  })

  it("drops a provider at the quiet or idle event of its last session", () => {
    let live = applyLifecycleEvent(IDLE_LIVENESS, event("started", "claude-code"))
    live = applyLifecycleEvent(live, event("activity", "antigravity", "session-2"))
    expect(liveProviders(live, NOW)).toEqual(["anthropic", "google"])

    live = applyLifecycleEvent(live, event("quiet", "claude-code"))
    expect(liveProviders(live, NOW)).toEqual(["google"])
    live = applyLifecycleEvent(live, event("idle", "antigravity", "session-2"))
    expect(isLive(live, NOW)).toBe(false)
  })

  it("closes a keyed session's window 30 seconds after its write, without an event", () => {
    let live = applyLifecycleEvent(IDLE_LIVENESS, event("activity", "claude-code"))
    expect(livenessExpiry(live, NOW)).toBe(NOW + LIVE_WINDOW_MS)
    expect(isLive(live, NOW + LIVE_WINDOW_MS)).toBe(false)

    // A later write moves the window; an older one does not pull it back.
    live = applyLifecycleEvent(
      live,
      event("activity", "claude-code", "session-1", NOW_SECS + 20),
    )
    expect(livenessExpiry(live, NOW)).toBe(NOW + 20_000 + LIVE_WINDOW_MS)
    live = applyLifecycleEvent(
      live,
      event("activity", "claude-code", "session-1", NOW_SECS + 5),
    )
    expect(livenessExpiry(live, NOW)).toBe(NOW + 20_000 + LIVE_WINDOW_MS)
  })

  it("expires keyless activity per agent, earliest first", () => {
    let live = applyLifecycleEvent(IDLE_LIVENESS, event("activity", "claude-code", null))
    live = applyLifecycleEvent(live, event("activity", "codex", null, NOW_SECS + 60))
    expect(liveProviders(live, NOW)).toEqual(["anthropic", "openai"])
    expect(livenessExpiry(live, NOW)).toBe(NOW + LIVE_WINDOW_MS)

    const afterFirst = NOW + LIVE_WINDOW_MS + 1
    expect(liveProviders(live, afterFirst)).toEqual(["openai"])
    expect(livenessExpiry(live, afterFirst)).toBe(NOW + 60_000 + LIVE_WINDOW_MS)
    expect(isLive(live, NOW + 60_000 + LIVE_WINDOW_MS + 1)).toBe(false)
  })

  it("keeps keyless activity across a snapshot, and reports the earliest expiry", () => {
    const anonymous = applyLifecycleEvent(IDLE_LIVENESS, event("activity", "codex", null))
    const live = livenessFromSnapshot(
      [{ session: ref("claude-code"), agent: "claude-code", lastActivityAt: NOW_SECS - 5 }],
      anonymous,
    )
    expect(liveProviders(live, NOW)).toEqual(["anthropic", "openai"])
    expect(livenessExpiry(live, NOW)).toBe(NOW + LIVE_WINDOW_MS - 5_000)
    expect(liveProviders(live, NOW + LIVE_WINDOW_MS - 4_000)).toEqual(["openai"])
  })
})
