/**
 * The folders that antiburn reads for each agent's sessions, for the Agents
 * step settings list. The folders do not change while the app runs, so the
 * store reads them one time and keeps them.
 */
import { agentSessionLocations, type AgentSessionLocations } from "./ipc"

let cache: AgentSessionLocations[] | null = null
let inFlight: Promise<void> | null = null
const listeners = new Set<() => void>()

function startLoad(): void {
  if (cache !== null || inFlight) return
  inFlight = agentSessionLocations()
    .then((locations) => {
      cache = locations
    })
    .catch(() => {
      cache = []
    })
    .finally(() => {
      inFlight = null
      for (const listener of listeners) listener()
    })
}

/** `useSyncExternalStore` subscribe: starts the one-shot load on first use. */
export function subscribeAgentSessionLocations(listener: () => void): () => void {
  startLoad()
  listeners.add(listener)
  return () => listeners.delete(listener)
}

/** `null` while the first read is still in flight or has not started. */
export function agentSessionLocationsSnapshot(): AgentSessionLocations[] | null {
  return cache
}
