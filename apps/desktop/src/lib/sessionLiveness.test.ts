import { describe, expect, it } from "vitest"

import type { SessionLifecycleEvent } from "./ipc"
import {
  IDLE_LIVENESS,
  applyLifecycleEvent,
  isLive,
  liveProviders,
  livenessExpiry,
  livenessFromSnapshot,
} from "./sessionLiveness"

const NOW = Date.parse("2026-09-08T00:00:00Z")

function ref(agent: string, sessionId = "session-1") {
  return { environmentKey: "native", agent, sessionId }
}

function event(
  kind: SessionLifecycleEvent["kind"],
  agent: string,
  sessionId: string | null = "session-1",
): SessionLifecycleEvent {
  const at = Math.floor(NOW / 1000)
  if (kind === "activity") {
    return { kind, session: sessionId == null ? null : ref(agent, sessionId), agent, at }
  }
  return { kind, session: ref(agent, sessionId ?? "session-1"), agent, at }
}

describe("sessionLiveness", () => {
  it("maps a live session's agent to the provider it draws on", () => {
    const live = livenessFromSnapshot([
      { session: ref("claude-code"), agent: "claude-code", lastActivityAt: 1 },
      { session: ref("codex", "session-2"), agent: "codex", lastActivityAt: 1 },
    ])
    expect(isLive(live, NOW)).toBe(true)
    expect(liveProviders(live, NOW)).toEqual(["anthropic", "openai"])
    expect(livenessExpiry(live, NOW)).toBeNull()
  })

  it("counts a live agent with no provider on screen as live, with nothing to blink", () => {
    const live = applyLifecycleEvent(IDLE_LIVENESS, event("activity", "cursor"))
    expect(isLive(live, NOW)).toBe(true)
    expect(liveProviders(live, NOW)).toEqual([])
  })

  it("drops a provider at the idle event of its last session", () => {
    let live = applyLifecycleEvent(IDLE_LIVENESS, event("started", "claude-code"))
    live = applyLifecycleEvent(live, event("activity", "antigravity", "session-2"))
    expect(liveProviders(live, NOW)).toEqual(["anthropic", "google"])

    live = applyLifecycleEvent(live, event("idle", "claude-code"))
    expect(liveProviders(live, NOW)).toEqual(["google"])
    live = applyLifecycleEvent(live, event("idle", "antigravity", "session-2"))
    expect(isLive(live, NOW)).toBe(false)
  })

  it("expires keyless activity per agent, earliest first", () => {
    let live = applyLifecycleEvent(IDLE_LIVENESS, event("activity", "claude-code", null))
    const later: SessionLifecycleEvent = {
      kind: "activity",
      session: null,
      agent: "codex",
      at: Math.floor(NOW / 1000) + 60,
    }
    live = applyLifecycleEvent(live, later)
    expect(liveProviders(live, NOW)).toEqual(["anthropic", "openai"])
    expect(livenessExpiry(live, NOW)).toBe(NOW + 180_000)

    const afterFirst = NOW + 180_001
    expect(liveProviders(live, afterFirst)).toEqual(["openai"])
    expect(livenessExpiry(live, afterFirst)).toBe(NOW + 240_000)
    expect(isLive(live, NOW + 240_001)).toBe(false)
  })

  it("keeps keyless activity across a snapshot, and reports its expiry beside keyed sessions", () => {
    const anonymous = applyLifecycleEvent(IDLE_LIVENESS, event("activity", "codex", null))
    const live = livenessFromSnapshot(
      [{ session: ref("claude-code"), agent: "claude-code", lastActivityAt: 1 }],
      anonymous,
    )
    expect(liveProviders(live, NOW)).toEqual(["anthropic", "openai"])
    expect(livenessExpiry(live, NOW)).toBe(NOW + 180_000)
    expect(liveProviders(live, NOW + 180_001)).toEqual(["anthropic"])
  })
})
