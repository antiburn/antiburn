const retryDelays = [150, 400]
const extendedRetryDelays = [...retryDelays, 1000, 2000]

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
      return "Cloud access changed. Check your sign-in or workspace access."
  }
}

/** A local listing can miss authority that a concurrent workspace check renews. */
export function cloudReadNeedsVerifiedScope(error: unknown) {
  const message = error instanceof Error ? error.message : error
  return message === "Cloud account changed." || message === "Cloud workspace sign-in required."
}

/** Retry reads only. Abort the wait when the account or source scope changes. */
export async function retryCloudRead<T>(
  read: () => Promise<T>,
  signal: AbortSignal,
  retryResult: (value: T) => boolean = () => false,
  onExtendedRetry?: () => void,
): Promise<T> {
  const delays = onExtendedRetry ? extendedRetryDelays : retryDelays
  for (let attempt = 0; ; attempt++) {
    signal.throwIfAborted()
    const delay = delays[attempt]
    try {
      const value = await read()
      if (delay === undefined || !retryResult(value)) return value
    } catch (error) {
      if (delay === undefined || cloudReadScopeMessage(error) !== undefined) throw error
    }
    signal.throwIfAborted()
    if (attempt === retryDelays.length) onExtendedRetry?.()
    await waitForReadRetry(delay, signal)
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
