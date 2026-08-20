// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { useCallback, useSyncExternalStore, type RefObject } from "react";

/**
 * Track the rendered width of `ref`'s element, re-rendering as it resizes.
 *
 * `getSnapshot` reads `getBoundingClientRect().width` fresh on every call,
 * which is fine to do unconditionally: the return value is a plain number, so
 * React's "did the snapshot change" check (`Object.is`) is comparing two
 * primitives rather than two object identities, and it is stable between
 * renders exactly when the layout hasn't changed — there is no caching
 * subtlety to get wrong here the way there would be for an object or array
 * snapshot.
 *
 * `subscribe` attaches a `ResizeObserver` to whatever element `ref` points to
 * *at subscribe time*. That is enough to cover the initial measurement too,
 * with no separate kick needed: `useSyncExternalStore` re-reads `getSnapshot`
 * right after `subscribe` runs on mount, which is after refs have attached to
 * their elements, so the first render's stale "not measured yet" value is
 * corrected before the reader ever sees it.
 *
 * No-ops (attaches nothing, cleans up nothing) when the ref has no element yet
 * or `ResizeObserver` doesn't exist — jsdom, which the test environment uses,
 * has no implementation of it.
 */
export function useElementWidth(ref: RefObject<HTMLElement | null>): number {
  const subscribe = useCallback(
    (onChange: () => void) => {
      const element = ref.current
      if (!element || typeof ResizeObserver === "undefined") return () => {}
      const observer = new ResizeObserver(onChange)
      observer.observe(element)
      return () => observer.disconnect()
    },
    [ref],
  )

  const getSnapshot = useCallback(() => ref.current?.getBoundingClientRect().width ?? 0, [ref])

  return useSyncExternalStore(subscribe, getSnapshot, getServerSnapshot)
}

function getServerSnapshot(): number {
  return 0
}
