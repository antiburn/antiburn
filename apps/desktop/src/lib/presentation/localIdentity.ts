/**
 * Stable identity for the things this app renders: an execution environment, a
 * session, and a repository.
 *
 * Two properties matter, and the tests pin both:
 *
 * - **No collisions.** Two different subjects never produce the same key. The
 *   parts are JSON-encoded as an array rather than joined with a separator, so
 *   a part containing the separator character cannot forge a different
 *   identity ("a|b" + "c" and "a" + "b|c" are distinct keys here).
 * - **Case-normalized environments.** A WSL distribution name is
 *   case-insensitive in practice, so `Ubuntu` and `UBUNTU` are one environment
 *   while `Ubuntu` and `Debian` stay two.
 *
 * Everything here is derived from what is on this machine — the agent, the
 * session id, the WSL distribution, and the repository's own stable key. There
 * is no account, tenant, or remote-cache dimension, because a local key has
 * nothing to be scoped to.
 */

import type { LocalSessionIdentity } from "../types/session"

/** The key for the native (non-WSL) environment. */
export const NATIVE_ENVIRONMENT_KEY = "native"

/**
 * Identity of the execution environment a session or clone lives in: the
 * machine itself, or one WSL distribution on it.
 *
 * Normalized with the invariant `en-US` locale rather than the user's, so the
 * key a session is cached under does not change when the system language does
 * (the Turkish dotless-i is the classic way that goes wrong).
 */
export function environmentKey(wslDistro?: string | null): string {
  const distro = wslDistro?.trim()
  return distro ? `wsl:${distro.toLocaleLowerCase("en-US")}` : NATIVE_ENVIRONMENT_KEY
}

/** True when two environments are the same, ignoring distribution-name case. */
export function sameEnvironment(a?: string | null, b?: string | null): boolean {
  return environmentKey(a) === environmentKey(b)
}

/**
 * Identity of one local session: its environment, its agent, and the id that
 * agent gave the transcript. Agent ids are only unique within an agent, and
 * only within an environment — a WSL clone of an agent may deliberately reuse
 * the ids of its native counterpart — so all three are part of the key.
 */
export function localSessionKey(
  agent: string,
  sessionId: string,
  wslDistro?: string | null,
): string {
  return JSON.stringify([environmentKey(wslDistro), agent, sessionId])
}

/** {@link localSessionKey} for an identity record. */
export function sessionIdentityKey(identity: LocalSessionIdentity): string {
  return localSessionKey(identity.agent, identity.sessionId, identity.wslDistro)
}

/** True when two session identities name the same local transcript. */
export function sameLocalSession(a: LocalSessionIdentity, b: LocalSessionIdentity): boolean {
  return sessionIdentityKey(a) === sessionIdentityKey(b)
}

/**
 * Identity of one local repository.
 *
 * `stableKey` is the repository's own durable identity when discovery
 * established one (a canonical root, say). It wins outright: two clones of the
 * same remote in different directories are different repositories, and a name
 * alone cannot tell them apart. Without one, the name is scoped to its
 * environment so a native clone and its WSL twin stay distinct.
 */
export function localRepositoryKey(
  repo: string,
  wslDistro?: string | null,
  stableKey?: string | null,
): string {
  const stable = stableKey?.trim()
  if (stable) return JSON.stringify(["stable", stable])
  return JSON.stringify(["repo", environmentKey(wslDistro), repo])
}
