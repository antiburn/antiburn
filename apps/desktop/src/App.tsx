import { lazy, Suspense } from "react"

import { useRoute } from "./lib/route"
import { NudgeView } from "./views/NudgeView"
import { PopoverView } from "./views/PopoverView"

const OverlayWindow = lazy(() =>
  import("./views/OverlayWindow").then(({ OverlayWindow: view }) => ({ default: view })),
)
const HudDetailView = lazy(() =>
  import("./views/overlay/HudDetailView").then(({ HudDetailView: view }) => ({
    default: view,
  })),
)

function RouteLoading() {
  return <div className="h-full" aria-busy="true" data-testid="route-loading" />
}

export function App() {
  const route = useRoute()
  if (route === "nudge") return <NudgeView />
  if (route === "overlay") {
    return (
      <Suspense fallback={<RouteLoading />}>
        <OverlayWindow />
      </Suspense>
    )
  }
  if (route === "hud-detail") {
    return (
      <Suspense fallback={<RouteLoading />}>
        <HudDetailView />
      </Suspense>
    )
  }
  return <PopoverView />
}
