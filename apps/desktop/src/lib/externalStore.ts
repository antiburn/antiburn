// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

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
  getSnapshot: () => T;
  /** Ref-counted: the first listener starts the store, the last stops it. */
  subscribe: (listener: () => void) => () => void;
  /** Re-run `load` (if any) and publish its result. */
  refresh: () => Promise<void>;
  /** Publish a value directly, bypassing `load`. */
  set: (value: T) => void;
};

export type ExternalStoreConfig<T> = {
  initial: T;
  /** Fetched once when the store starts, before `subscribe` is awaited. */
  load?: () => Promise<T>;
  /**
   * Attach a push channel that calls `set` with every update it sees, and
   * resolve to the function that detaches it. Modeled on the app's `onXxx`
   * IPC helpers, which are themselves `async () => UnlistenFn`.
   */
  subscribe?: (set: (value: T) => void) => Promise<() => void>;
};

export function createExternalStore<T>(config: ExternalStoreConfig<T>): ExternalStore<T> {
  let snapshot = config.initial;
  const listeners = new Set<() => void>();
  let started = false;
  // Bumped every stop, so a subscribe() that resolves after a later stop (or
  // a stop-then-restart) can tell its own attempt is stale.
  let generation = 0;
  let unlisten: (() => void) | null = null;

  function publish(value: T): void {
    snapshot = value;
    for (const listener of listeners) listener();
  }

  async function start(): Promise<void> {
    started = true;
    const thisGeneration = generation;

    if (config.load) {
      const value = await config.load().catch(() => undefined);
      if (thisGeneration !== generation) return;
      if (value !== undefined) publish(value);
    }

    if (config.subscribe) {
      const stop = await config.subscribe(publish).catch(() => null);
      if (thisGeneration !== generation) {
        // The last listener left (or the store was restarted) while the
        // channel was still connecting. There is nothing left to publish
        // into, so detach it immediately rather than leaking it.
        stop?.();
        return;
      }
      unlisten = stop;
    }
  }

  function stop(): void {
    started = false;
    generation += 1;
    unlisten?.();
    unlisten = null;
  }

  return {
    getSnapshot: () => snapshot,

    subscribe(listener) {
      listeners.add(listener);
      if (!started) void start();
      return () => {
        listeners.delete(listener);
        if (listeners.size === 0) stop();
      };
    },

    refresh: async () => {
      if (!config.load) return;
      publish(await config.load());
    },

    set(value: T) {
      publish(value);
    },
  };
}
