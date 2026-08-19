// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { cleanup, render, screen } from "@testing-library/react"
import { AlertTriangle } from "lucide-react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { RowTimeCorner, TruncatedText } from "./ActivityRowPrimitives"
import { ROW_CORNER_ATTR, ROW_CORNER_PINNED_ATTR, rowTimeLabel } from "./activityRow"

const NOW = new Date("2026-03-04T12:00:00.000Z")
const FIVE_MINUTES_AGO = new Date(NOW.getTime() - 5 * 60_000).toISOString()

beforeEach(() => {
  vi.useFakeTimers()
  vi.setSystemTime(NOW)
})

afterEach(() => {
  vi.useRealTimers()
  cleanup()
})

describe("TruncatedText", () => {
  it("renders its text without a redundant tooltip when it fits", () => {
    render(<TruncatedText text="Short title" />)
    const el = screen.getByText("Short title")
    expect(el.getAttribute("title")).toBeNull()
    expect(el.getAttribute("data-text")).toBeNull()
  })

  it("duplicates the text for the shimmer overlay and keeps it announceable", () => {
    render(<TruncatedText text="Running session" shimmer />)
    const el = screen.getByText("Running session")
    expect(el.getAttribute("data-text")).toBe("Running session")
    expect(el.getAttribute("aria-label")).toBe("Running session")
    expect(el.className).toContain("activity-row-title-shimmer")
  })

  it("reveals the full value once the text is actually cut off", () => {
    // jsdom reports zero dimensions, so drive the measurement directly.
    vi.spyOn(HTMLElement.prototype, "scrollWidth", "get").mockReturnValue(400)
    vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(120)
    render(<TruncatedText text="A very long session title that will not fit" />)
    expect(
      screen.getByText("A very long session title that will not fit").getAttribute("title"),
    ).toBe("A very long session title that will not fit")
    vi.restoreAllMocks()
  })
})

describe("RowTimeCorner", () => {
  it("announces the status and time even while the painted corner is hidden", () => {
    render(<RowTimeCorner timestamp={FIVE_MINUTES_AGO} label="Last activity" />)
    expect(screen.getByText("Last activity 5m ago")).toBeTruthy()
    // The painted copy is compact and decorative.
    expect(screen.getByText("5m")).toBeTruthy()
  })

  it("suppresses the announced copy for a row that names itself", () => {
    render(
      <RowTimeCorner timestamp={FIVE_MINUTES_AGO} label="Last activity" announce={false} />,
    )
    expect(screen.queryByText("Last activity 5m ago")).toBeNull()
    expect(screen.getByText("5m")).toBeTruthy()
  })

  it("marks a pinned corner and drops the reveal classes it would otherwise carry", () => {
    const { container } = render(
      <RowTimeCorner
        timestamp={FIVE_MINUTES_AGO}
        label="Needs attention"
        icon={AlertTriangle}
        tint="text-system-orange"
        pinned
        revealClassName="activity-row-secondary-detail"
      />,
    )
    const corner = container.querySelector(`[${ROW_CORNER_ATTR}]`)
    expect(corner?.hasAttribute(ROW_CORNER_PINNED_ATTR)).toBe(true)
    expect(corner?.className).not.toContain("activity-row-secondary-detail")
    expect(container.querySelector(".text-system-orange")).toBeTruthy()
  })

  it("hides an unpinned corner behind the reveal classes", () => {
    const { container } = render(
      <RowTimeCorner
        timestamp={FIVE_MINUTES_AGO}
        label="Last activity"
        revealClassName="activity-row-secondary-detail"
      />,
    )
    const corner = container.querySelector(`[${ROW_CORNER_ATTR}]`)
    expect(corner?.hasAttribute(ROW_CORNER_PINNED_ATTR)).toBe(false)
    expect(corner?.className).toContain("activity-row-secondary-detail")
  })
})

describe("rowTimeLabel", () => {
  it("joins the status and the relative time into one spoken phrase", () => {
    expect(rowTimeLabel("Last activity", FIVE_MINUTES_AGO)).toBe("Last activity 5m ago")
  })
})
