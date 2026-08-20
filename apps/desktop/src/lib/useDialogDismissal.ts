// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { useCallback, type RefObject } from "react";

import { useExternalSubscription } from "./useExternalSubscription"
import { useGlobalKeydown } from "./useGlobalKeydown"

/**
 * The recurring dismissal contract for an anchored dialog/popover: a pointer
 * press outside it closes it, Escape closes it from the keyboard, and — only
 * while `trapFocus` is true — Tab is held inside `trapRef` rather than
 * walking out into the page behind it.
 *
 * Built on {@link useExternalSubscription} for the outside-pointerdown
 * listener and {@link useGlobalKeydown} for Escape/Tab, so both attach only
 * while `active` and tear down together when it goes false or the component
 * unmounts — no bespoke effect required.
 */
export function useDialogDismissal({
  active,
  containerRef,
  trapRef,
  trapFocus,
  focusableSelector,
  onDismiss,
}: {
  /** Whether the dialog is open; both listeners attach only then. */
  active: boolean
  /** A pointerdown outside this element dismisses the dialog. */
  containerRef: RefObject<HTMLElement | null>
  /** The element Tab is held inside while `trapFocus` is true. */
  trapRef: RefObject<HTMLElement | null>
  /** Only a deliberately-opened dialog captures Tab; a passive one must not. */
  trapFocus: boolean
  /** Selector for the elements Tab may land on inside `trapRef`. */
  focusableSelector: string
  onDismiss: () => void
}): void {
  useExternalSubscription(
    useCallback(() => {
      if (!active) return () => {}
      const onPointerDown = (event: PointerEvent) => {
        if (!containerRef.current?.contains(event.target as Node)) onDismiss()
      }
      document.addEventListener("pointerdown", onPointerDown)
      return () => document.removeEventListener("pointerdown", onPointerDown)
    }, [active, containerRef, onDismiss]),
  )

  useGlobalKeydown(active, (event) => {
    if (event.key === "Escape") {
      // Claimed, so a host's own Escape handler does not also dismiss
      // something behind this dialog: one key press closes one thing.
      event.preventDefault()
      onDismiss()
      return
    }
    // Tab is only held inside a dialog opened on purpose. A passive one is
    // not modal and must not capture the key.
    if (event.key !== "Tab" || !trapFocus) return

    const panel = trapRef.current
    if (!panel) return
    const focusable = Array.from(panel.querySelectorAll<HTMLElement>(focusableSelector))
    // A panel with nothing to focus still holds the key, or Tab would walk
    // out of a dialog the reader has not dismissed.
    if (focusable.length === 0) {
      event.preventDefault()
      return
    }
    const first = focusable[0]
    const last = focusable[focusable.length - 1]
    const activeElement = document.activeElement
    if (event.shiftKey && (activeElement === first || !panel.contains(activeElement))) {
      event.preventDefault()
      last?.focus()
    } else if (!event.shiftKey && (activeElement === last || !panel.contains(activeElement))) {
      event.preventDefault()
      first?.focus()
    }
  })
}
