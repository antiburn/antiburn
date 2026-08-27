import { useMemo, useSyncExternalStore } from "react"

import { createExternalStore } from "./externalStore"
import { getSessionHygiene, type SessionHygienePayload } from "./insightsIpc"
import { onScanEvent, onSessionsInvalidated } from "./ipc"
import { INITIAL_SESSION_HYGIENE } from "./presentation/sessionHygiene"

const REFRESH_INTERVAL_MS = 60_000

/** Read one session's badges and refresh them at the evidence update boundaries. */
export function useSessionHygiene(
  agent: string,
  sessionId: string | undefined,
  wslDistro?: string | null,
): SessionHygienePayload {
  const store = useMemo(() => {
    const load = async () =>
      sessionId
        ? ((await getSessionHygiene(agent, sessionId, wslDistro)) ?? INITIAL_SESSION_HYGIENE)
        : INITIAL_SESSION_HYGIENE

    return createExternalStore<SessionHygienePayload>({
      initial: INITIAL_SESSION_HYGIENE,
      load,
      subscribe: async (set) => {
        if (!sessionId) return () => undefined

        let active = true
        let refreshing = false
        const refresh = async () => {
          if (refreshing) return
          refreshing = true
          const value = await load().catch(() => undefined)
          refreshing = false
          if (active && value) set(value)
        }
        const interval = window.setInterval(() => void refresh(), REFRESH_INTERVAL_MS)
        const [stopScan, stopInvalidation] = await Promise.all([
          onScanEvent((_status, phase) => {
            if (phase === "finished") void refresh()
          }),
          onSessionsInvalidated(() => void refresh()),
        ])
        return () => {
          active = false
          window.clearInterval(interval)
          stopScan()
          stopInvalidation()
        }
      },
    })
  }, [agent, sessionId, wslDistro])

  return useSyncExternalStore(store.subscribe, store.getSnapshot, store.getSnapshot)
}
