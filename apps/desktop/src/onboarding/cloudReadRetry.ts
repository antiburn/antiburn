const retryDelays = [150, 400]
const maxDelay = 30_000

/** Desktop no longer recognizes the account bound to this read. */
export function invalidCloudReadScope(error: unknown) {
  return cloudReadScopeMessage(error) !== undefined
}

/** Map only closed desktop errors. Never show raw command errors. */
export function cloudReadScopeMessage(error: unknown) {
  const message = error instanceof Error ? error.message : error
  switch (message) {
    case "Cloud workspace sign-in required.":
      return "Sign in again to check Cloud workspaces."
    case "Cloud workspace access denied.":
      return "You do not have access to the selected Cloud workspace."
    case "The signed-in workspace is not available for selection.":
      return "The selected Cloud workspace is no longer available."
    case "Cloud account changed.":
      return cloudReadScopeChangedMessage
  }
}

export const cloudReadScopeChangedMessage =
  "Cloud access changed. Check your sign-in or workspace access."

/** Retry reads only. Abort the wait when the account or source scope changes. */
export async function retryCloudRead<T>(
  read: () => Promise<T>,
  signal: AbortSignal,
  retryResult: (value: T) => boolean = () => false,
  onExtendedRetry?: () => void,
): Promise<T> {
  for (let attempt = 0; ; attempt++) {
    signal.throwIfAborted()
    const last = !onExtendedRetry && attempt === retryDelays.length
    try {
      const value = await read()
      if (last || !retryResult(value)) return value
    } catch (error) {
      if (last || invalidCloudReadScope(error)) throw error
    }
    signal.throwIfAborted()
    if (attempt === retryDelays.length) onExtendedRetry?.()
    await waitForReadRetry(
      retryDelays[attempt] ?? Math.min(maxDelay, 1000 * 2 ** (attempt - retryDelays.length)),
      signal,
    )
  }
}

/** Release the timer immediately when the pane exits. */
function waitForReadRetry(delay: number, signal: AbortSignal) {
  return new Promise<void>((resolve) => {
    const finish = () => {
      clearTimeout(timer)
      signal.removeEventListener("abort", finish)
      resolve()
    }
    const timer = setTimeout(finish, delay)
    signal.addEventListener("abort", finish, { once: true })
    if (signal.aborted) finish()
  })
}
