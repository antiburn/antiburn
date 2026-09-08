import type {
  LiveSessionPayload,
  LiveUsageProvider,
  SessionLifecycleEvent,
  SessionRefPayload,
} from "./ipc"

/**
 * How long agent-level activity with no session counts as live, in
 * milliseconds. Mirrors `ACTIVE_SESSION_WINDOW_SECS` in the engine: the
 * shell's lifecycle bus applies the same window to a keyed session, and
 * publishes `idle` for it, so only the keyless case needs a local expiry.
 */
const LIVE_WINDOW_MS = 180_000

/**
 * The provider whose limits an agent draws down, by agent slug. Mirrors the
 * fixed routes in `src-tauri/src/provider_usage/providers.rs`, kept to the
 * providers the limits surfaces can show. An agent with no entry has no
 * meter on screen to blink.
 */
const AGENT_PROVIDERS: Readonly<Record<string, LiveUsageProvider>> = {
  "claude-code": "anthropic",
  codex: "openai",
  antigravity: "google",
}

/** What a surface knows about live sessions, from the bus. */
export interface Liveness {
  /** The sessions the bus says are live, by `liveSessionKey`, with their agent slug. */
  keys: ReadonlyMap<string, string>
  /**
   * When agent-level activity with no session stops counting, as epoch
   * milliseconds, by agent slug. A write under an agent's root that the
   * store has not indexed yet reaches here.
   */
  anonymousUntil: ReadonlyMap<string, number>
}

export const IDLE_LIVENESS: Liveness = { keys: new Map(), anonymousUntil: new Map() }

function liveSessionKey(session: SessionRefPayload): string {
  return JSON.stringify([session.environmentKey, session.agent, session.sessionId])
}

/** The live set a snapshot states. Keyless activity is kept from `previous`. */
export function livenessFromSnapshot(
  sessions: readonly LiveSessionPayload[],
  previous: Liveness = IDLE_LIVENESS,
): Liveness {
  return {
    keys: new Map(sessions.map(({ session, agent }) => [liveSessionKey(session), agent])),
    anonymousUntil: previous.anonymousUntil,
  }
}

/** The live set after one bus event. */
export function applyLifecycleEvent(state: Liveness, event: SessionLifecycleEvent): Liveness {
  switch (event.kind) {
    case "started":
      return withKey(state, liveSessionKey(event.session), event.agent)
    case "activity": {
      if (event.session) return withKey(state, liveSessionKey(event.session), event.agent)
      const until = event.at * 1000 + LIVE_WINDOW_MS
      const anonymousUntil = new Map(state.anonymousUntil)
      anonymousUntil.set(event.agent, Math.max(anonymousUntil.get(event.agent) ?? 0, until))
      return { keys: state.keys, anonymousUntil }
    }
    case "idle": {
      const key = liveSessionKey(event.session)
      if (!state.keys.has(key)) return state
      const keys = new Map(state.keys)
      keys.delete(key)
      return { keys, anonymousUntil: state.anonymousUntil }
    }
  }
}

function withKey(state: Liveness, key: string, agent: string): Liveness {
  if (state.keys.get(key) === agent) return state
  return { keys: new Map(state.keys).set(key, agent), anonymousUntil: state.anonymousUntil }
}

/** The agent slugs with a live session at `now` (epoch milliseconds). */
function liveAgents(state: Liveness, now: number): Set<string> {
  const agents = new Set(state.keys.values())
  for (const [agent, until] of state.anonymousUntil) {
    if (until > now) agents.add(agent)
  }
  return agents
}

/** True while any session is live at `now` (epoch milliseconds). */
export function isLive(state: Liveness, now: number): boolean {
  return liveAgents(state, now).size > 0
}

/**
 * The providers a live session draws on at `now`, sorted. A live agent
 * with no provider on the limits surfaces contributes nothing here, but it
 * still counts for `isLive`.
 */
export function liveProviders(state: Liveness, now: number): LiveUsageProvider[] {
  const providers = new Set<LiveUsageProvider>()
  for (const agent of liveAgents(state, now)) {
    const provider = AGENT_PROVIDERS[agent]
    if (provider) providers.add(provider)
  }
  return [...providers].sort()
}

/**
 * The next instant the live set changes on its own, as epoch milliseconds,
 * or `null` when nothing expires locally: a keyed session ends with an
 * `idle` event, not a timer.
 */
export function livenessExpiry(state: Liveness, now: number): number | null {
  let next: number | null = null
  for (const until of state.anonymousUntil.values()) {
    if (until > now && (next == null || until < next)) next = until
  }
  return next
}
