// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { act, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { DEFAULT_POPOVER_HEIGHT, POPOVER_HEIGHTS } from "../../lib/popoverHeight"
import { HotspotBlock } from "./HotspotBlock"
import {
  HOTSPOT_CATEGORIES,
  HOTSPOT_COPY,
  hotspotCountLabel,
  type HotspotFinding,
} from "./hotspot"

const FINDING: HotspotFinding = {
  category: "unusedMcpServers",
  sessions: 34,
  saving: "≈ 210k tokens",
  fix: "claude mcp remove playwright",
  evidence: [
    { label: "Tokens over the window", value: "210,400" },
    { label: "Servers loaded, never called", value: "3" },
  ],
}

describe("HotspotBlock", () => {
  it("renders nothing when the report has no finding", () => {
    const { container } = render(<HotspotBlock finding={null} />)

    // FR-14: a cohort that is only half read must never render as clean. An
    // empty shell or an "all clear" line could be misread; nothing cannot.
    expect(container.firstChild).toBeNull()
  })

  it("names the count, the category and the saving on the claim line", () => {
    render(<HotspotBlock finding={FINDING} />)

    // The default normalizer collapses the hair space in `34 ×`, which is the
    // one character this label exists to add.
    expect(screen.getByText(hotspotCountLabel(34), { normalizer: (text) => text })).toBeTruthy()
    expect(screen.getByText(HOTSPOT_COPY.unusedMcpServers.name)).toBeTruthy()
    expect(screen.getByText("≈ 210k tokens")).toBeTruthy()
  })

  it("shows the fix line whether the detail is open or closed", () => {
    render(<HotspotBlock finding={FINDING} />)
    expect(screen.getByText("claude mcp remove playwright")).toBeTruthy()

    fireEvent.click(screen.getByRole("button", { name: /Unused MCP servers/ }))
    expect(screen.getByText("claude mcp remove playwright")).toBeTruthy()
  })

  it("keeps the evidence out of the tree until the claim line is opened", () => {
    render(<HotspotBlock finding={FINDING} />)
    const claim = screen.getByRole("button", { name: /Unused MCP servers/ })

    expect(claim.getAttribute("aria-expanded")).toBe("false")
    expect(screen.queryByText("Tokens over the window")).toBeNull()

    fireEvent.click(claim)
    expect(claim.getAttribute("aria-expanded")).toBe("true")
    expect(screen.getByText(HOTSPOT_COPY.unusedMcpServers.mechanism)).toBeTruthy()
    expect(screen.getByText("Tokens over the window")).toBeTruthy()
    expect(screen.getByText("Servers loaded, never called")).toBeTruthy()

    fireEvent.click(claim)
    expect(claim.getAttribute("aria-expanded")).toBe("false")
    expect(screen.queryByText("Tokens over the window")).toBeNull()
  })

  it("points aria-controls at the evidence it reveals", () => {
    render(<HotspotBlock finding={FINDING} />)
    const claim = screen.getByRole("button", { name: /Unused MCP servers/ })

    fireEvent.click(claim)
    const controls = claim.getAttribute("aria-controls")
    expect(controls).toBeTruthy()
    expect(document.getElementById(controls as string)?.textContent).toContain(
      "Servers loaded, never called",
    )
  })

  it("leaves the saving out when pricing cannot value the category", () => {
    render(<HotspotBlock finding={{ ...FINDING, saving: null }} />)

    expect(screen.queryByText(/≈/)).toBeNull()
  })

  describe("copy", () => {
    let written: string[]

    beforeEach(() => {
      vi.useFakeTimers()
      written = []
      Object.defineProperty(navigator, "clipboard", {
        configurable: true,
        value: {
          writeText: (text: string) => {
            written.push(text)
            return Promise.resolve()
          },
        },
      })
    })

    afterEach(() => {
      vi.useRealTimers()
    })

    it("writes the fix verbatim and reverts the check without an effect", async () => {
      render(<HotspotBlock finding={FINDING} />)

      fireEvent.click(screen.getByRole("button", { name: "Copy claude mcp remove playwright" }))
      await act(async () => {})
      expect(written).toEqual(["claude mcp remove playwright"])
      expect(
        screen.getByRole("button", { name: "Copied claude mcp remove playwright" }),
      ).toBeTruthy()

      // The revert is a timer the click starts, not a subscription a render
      // sets up. It has to fire on its own.
      await act(async () => {
        vi.advanceTimersByTime(2000)
      })
      expect(
        screen.getByRole("button", { name: "Copy claude mcp remove playwright" }),
      ).toBeTruthy()
    })

    it("stays quiet when the webview refuses the write", async () => {
      // A webview with no user activation or no focus rejects `writeText`.
      // Seen for real in `pnpm dev:web` as `NotAllowedError`.
      //
      // The assertion below only covers the visible half: no check, because
      // nothing was copied. The other half — that the rejection is handled —
      // is enforced by vitest, which reports an unhandled rejection raised
      // during a test and exits non-zero. Drop the `.catch` in `onCopy` and
      // this file fails on that route, not on an `expect`.
      Object.defineProperty(navigator, "clipboard", {
        configurable: true,
        value: { writeText: () => Promise.reject(new Error("Write permission denied.")) },
      })

      render(<HotspotBlock finding={FINDING} />)
      fireEvent.click(screen.getByRole("button", { name: "Copy claude mcp remove playwright" }))
      await act(async () => {})

      expect(
        screen.getByRole("button", { name: "Copy claude mcp remove playwright" }),
      ).toBeTruthy()
    })
  })

  it("makes the whole fix field the copy target, not just the icon", () => {
    render(<HotspotBlock finding={FINDING} />)

    // The command text has to sit inside the button. A 13px icon beside a wide
    // command is the smaller of the two things a reader aims at.
    const field = screen.getByRole("button", { name: "Copy claude mcp remove playwright" })
    expect(field.textContent).toBe("claude mcp remove playwright")
  })

  it("shows every evidence row the finding carries", () => {
    const rows = [
      { label: "Tokens over the window", value: "210,400" },
      { label: "Servers loaded, never called", value: "3" },
      { label: "Tool definitions loaded", value: "41" },
      { label: "Sessions in the cohort", value: "61" },
      { label: "Longest run without a call", value: "12 days" },
    ]
    render(<HotspotBlock finding={{ ...FINDING, evidence: rows }} />)
    fireEvent.click(screen.getByRole("button", { name: /Unused MCP servers/ }))

    // No cap on the count. Past the detail's max height the rows scroll, so a
    // long finding never pushes the session list out of the window.
    for (const row of rows) expect(screen.getByText(row.label)).toBeTruthy()
  })

  it("has copy for every category the report can send", () => {
    for (const category of HOTSPOT_CATEGORIES) {
      expect(HOTSPOT_COPY[category].name.length).toBeGreaterThan(0)
      expect(HOTSPOT_COPY[category].mechanism.length).toBeGreaterThan(0)
    }
  })

  it("never asks the shell for a taller activity window", () => {
    // The block earns its height from the session list, which already scrolls.
    // A later change that grows the window instead would move the tray popover
    // under the cursor, so pin the contract here.
    expect(POPOVER_HEIGHTS.activity).toBe(DEFAULT_POPOVER_HEIGHT)
  })
})
