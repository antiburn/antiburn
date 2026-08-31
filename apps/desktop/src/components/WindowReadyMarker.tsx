import type { ReactNode } from "react"

import { hasShell, windowReady } from "../lib/ipc"

declare global {
  interface Window {
    readonly __ANTIBURN_WINDOW_GENERATION__?: number
  }
}

function reportReady(node: HTMLSpanElement | null): void {
  if (!node) return
  const generation = window.__ANTIBURN_WINDOW_GENERATION__
  if (typeof generation === "number" && Number.isSafeInteger(generation)) {
    void windowReady(generation).catch(() => undefined)
    return
  }
  // The shell injects the generation before the page loads. A shell page
  // without the value cannot report readiness, so the window stays hidden.
  // A plain browser has no shell and no generation; stay quiet there.
  if (hasShell()) {
    console.warn(
      "WindowReadyMarker: window.__ANTIBURN_WINDOW_GENERATION__ is missing; the window cannot report readiness",
    )
  }
}

/** Reports readiness after React commits the window shell. */
function WindowReadyMarker() {
  return <span ref={reportReady} hidden aria-hidden data-window-ready-marker />
}

/** Places the readiness marker after every callback ref in the window view. */
export function WindowReadyBoundary({ children }: { children: ReactNode }) {
  return (
    <>
      {children}
      <WindowReadyMarker />
    </>
  )
}
