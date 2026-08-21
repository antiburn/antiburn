// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { describe, expect, it } from "vitest"

import {
  applyRouteAttribute,
  NUDGE_FRAGMENT,
  OVERLAY_FRAGMENT,
  routeFromHash,
  SETTINGS_FRAGMENT,
} from "./route"

/**
 * The fragment is the only thing that distinguishes one window from another,
 * so a shell window opened with a fragment this table doesn't know would render
 * the popover inside it. That makes the exported fragment constants and the
 * parser worth checking together.
 */
describe("routeFromHash", () => {
  it("resolves the fragments the shell opens windows with", () => {
    expect(routeFromHash(SETTINGS_FRAGMENT)).toBe("settings")
    expect(routeFromHash(NUDGE_FRAGMENT)).toBe("nudge")
    expect(routeFromHash(OVERLAY_FRAGMENT)).toBe("overlay")
  })

  it("accepts a fragment with or without the leading slash", () => {
    expect(routeFromHash("#settings")).toBe("settings")
    expect(routeFromHash("#nudge")).toBe("nudge")
    expect(routeFromHash("#overlay")).toBe("overlay")
  })

  it("falls back to the popover for an empty fragment", () => {
    expect(routeFromHash("")).toBe("popover")
    expect(routeFromHash("#")).toBe("popover")
    expect(routeFromHash("#/")).toBe("popover")
  })

  it("falls back to the popover rather than rendering nothing for an unknown one", () => {
    expect(routeFromHash("#/account")).toBe("popover")
  })

  it("does not resolve a route from an inherited object property", () => {
    // A fragment like "constructor" must not resolve to anything just because
    // it is an inherited object property rather than a known route.
    expect(routeFromHash("#/constructor")).toBe("popover")
    expect(routeFromHash("#/toString")).toBe("popover")
  })
})

describe("applyRouteAttribute", () => {
  it("publishes the route so CSS can branch on the window", () => {
    const root = document.createElement("html")

    applyRouteAttribute(root, "popover")

    expect(root.getAttribute("data-route")).toBe("popover")
  })

  it("overwrites a stale value instead of appending", () => {
    const root = document.createElement("html")

    applyRouteAttribute(root, "popover")
    applyRouteAttribute(root, "settings")

    expect(root.getAttribute("data-route")).toBe("settings")
  })
})
