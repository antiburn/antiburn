import type { ReactNode } from "react"

import { windowReady } from "../lib/ipc"

declare global {
  interface Window {
    readonly __ANTIBURN_WINDOW_GENERATION__?: number
  }
}

function reportReady(node: HTMLSpanElement | null): void {
  const generation = window.__ANTIBURN_WINDOW_GENERATION__
  if (node && typeof generation === "number" && Number.isSafeInteger(generation)) {
    void windowReady(generation).catch(() => undefined)
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
