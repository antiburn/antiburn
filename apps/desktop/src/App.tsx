// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { lazy, Suspense } from "react"

import { useRoute } from "./lib/route"
import { NudgeView } from "./views/NudgeView"
import { PopoverView } from "./views/PopoverView"

// Keep the nudge and popover views in the entry chunk. Both must respond when
// their shell window appears. The larger standalone windows load by route.
const OnboardingView = lazy(() =>
  import("./views/OnboardingView").then(({ OnboardingView: view }) => ({ default: view })),
)
const SettingsView = lazy(() =>
  import("./views/SettingsView").then(({ SettingsView: view }) => ({ default: view })),
)
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
  if (route !== "settings" && route !== "onboarding") return <PopoverView />

  const view = route === "settings" ? <SettingsView /> : <OnboardingView />

  return <Suspense fallback={<RouteLoading />}>{view}</Suspense>
}
