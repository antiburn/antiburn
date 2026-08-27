import { useMemo, useSyncExternalStore } from "react"

import { createExternalStore, type ExternalStore } from "./externalStore"
import { getSessionHygiene, type SessionHygienePayload } from "./insightsIpc"
import { onScanEvent, onSessionEntryChanged, onSessionsInvalidated } from "./ipc"
import { localSessionKey } from "./presentation/localIdentity"
import { INITIAL_SESSION_HYGIENE } from "./presentation/sessionHygiene"
import type { LocalSessionIdentity } from "./types/session"

export type SessionHygieneSnapshot = ReadonlyMap<string, SessionHygienePayload>

type IdentityTuple = [agent: string, sessionId: string, wslDistro: string | null]

function identitiesFromKey(requestKey: string): LocalSessionIdentity[] {
  return (JSON.parse(requestKey) as IdentityTuple[]).map(([agent, sessionId, wslDistro]) => ({
    agent,
    sessionId,
    wslDistro,
  }))
}

function createSessionHygieneStore(requestKey: string): ExternalStore<SessionHygieneSnapshot> {
  const sessions = identitiesFromKey(requestKey)
  const requestedKeys = new Set(
    sessions.map((session) =>
      localSessionKey(session.agent, session.sessionId, session.wslDistro),
    ),
  )
  let snapshot = new Map(
    sessions.map(
      (session) =>
        [
          localSessionKey(session.agent, session.sessionId, session.wslDistro),
          INITIAL_SESSION_HYGIENE,
        ] as const,
    ),
  )

  const read = async (
    requested: readonly LocalSessionIdentity[],
  ): Promise<SessionHygieneSnapshot | null> => {
    const payloads = await getSessionHygiene(requested)
    if (!payloads) return null

    const next = new Map(snapshot)
    requested.forEach((session, index) => {
      next.set(
        localSessionKey(session.agent, session.sessionId, session.wslDistro),
        payloads[index] ?? INITIAL_SESSION_HYGIENE,
      )
    })
    snapshot = next
    return snapshot
  }

  return createExternalStore<SessionHygieneSnapshot>({
    initial: snapshot,
    load: async () => (sessions.length === 0 ? snapshot : ((await read(sessions)) ?? snapshot)),
    subscribe: async (set) => {
      if (sessions.length === 0) return () => undefined

      let active = true
      let refreshing = false
      const queued = new Map<string, LocalSessionIdentity>()
      const refresh = async (requested: readonly LocalSessionIdentity[]) => {
        for (const session of requested) {
          queued.set(
            localSessionKey(session.agent, session.sessionId, session.wslDistro),
            session,
          )
        }
        if (refreshing) return

        refreshing = true
        while (active && queued.size > 0) {
          const batch = [...queued.values()]
          queued.clear()
          const value = await read(batch).catch(() => null)
          if (active && value) set(value)
        }
        refreshing = false
      }
      const [stopScan, stopInvalidation, stopEntryChange] = await Promise.all([
        onScanEvent((_status, phase) => {
          if (phase === "finished") void refresh(sessions)
        }),
        onSessionsInvalidated(() => void refresh(sessions)),
        onSessionEntryChanged((entry) => {
          const identity = {
            agent: entry.agent,
            sessionId: entry.sessionId,
            wslDistro: entry.wslDistro,
          }
          if (
            requestedKeys.has(
              localSessionKey(identity.agent, identity.sessionId, identity.wslDistro),
            )
          ) {
            void refresh([identity])
          }
        }),
      ])
      return () => {
        active = false
        stopScan()
        stopInvalidation()
        stopEntryChange()
      }
    },
  })
}

/** Read a bounded session set through one IPC batch and one listener set. */
export function useSessionHygiene(
  requestedSessions: readonly LocalSessionIdentity[],
): SessionHygieneSnapshot {
  const requestKey = JSON.stringify([
    ...new Map(
      requestedSessions.map((session) => [
        localSessionKey(session.agent, session.sessionId, session.wslDistro),
        [session.agent, session.sessionId, session.wslDistro ?? null] satisfies IdentityTuple,
      ]),
    ).values(),
  ])
  const store = useMemo(() => createSessionHygieneStore(requestKey), [requestKey])
  return useSyncExternalStore(store.subscribe, store.getSnapshot, store.getSnapshot)
}

/** Select one payload from a batched hygiene snapshot. */
export function sessionHygieneFor(
  snapshot: SessionHygieneSnapshot,
  identity: LocalSessionIdentity,
): SessionHygienePayload {
  return (
    snapshot.get(localSessionKey(identity.agent, identity.sessionId, identity.wslDistro)) ??
    INITIAL_SESSION_HYGIENE
  )
}
