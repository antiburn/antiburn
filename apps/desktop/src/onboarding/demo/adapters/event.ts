type Listener<T> = (event: { payload: T }) => void

const listeners = new Map<string, Set<Listener<unknown>>>()

export type UnlistenFn = () => void

export async function listen<T>(event: string, listener: Listener<T>): Promise<UnlistenFn> {
  const eventListeners = listeners.get(event) ?? new Set<Listener<unknown>>()
  eventListeners.add(listener as Listener<unknown>)
  listeners.set(event, eventListeners)
  return () => eventListeners.delete(listener as Listener<unknown>)
}

export function emitFixtureEvent<T>(event: string, payload: T): void {
  for (const listener of listeners.get(event) ?? []) listener({ payload })
}

window.__ANTIBURN_VISUAL_EMIT__ = emitFixtureEvent
