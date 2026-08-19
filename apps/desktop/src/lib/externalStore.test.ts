// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { describe, expect, it, vi } from 'vitest';

import { createExternalStore } from './externalStore';

/** A promise plus the functions that settle it, for controlling timing by hand. */
function deferred<T>(): {
  promise: Promise<T>;
  resolve: (value: T) => void;
} {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

/**
 * Let every already-queued promise chain settle, however many microtask hops
 * long. A macrotask always runs after the whole microtask queue drains, so
 * this is more robust than guessing how many `await Promise.resolve()` hops a
 * given chain needs.
 */
async function flush(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

describe('createExternalStore', () => {
  it('is idle until the first listener subscribes, and stops once the last leaves', async () => {
    const unlisten = vi.fn();
    const subscribe = vi.fn(async () => unlisten);
    const store = createExternalStore({ initial: 0, subscribe });

    expect(subscribe).not.toHaveBeenCalled();

    const unsubscribeA = store.subscribe(() => {});
    const unsubscribeB = store.subscribe(() => {});
    await flush();

    // One listener started the store; a second joining does not start it again.
    expect(subscribe).toHaveBeenCalledTimes(1);
    expect(unlisten).not.toHaveBeenCalled();

    unsubscribeA();
    expect(unlisten).not.toHaveBeenCalled();

    unsubscribeB();
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it('publishes what load resolves to before subscribers see the first snapshot change', async () => {
    const load = vi.fn(async () => 'loaded');
    const store = createExternalStore({ initial: 'initial', load });

    expect(store.getSnapshot()).toBe('initial');

    const listener = vi.fn();
    store.subscribe(listener);
    await flush();

    expect(store.getSnapshot()).toBe('loaded');
    expect(listener).toHaveBeenCalled();
  });

  it('unlistens immediately when the async subscribe resolves after the store has already stopped', async () => {
    const unlisten = vi.fn();
    const pending = deferred<() => void>();
    const subscribe = vi.fn(() => pending.promise);
    const store = createExternalStore({ initial: 0, subscribe });

    const unsubscribe = store.subscribe(() => {});
    // The last listener leaves while the channel is still connecting.
    unsubscribe();
    expect(unlisten).not.toHaveBeenCalled();

    // The channel finally connects, after nobody is listening any more.
    pending.resolve(unlisten);
    await flush();

    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it('restarting after a stop discards a subscribe that was still connecting for the old run', async () => {
    const firstUnlisten = vi.fn();
    const secondUnlisten = vi.fn();
    const firstPending = deferred<() => void>();
    const subscribe = vi
      .fn()
      .mockImplementationOnce(() => firstPending.promise)
      .mockImplementationOnce(async () => secondUnlisten);
    const store = createExternalStore({ initial: 0, subscribe });

    const stopFirstRun = store.subscribe(() => {});
    stopFirstRun();

    // A fresh run starts before the first run's subscribe has resolved.
    store.subscribe(() => {});
    await flush();
    expect(secondUnlisten).not.toHaveBeenCalled();

    // The stale first run's channel connects late; it must be torn straight
    // back down rather than becoming the active run's unlisten.
    firstPending.resolve(firstUnlisten);
    await flush();

    expect(firstUnlisten).toHaveBeenCalledTimes(1);
    expect(secondUnlisten).not.toHaveBeenCalled();
  });

  it('refresh re-runs load and publishes the result', async () => {
    const load = vi.fn().mockResolvedValueOnce('first').mockResolvedValueOnce('second');
    const store = createExternalStore({ initial: 'initial', load });

    store.subscribe(() => {});
    await flush();
    expect(store.getSnapshot()).toBe('first');

    await store.refresh();
    expect(load).toHaveBeenCalledTimes(2);
    expect(store.getSnapshot()).toBe('second');
  });

  it('set publishes a value directly, without going through load', async () => {
    const load = vi.fn(async () => 'from load');
    const store = createExternalStore({ initial: 'initial', load });
    const listener = vi.fn();
    store.subscribe(listener);
    await flush();
    listener.mockClear();

    store.set('direct');

    expect(store.getSnapshot()).toBe('direct');
    expect(listener).toHaveBeenCalledTimes(1);
  });
});
