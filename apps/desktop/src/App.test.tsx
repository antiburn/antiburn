// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { render, screen } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import { App } from "./App"

// The shell is not attached under jsdom, so the IPC surface is stubbed at the
// module boundary: `isTauri()` reports presence and `invoke` answers commands.
const invoke = vi.hoisted(() => vi.fn())
vi.mock("@tauri-apps/api/core", () => ({ invoke, isTauri: () => true }))
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }))
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    outerPosition: async () => ({ x: 0, y: 0 }),
    setPosition: async () => {},
    hide: async () => {},
  }),
  currentMonitor: async () => null,
}))
vi.mock("@tauri-apps/api/dpi", () => ({
  LogicalPosition: class {
    x: number
    y: number
    constructor(x: number, y: number) {
      this.x = x
      this.y = y
    }
  },
}))

const settings = {
  theme: "system",
  activityWindowDays: 7,
  onboardingCompleted: false,
  launchAtLogin: false,
  autoUpdate: true,
}

describe("App", () => {
  beforeEach(() => {
    invoke.mockReset()
    invoke.mockImplementation((command: string) => {
      switch (command) {
        case "get_settings":
          return Promise.resolve(settings)
        case "app_info":
          return Promise.resolve({
            appVersion: "0.1.0",
            debugBuild: false,
            pricingCatalogVersion: "2026-01-01",
            schemaVersion: 1,
            dataDir: "/home/avery/.antiburn",
            updatesSupported: false,
          })
        case "default_scan_roots":
          return Promise.resolve(["/home/avery/code"])
        case "list_scan_roots":
        case "list_recent_sessions":
        case "list_repositories":
          return Promise.resolve([])
        default:
          return Promise.resolve(null)
      }
    })
    window.location.hash = ""
    delete document.documentElement.dataset["theme"]
  })

  it("renders the popover for an unknown fragment, not the first-run flow", async () => {
    // The default route is the popover on purpose, so a fragment nothing
    // recognizes lands somewhere real. Since onboarding moved to its own
    // window (D-25) that fallback must not draw the flow, whatever the
    // onboarding flag says.
    window.location.hash = "#/not-a-window"

    render(<App />)

    expect(screen.queryByTestId("route-loading")).not.toBeInTheDocument()
    await screen.findByRole("button", { name: "antiburn v0.1.0" })
    expect(
      screen.queryByRole("heading", { name: "Stop hitting your token limits." }),
    ).not.toBeInTheDocument()
  })

  it("renders the floating HUD for the overlay fragment", async () => {
    window.location.hash = "#/overlay"
    render(<App />)
    expect(await screen.findByRole("button", { name: "Close overlay" })).toBeInTheDocument()
    expect(document.body.dataset.transparentWindow).toBe("true")
  })
})
