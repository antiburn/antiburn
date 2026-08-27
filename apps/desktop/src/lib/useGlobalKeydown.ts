import { useCallback, useRef } from "react"

import { useExternalSubscription } from "./useExternalSubscription"

/**
 * Listen for `keydown` on `document` or `window` while `active`, without
 * tearing the listener down and rebuilding it whenever `onKeyDown` is a fresh
 * closure (as it usually is, coming straight from a render body).
 *
 * The handler is kept fresh in a ref written during render. That write is
 * safe even though it happens outside an event handler: the ref is read only
 * from inside the event listener, never during render, so there is no
 * tearing — React never reads `handlerRef.current` itself, and the listener
 * only ever sees the ref after this render has committed.
 *
 * The listener itself is attached through `useExternalSubscription`, with
 * `subscribe` memoized on `[active, target]` so it (and the underlying
 * `addEventListener`) is only rebuilt when one of those actually changes,
 * not on every keystroke or every render.
 */
export function useGlobalKeydown(
  active: boolean,
  onKeyDown: (e: KeyboardEvent) => void,
  target: "document" | "window" = "document",
): void {
  const handlerRef = useRef(onKeyDown)
  // Deliberate render-body ref write (the "latest ref" pattern), not a stray
  // one: see the doc comment above for why it is safe here specifically. The
  // lint rule's default assumption — a render-body ref write is almost always
  // a bug — does not hold for a ref that is never read during render.
  // eslint-disable-next-line react-hooks/refs
  handlerRef.current = onKeyDown

  useExternalSubscription(
    useCallback(() => {
      if (!active) return () => undefined
      const source = target === "window" ? window : document
      // `source`'s static type is the `Document | Window` union, which only
      // has the untyped `EventListener` overload of `addEventListener` — the
      // per-event-name overloads that would infer `KeyboardEvent` live on
      // `Document` and `Window` separately. The cast is safe: this listener
      // is only ever attached to the `keydown` event.
      const listener: EventListener = (e) => handlerRef.current(e as KeyboardEvent)
      source.addEventListener("keydown", listener)
      return () => source.removeEventListener("keydown", listener)
    }, [active, target]),
  )
}
