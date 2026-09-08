import type { ReactNode } from "react"

import { windowReady } from "../lib/ipc"

declare global {
  interface Window {
    readonly __ANTIBURN_WINDOW_GENERATION__?: number
  }
}

export type WindowReadyReporter = (generation: number) => Promise<void>

function reportReady(node: HTMLSpanElement | null, reporter: WindowReadyReporter): void {
  const generation = window.__ANTIBURN_WINDOW_GENERATION__
  if (node && typeof generation === "number" && Number.isSafeInteger(generation)) {
    void reporter(generation).catch(() => undefined)
  }
}

/** Reports readiness after React commits the window shell. */
function WindowReadyMarker({ reporter }: { reporter: WindowReadyReporter }) {
  return (
    <span
      ref={(node) => reportReady(node, reporter)}
      hidden
      aria-hidden
      data-window-ready-marker
    />
  )
}

/** Places the readiness marker after every callback ref in the window view. */
export function WindowReadyBoundary({
  children,
  reporter = windowReady,
}: {
  children: ReactNode
  reporter?: WindowReadyReporter
}) {
  return (
    <>
      {children}
      <WindowReadyMarker reporter={reporter} />
    </>
  )
}
