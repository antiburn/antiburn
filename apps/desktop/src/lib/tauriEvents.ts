import { listen as tauriListen, type UnlistenFn } from "@tauri-apps/api/event"

export type { UnlistenFn } from "@tauri-apps/api/event"

export const UNLISTEN_RETRY_DELAY_MS = 50
export const UNLISTEN_MAX_ATTEMPTS = 20

/**
 * Tauri's `listen`, with an unlisten that does not reject. Use this instead of
 * importing `listen` from `@tauri-apps/api/event`.
 *
 * Tauri adds the listener to the page with a separate script. An unlisten
 * that runs before that script throws, and the shell keeps its side of the
 * listener. This happens when a store stops right after it starts, as React
 * StrictMode does on the first render. The unlisten that this function returns
 * tries again until the page has the listener. A second call does nothing.
 */
export async function listen<T>(
  ...args: Parameters<typeof tauriListen<T>>
): Promise<UnlistenFn> {
  const unlisten = await tauriListen<T>(...args)
  let released = false
  return () => {
    if (released) return
    released = true
    release(unlisten, 1)
  }
}

function release(unlisten: UnlistenFn, attempt: number): void {
  let result: Promise<void>
  try {
    result = Promise.resolve(unlisten())
  } catch (error) {
    result = Promise.reject(error)
  }
  result.catch(() => {
    if (attempt >= UNLISTEN_MAX_ATTEMPTS) return
    setTimeout(() => release(unlisten, attempt + 1), UNLISTEN_RETRY_DELAY_MS)
  })
}
