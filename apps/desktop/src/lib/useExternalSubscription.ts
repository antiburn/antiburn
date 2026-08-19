// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { useSyncExternalStore } from "react"

// A constant snapshot: this hook exists purely for the subscribe/unsubscribe
// lifecycle React runs around `useSyncExternalStore`, not for the value it
// reads. Returning the same literal on every call means the "did the
// snapshot change" check is always false, so nothing here ever re-renders.
const CONSTANT_SNAPSHOT = 0
function getSnapshot(): number {
  return CONSTANT_SNAPSHOT
}
function getServerSnapshot(): number {
  return CONSTANT_SNAPSHOT
}

/**
 * Run `subscribe` for as long as the component is mounted, using
 * `useSyncExternalStore` purely for its attach/detach lifecycle rather than
 * for any value it produces.
 *
 * This is the general-purpose replacement for a mount/unmount
 * `useEffect(() => { ...; return cleanup; }, [])`: React calls `subscribe`
 * after commit, keeps its returned cleanup around, and calls that cleanup
 * before the next `subscribe` call or on unmount — exactly the timing an
 * effect would give, without an effect.
 *
 * `subscribe` **must be referentially stable** (wrap it in `useCallback` with
 * the right dependency array) or React tears it down and re-runs it on every
 * render, which for most callers means attaching and detaching a listener or
 * timer every render. That resubscribe-on-identity-change behavior is not
 * always a bug, though: if `subscribe` is deliberately built from a
 * dependency (a keyed subscription — "listen to *this* channel"), then
 * resubscribing whenever that key changes is exactly the desired behavior,
 * and is the same tradeoff `useEffect`'s dependency array makes.
 */
export function useExternalSubscription(subscribe: () => () => void): void {
  useSyncExternalStore(subscribe, getSnapshot, getServerSnapshot)
}
