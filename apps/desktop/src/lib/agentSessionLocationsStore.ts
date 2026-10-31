import { agentSessionLocations, type AgentSessionLocations } from "./ipc"
import { scanStatusStore } from "./scanStatusStore"

let cache: AgentSessionLocations[] | null = null
let inFlight: Promise<void> | null = null
let revision = 0
let stopScan: (() => void) | null = null
const listeners = new Set<() => void>()

function notify(): void {
  for (const listener of listeners) listener()
}

function refreshAgentSessionLocations(): Promise<void> {
  if (listeners.size === 0) return Promise.resolve()
  if (inFlight) return inFlight
  const requestRevision = revision
  inFlight = agentSessionLocations()
    .then((locations) => {
      if (requestRevision === revision) cache = locations
    })
    .catch((error: unknown) => {
      if (requestRevision === revision) cache ??= []
      throw error
    })
    .finally(() => {
      inFlight = null
      if (requestRevision === revision) notify()
      else void refreshAgentSessionLocations().catch(() => undefined)
    })
  return inFlight
}

export function subscribeAgentSessionLocations(listener: () => void): () => void {
  listeners.add(listener)
  if (listeners.size === 1) {
    revision += 1
    let previous = scanStatusStore.getSnapshot()
    stopScan = scanStatusStore.subscribe(() => {
      const status = scanStatusStore.getSnapshot()
      const finished =
        status &&
        !status.running &&
        (previous?.running ||
          (status.finishedAt !== null && status.finishedAt !== previous?.finishedAt))
      previous = status
      if (finished) {
        revision += 1
        void refreshAgentSessionLocations().catch(() => undefined)
      }
    })
    void refreshAgentSessionLocations().catch(() => undefined)
  }
  return () => {
    listeners.delete(listener)
    if (listeners.size === 0) {
      revision += 1
      stopScan?.()
      stopScan = null
    }
  }
}

/** Null means the initial root read has not completed. */
export function agentSessionLocationsSnapshot(): AgentSessionLocations[] | null {
  return cache
}
