/**
 * The recurring shape behind the app's `onXxx` IPC helpers (see
 * `lib/ipc.ts`): fetch a value once, then subscribe to a push channel that
 * resolves asynchronously — a Tauri `listen()` call is itself a promise — and
 * publishes updates from then on. `OnboardingSession` hand-rolls this shape
 * for one view; this factory is the same lifecycle generalized so a
 * `useSyncExternalStore` consumer for it does not have to be written by hand
 * each time.
 *
 * Ref-counted: the store is idle with no work outstanding until the first
 * `subscribe` listener arrives, and tears itself back down when the last one
 * leaves. A component that mounts, unmounts, and remounts gets a fresh
 * `load()` + `subscribe()` pass each time, exactly as an effect with an empty
 * dependency array would.
 */
export type ExternalStore<T> = {
  getSnapshot: () => T
  /** Ref-counted: the first listener starts the store, the last stops it. */
  subscribe: (listener: () => void) => () => void
  /** Re-run `load` (if any) and publish its result. */
  refresh: () => Promise<void>
  /** Publish a value directly, bypassing `load`. */
  set: (value: T) => void
}

export type ExternalStoreConfig<T> = {
  initial: T
  /**
   * Fetched once when the store starts, after `subscribe` is attached. A
   * push the channel delivers before this resolves wins over its result —
   * see the `revision` guard in `start()`.
   */
  load?: () => Promise<T>
  /** Restore `initial` after the final listener leaves. */
  resetOnStop?: boolean
  /**
   * Attach a push channel that calls `set` with every update it sees, and
   * resolve to the function that detaches it. Modeled on the app's `onXxx`
   * IPC helpers, which are themselves `async () => UnlistenFn`. Attached
   * before `load` runs, so a push that arrives during the load, or during
   * this attachment's own handshake, is never missed.
   */
  subscribe?: (set: (value: T) => void) => Promise<() => void>
}

export function createExternalStore<T>(config: ExternalStoreConfig<T>): ExternalStore<T> {
  let snapshot = config.initial
  const listeners = new Set<() => void>()
  let started = false
  // Bumped every stop, so a subscribe() that resolves after a later stop (or
  // a stop-then-restart) can tell its own attempt is stale.
  let generation = 0
  let unlisten: (() => void) | null = null
  // Bumped every publish, so a load() or refresh() in flight can tell a push
  // already landed a newer value while it waited, and skip overwriting it.
  let revision = 0
  // Bumped every config.load() call, so an earlier load() or refresh() that
  // resolves after a later one can tell it is not the latest, and skip
  // overwriting the later result. Together with `revision`, a load result
  // publishes only when no push landed and no later load started while it
  // was in flight.
  let loadRequest = 0

  function publish(value: T): void {
    snapshot = value
    revision += 1
    for (const listener of listeners) listener()
  }

  async function start(): Promise<void> {
    started = true
    const thisGeneration = generation
    const thisRevision = revision

    if (config.subscribe) {
      const stop = await config
        .subscribe((value) => {
          // A push from a generation this store has already left behind
          // (the last listener left, or the store restarted) must not
          // resurrect a snapshot nobody is reading any more.
          if (thisGeneration !== generation) return
          publish(value)
        })
        .catch(() => null)
      if (thisGeneration !== generation) {
        // The last listener left (or the store was restarted) while the
        // channel was still connecting. There is nothing left to publish
        // into, so detach it immediately rather than leaking it.
        stop?.()
        return
      }
      unlisten = stop
    }

    if (config.load) {
      const thisLoadRequest = ++loadRequest
      const value = await config.load().catch(() => undefined)
      if (thisGeneration !== generation) return
      // Publish only when no push landed (revision unchanged) and no later
      // load() or refresh() started (loadRequest unchanged) while this one
      // was in flight — either means a newer value already won.
      if (value !== undefined && revision === thisRevision && loadRequest === thisLoadRequest) {
        publish(value)
      }
    }
  }

  function stop(): void {
    started = false
    generation += 1
    unlisten?.()
    unlisten = null
    if (config.resetOnStop) snapshot = config.initial
  }

  return {
    getSnapshot: () => snapshot,

    subscribe(listener) {
      listeners.add(listener)
      if (!started) void start()
      return () => {
        listeners.delete(listener)
        if (listeners.size === 0) stop()
      }
    },

    refresh: async () => {
      if (!config.load) return
      const thisRevision = revision
      const thisLoadRequest = ++loadRequest
      const value = await config.load()
      // Same guard as `start()`'s load: a push that landed, or a later
      // load()/refresh() that started, while this one was in flight is
      // newer than its result.
      if (revision === thisRevision && loadRequest === thisLoadRequest) publish(value)
    },

    set(value: T) {
      publish(value)
    },
  }
}
