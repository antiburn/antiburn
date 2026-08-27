// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { render, screen } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import { InsightsPane } from "./InsightsPane"

/**
 * CH-012 requires portability to be proven, not asserted: a test mounts
 * the pane from a second entry point with no change to the pane itself.
 *
 * The harness below is that second entry point. It is not `SettingsView`,
 * it mounts no `SettingsWindowSession`, it reads no settings-window state,
 * and it passes the pane nothing. If the pane ever grows a dependency on
 * being inside the settings window, this file fails to express it and the
 * assertions below break.
 *
 * Shipping a real second window is explicitly out of scope for CH-012;
 * this test is the proof artifact that one could mount the pane unchanged.
 */

const invoke = vi.hoisted(() => vi.fn())

vi.mock("@tauri-apps/api/core", () => ({ invoke, isTauri: () => true }))

/** A second entry point: a bare host that is not the settings window. */
function StandaloneInsightsHost() {
  return (
    <main aria-label="Standalone insights host">
      <InsightsPane />
    </main>
  )
}

beforeEach(() => {
  vi.clearAllMocks()
  invoke.mockImplementation((command: string) => {
    switch (command) {
      case "get_insights_report":
        return Promise.resolve({
          environmentKey: "native",
          windowStartEpoch: 100,
          windowEndEpoch: 200,
          computedAtEpoch: 200,
          coverage: {
            discovered: 2,
            unknownStart: 0,
            pending: 1,
            processing: 0,
            failed: 0,
            unsupported: 0,
            stale: 0,
            ready: 1,
            activelyGrowing: 0,
            awaitingProviderSupport: 0,
          },
          assessedSessions: 1,
          categories: [
            {
              id: "sessionsOverDepth",
              eligible: 1,
              assessed: 1,
              status: "clean",
              findingSessions: null,
              notAssessedReason: null,
            },
          ],
          quotaPressure: { assessed: false, findings: null },
          catalogRevision: 1,
        })
      case "get_insights_status":
        return Promise.resolve({ calculating: false, pending: 1, processing: 0 })
      default:
        return Promise.resolve(null)
    }
  })
})

describe("InsightsPane portability", () => {
  it("mounts and loads from a second entry point, unchanged", async () => {
    render(<StandaloneInsightsHost />)

    // The pane renders its full report inside the foreign host.
    expect(await screen.findByRole("heading", { name: "Insights" })).toBeInTheDocument()
    expect(await screen.findByText("2 discovered · 1 assessed")).toBeInTheDocument()
    expect(screen.getByText("Clean across 1 assessed session")).toBeInTheDocument()

    // It asked the shell only for its own data: no settings-window
    // handshake, no pane routing, no app-info fetch.
    const commands = invoke.mock.calls.map(([command]) => command)
    expect(commands).toContain("get_insights_report")
    expect(commands).not.toContain("app_info")
    expect(commands).not.toContain("take_settings_pane")
    expect(commands).not.toContain("get_settings")
  })
})
