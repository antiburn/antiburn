// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { lazy, Suspense } from "react"

import { useRoute } from "./lib/route"
import { NudgeView } from "./views/NudgeView"
import { PopoverView } from "./views/PopoverView"

const OverlayWindow = lazy(() =>
  import("./views/OverlayWindow").then(({ OverlayWindow: view }) => ({ default: view })),
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
  return <PopoverView />
}
