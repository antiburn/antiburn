import { useCallback, useState, useSyncExternalStore } from "react"

import type { SessionListEntry } from "../components/session/SessionList"
import { onCheckAvailabilityChanged } from "./checkAvailability"
import {
  getSessionHygiene,
  onChecksReportChanged,
  type SessionHygienePayload,
} from "./insightsIpc"
import { onSessionIndexChanged, onSessionUpdated } from "./ipc"
import { localSessionKey } from "./presentation/localIdentity"
import { INITIAL_SESSION_HYGIENE } from "./presentation/sessionHygiene"
import type { LocalSessionIdentity } from "./types/session"

export type SessionHygieneSnapshot = ReadonlyMap<string, SessionHygienePayload>

const FAILED_HYGIENE_READ: SessionHygienePayload = {
  ...INITIAL_SESSION_HYGIENE,
  evidenceState: "failed",
}

/** Share badge reads and the latest cache with actions that outlive one render. */
export interface SessionHygieneResource {
  snapshot: SessionHygieneSnapshot
  getSnapshot: () => SessionHygieneSnapshot
  /** Publish completed badges and report whether every requested badge was read. */
  resolve: (
    identities: readonly LocalSessionIdentity[],
    signal: AbortSignal,
    retained: () => readonly LocalSessionIdentity[] | null,
  ) => Promise<boolean>
}

/**
 * Every entry with a transcript id, as the identity a hygiene fetch needs.
 *
 * A caller that lifts `useSessionHygiene` above a filtered or grouped view
 * uses this over its full, unfiltered entry list, so the request key does not
 * change when a filter or day-window grouping changes what actually renders.
 */
export function sessionHygieneIdentities(
  entries: readonly SessionListEntry[],
): LocalSessionIdentity[] {
  return entries.flatMap((entry) =>
    entry.sessionId
      ? [
          {
            agent: entry.agent,
            sessionId: entry.sessionId,
            wslDistro: entry.wslDistro ?? null,
            remoteHostId: entry.remoteHostId ?? null,
          },
        ]
      : [],
  )
}

type IdentityTuple = [
  agent: string,
  sessionId: string,
  wslDistro: string | null,
  remoteHostId: string | null,
]

function identitiesFromKey(requestKey: string): LocalSessionIdentity[] {
  return (JSON.parse(requestKey) as IdentityTuple[]).map(
    ([agent, sessionId, wslDistro, remoteHostId]) => ({
      agent,
      sessionId,
      wslDistro,
      remoteHostId,
    }),
  )
}

function identityKey(session: LocalSessionIdentity): string {
  return localSessionKey(
    session.agent,
    session.sessionId,
    session.wslDistro,
    session.remoteHostId,
  )
}

