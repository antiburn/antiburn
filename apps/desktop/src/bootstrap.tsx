import { StrictMode, type ReactNode } from "react"
import { createRoot } from "react-dom/client"

import { WindowReadyBoundary } from "./components/WindowReadyMarker"
import { installFocusModality } from "./lib/focusModality"
import { applyPlatformAttribute } from "./lib/platform"
import { applyRouteAttribute, type Route } from "./lib/route"

export function mountWindow(view: ReactNode, route?: Route): void {
  const container = document.getElementById("root")
  if (!container) {
    throw new Error("The window entry is missing the #root mount point")
  }

  // Set the attributes before React starts the first render.
  applyPlatformAttribute()
  applyRouteAttribute(document.documentElement, route)
  installFocusModality()

  createRoot(container).render(
    <StrictMode>
      <WindowReadyBoundary>{view}</WindowReadyBoundary>
    </StrictMode>,
  )
}
