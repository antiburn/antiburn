import type {
  LiveSessionPayload,
  LiveUsageProvider,
  SessionLifecycleEvent,
  SessionRefPayload,
} from "./ipc"

/**
 * How long one write keeps a session in the sweep, in milliseconds. Mirrors
 * `QUIET_WINDOW_SECS` in the shell: the lifecycle bus publishes `quiet` for
 * a keyed session at the same point. The local clock covers a snapshot that
 * lists an older write, a missed event, and keyless activity.
 */
export const LIVE_WINDOW_MS = 30_000

/**
 * The provider whose limits an agent draws down, by agent slug. Mirrors the
 * fixed routes in `src-tauri/src/provider_usage/providers.rs`, kept to the
 * providers the limits surfaces can show. An agent with no entry has no
 * meter on screen to sweep.
 */
const AGENT_PROVIDERS: Readonly<Record<string, LiveUsageProvider>> = {
  "claude-code": "anthropic",
  codex: "openai",
  antigravity: "google",
}

/** One session with a recent write. */
interface LiveEntry {
  /** The agent slug. */
  agent: string
  /** When the session's last write stops counting, as epoch milliseconds. */
  until: number
}

/** What a surface knows about live sessions, from the bus. */
export interface Liveness {
  /** The sessions with a recent write, by `liveSessionKey`. */
  sessions: ReadonlyMap<string, LiveEntry>
  /**
   * When agent-level activity with no session stops counting, as epoch
   * milliseconds, by agent slug. A write under an agent's root that the
   * store has not indexed yet reaches here.
   */
  anonymousUntil: ReadonlyMap<string, number>
}

export const IDLE_LIVENESS: Liveness = { sessions: new Map(), anonymousUntil: new Map() }

function liveSessionKey(session: SessionRefPayload): string {
  return JSON.stringify([session.environmentKey, session.agent, session.sessionId])
}

/**
 * The live set a snapshot states. A session written more than
 * `LIVE_WINDOW_MS` ago is in the active window but not in the sweep. Keyless
 * activity is kept from `previous`.
 */
export function livenessFromSnapshot(
  sessions: readonly LiveSessionPayload[],
  previous: Liveness = IDLE_LIVENESS,
): Liveness {
  return {
    sessions: new Map(
      sessions.map(({ session, agent, lastActivityAt }) => [
        liveSessionKey(session),
        { agent, until: lastActivityAt * 1000 + LIVE_WINDOW_MS },
      ]),
    ),
    anonymousUntil: previous.anonymousUntil,
  }
}

/** The live set after one bus event. */
export function applyLifecycleEvent(state: Liveness, event: SessionLifecycleEvent): Liveness {
  const until = event.at * 1000 + LIVE_WINDOW_MS
  switch (event.kind) {
    case "started":
      return withSession(state, liveSessionKey(event.session), event.agent, until)
    case "activity": {
      if (event.session) {
        return withSession(state, liveSessionKey(event.session), event.agent, until)
      }
      const anonymousUntil = new Map(state.anonymousUntil)
      anonymousUntil.set(event.agent, Math.max(anonymousUntil.get(event.agent) ?? 0, until))
      return { sessions: state.sessions, anonymousUntil }
    }
    case "quiet":
    case "idle": {
      const key = liveSessionKey(event.session)
      if (!state.sessions.has(key)) return state
      const sessions = new Map(state.sessions)
      sessions.delete(key)
      return { sessions, anonymousUntil: state.anonymousUntil }
    }
  }
}

function withSession(state: Liveness, key: string, agent: string, until: number): Liveness {
  const current = state.sessions.get(key)
  if (current && current.agent === agent && current.until >= until) return state
  return {
    sessions: new Map(state.sessions).set(key, { agent, until }),
    anonymousUntil: state.anonymousUntil,
  }
}

/** The agent slugs with a live session at `now` (epoch milliseconds). */
function liveAgents(state: Liveness, now: number): Set<string> {
  const agents = new Set<string>()
  for (const { agent, until } of state.sessions.values()) {
    if (until > now) agents.add(agent)
  }
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
 * or `null` when nothing is live. The bus publishes `quiet` at the same
 * point for a keyed session; the local clock is the fallback for it.
 */
export function livenessExpiry(state: Liveness, now: number): number | null {
  let next: number | null = null
  const consider = (until: number) => {
    if (until > now && (next == null || until < next)) next = until
  }
  for (const { until } of state.sessions.values()) consider(until)
  for (const until of state.anonymousUntil.values()) consider(until)
  return next
}