/** Own one subscription set and one bounded-to-request cache per hook instance. */
function createSessionHygieneStore() {
  let snapshot: SessionHygieneSnapshot = new Map()
  const unfinished = new Set<string>()
  const publishedReads = new Map<string, number>()
  let readSequence = 0
  let notifyLatest: (() => void) | undefined

  function publish(
    batch: readonly LocalSessionIdentity[],
    payloads: readonly SessionHygienePayload[] | null,
    sequence: number,
    retained: ReadonlyMap<string, LocalSessionIdentity>,
  ) {
    const next = new Map(snapshot)
    batch.forEach((session, index) => {
      const key = identityKey(session)
      if (!retained.has(key)) return
      const payload = payloads?.[index]
      // A background read and a navigation read can finish in either order.
      if (payload && sequence > (publishedReads.get(key) ?? 0)) {
        publishedReads.set(key, sequence)
        next.set(key, payload)
      } else if (!next.has(key)) {
        next.set(key, FAILED_HYGIENE_READ)
      }
    })
    snapshot = next
    notifyLatest?.()
  }

  return {
    getSnapshot: () => snapshot,
    async resolve(
      identities: readonly LocalSessionIdentity[],
      signal: AbortSignal,
      retained: () => readonly LocalSessionIdentity[] | null,
    ): Promise<boolean> {
      if (signal.aborted) return false
      const sequence = ++readSequence
      const payloads = await getSessionHygiene(identities, signal).catch(() => null)
      const current = retained()
      if (signal.aborted || !current || !payloads) return false
      publish(
        identities.slice(0, payloads.length),
        payloads,
        sequence,
        new Map(current.map((session) => [identityKey(session), session])),
      )
      return payloads.length === identities.length
    },
    subscribe(requestKey: string, notify: () => void) {
      notifyLatest = notify
      const sessions = identitiesFromKey(requestKey)
      const identities = new Map(sessions.map((session) => [identityKey(session), session]))
      const missing = sessions.filter((session) => {
        const key = identityKey(session)
        const cached = snapshot.get(key)
        return unfinished.has(key) || !cached || cached === FAILED_HYGIENE_READ
      })
      for (const key of unfinished) if (!identities.has(key)) unfinished.delete(key)
      for (const key of publishedReads.keys())
        if (!identities.has(key)) publishedReads.delete(key)
      const retained = new Map([...snapshot].filter(([key]) => identities.has(key)))
      if (retained.size !== snapshot.size) snapshot = retained
      if (sessions.length === 0)
        return () => {
          notifyLatest = undefined
        }

      const controller = new AbortController()
      const { signal } = controller
      let refreshing = false
      const queued = new Map<string, LocalSessionIdentity>()
      async function refresh(requested: readonly LocalSessionIdentity[]) {
        if (signal.aborted) return
        for (const session of requested) {
          const key = identityKey(session)
          queued.set(key, session)
          unfinished.add(key)
        }
        if (refreshing) return
        refreshing = true
        while (!signal.aborted && queued.size > 0) {
          const batch = [...queued.values()]
          queued.clear()
          const sequence = ++readSequence
          const payloads = await getSessionHygiene(batch, signal).catch(() => null)
          if (signal.aborted) break
          for (const session of batch) {
            const key = identityKey(session)
            if (!queued.has(key)) unfinished.delete(key)
          }
          publish(batch, payloads, sequence, identities)
        }
        refreshing = false
      }

      const stops = [
        onSessionIndexChanged((change) => {
          if (change.cause !== "scan_pass") void refresh(sessions)
        }),
        onSessionUpdated((update) => {
          if (!update.facets.analysis && !update.facets.checks) return
          const identity = identities.get(identityKey(update.entry))
          if (identity) void refresh([identity])
        }),
        onChecksReportChanged((change) => {
          if (change !== "summaryOnly") void refresh(sessions)
        }),
        onCheckAvailabilityChanged((event) => {
          if (event.status === "updated") void refresh(sessions)
        }),
      ]
      const unlisten: (() => void)[] = []
      void Promise.all(
        stops.map(async (pending) => {
          const stop = await pending.catch(() => null)
          if (stop && !signal.aborted) unlisten.push(stop)
          else stop?.()
        }),
      ).then(() => refresh(missing))
      return () => {
        notifyLatest = undefined
        controller.abort()
        for (const stop of unlisten) stop()
      }
    },
  }
}

/** Reuse resolved hygiene and provide targeted reads for the current inventory. */
export function useSessionHygieneResource(
  requestedSessions: readonly LocalSessionIdentity[],
): SessionHygieneResource {
  const requestKey = JSON.stringify([
    ...new Map(
      requestedSessions.map((session) => [
        identityKey(session),
        [
          session.agent,
          session.sessionId,
          session.wslDistro ?? null,
          session.remoteHostId ?? null,
        ] satisfies IdentityTuple,
      ]),
    ).values(),
  ])
  const [store] = useState(createSessionHygieneStore)
  const subscribe = useCallback(
    (notify: () => void) => store.subscribe(requestKey, notify),
    [requestKey, store],
  )
  const snapshot = useSyncExternalStore(subscribe, store.getSnapshot, store.getSnapshot)
  return { snapshot, getSnapshot: store.getSnapshot, resolve: store.resolve }
}

/** Reuse resolved hygiene for identities retained when the requested set changes. */
export function useSessionHygiene(
  requestedSessions: readonly LocalSessionIdentity[],
): SessionHygieneSnapshot {
  return useSessionHygieneResource(requestedSessions).snapshot
}

/** Select one payload from a batched hygiene snapshot. */
export function sessionHygieneFor(
  snapshot: SessionHygieneSnapshot,
  identity: LocalSessionIdentity,
): SessionHygienePayload {
  return (
    snapshot.get(
      localSessionKey(
        identity.agent,
        identity.sessionId,
        identity.wslDistro,
        identity.remoteHostId,
      ),
    ) ?? INITIAL_SESSION_HYGIENE
  )
}
