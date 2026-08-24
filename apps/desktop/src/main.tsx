// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { StrictMode } from "react"
import { createRoot } from "react-dom/client"

import { App } from "./App"
import { installFocusModality } from "./lib/focusModality"
import { applyPlatformAttribute } from "./lib/platform"
import { applyRouteAttribute } from "./lib/route"
import "./styles.css"

const container = document.getElementById("root")
if (!container) {
  throw new Error("index.html is missing the #root mount point")
}

// All three write attributes the stylesheet keys off, so they run before the
// first paint: `data-platform` guards the platform-specific control rules,
// `data-route` gives floating windows their corners, and `data-keyboard` gates
// the focus ring.
applyPlatformAttribute()
applyRouteAttribute()
installFocusModality()

createRoot(container).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
