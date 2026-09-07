/**
 * Sandbox replacement for `@tauri-apps/api/event`.
 *
 * `listen` keeps each handler by event name. The page can push an event with
 * `window.__sandbox.emit(name, payload)` from the console or from the overlay.
 */

export type UnlistenFn = () => void

interface SandboxEvent<T> {
  event: string
  id: number
  payload: T
}

type Handler<T> = (event: SandboxEvent<T>) => void

const handlers = new Map<string, Set<Handler<unknown>>>()
let nextEventId = 0

export async function listen<T>(event: string, handler: Handler<T>): Promise<UnlistenFn> {
  const set = handlers.get(event) ?? new Set<Handler<unknown>>()
  set.add(handler as Handler<unknown>)
  handlers.set(event, set)
  return () => {
    set.delete(handler as Handler<unknown>)
  }
}

/** Deliver `payload` to every handler of `event`. Returns the handler count. */
export function emit(event: string, payload: unknown): number {
  const set = handlers.get(event)
  if (!set) return 0
  nextEventId += 1
  for (const handler of set) handler({ event, id: nextEventId, payload })
  return set.size
}

/** The event names that have at least one handler. */
function listeners(): string[] {
  return [...handlers.entries()].filter(([, set]) => set.size > 0).map(([name]) => name)
}

declare global {
  interface Window {
    __sandbox?: { emit: typeof emit; listeners: typeof listeners }
  }
}

window.__sandbox = { emit, listeners }
