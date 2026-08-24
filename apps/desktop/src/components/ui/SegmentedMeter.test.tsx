// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { render } from "@testing-library/react"
import { describe, expect, it } from "vitest"

import { SegmentedMeter } from "./SegmentedMeter"

function segments(container: HTMLElement): HTMLElement[] {
  return Array.from(container.querySelectorAll<HTMLElement>("span.rounded-full"))
}

function filled(container: HTMLElement): number {
  return segments(container).filter((node) => node.className.includes("bg-brand-tint")).length
}

describe("SegmentedMeter", () => {
  it("fills segments in proportion to the percentage", () => {
    const { container } = render(<SegmentedMeter percent={50} />)
    expect(segments(container)).toHaveLength(32)
    expect(filled(container)).toBe(16)
  })

  it("clamps a percentage outside 0–100 rather than overflowing the track", () => {
    // A provider that reports over its own allowance still gets a full meter,
    // not a row longer than the row.
    const { container: over } = render(<SegmentedMeter percent={140} />)
    expect(filled(over)).toBe(32)
    const { container: under } = render(<SegmentedMeter percent={-10} />)
    expect(filled(under)).toBe(0)
  })

  it("renders a null percent as an empty meter at half strength", () => {
    // The distinction that matters: no reading, not a reading of zero. A
    // meter at zero states a figure nobody supplied.
    const { container } = render(<SegmentedMeter percent={null} />)
    expect(filled(container)).toBe(0)
    expect(segments(container).every((node) => node.className.includes("opacity-50"))).toBe(
      true,
    )
  })

  it("keeps a stated zero at full strength, because it is a real reading", () => {
    const { container } = render(<SegmentedMeter percent={0} />)
    expect(filled(container)).toBe(0)
    expect(segments(container).some((node) => node.className.includes("opacity-50"))).toBe(
      false,
    )
  })

  it("puts the notch at the elapsed fraction of the row", () => {
    const { getByTestId } = render(<SegmentedMeter percent={42} expectedFraction={0.25} />)
    expect(getByTestId("segmented-meter-notch")).toHaveStyle({ left: "25%" })
  })

  it("draws no notch without an expected fraction", () => {
    // A notch is never drawn from an assumed period.
    const { queryByTestId } = render(<SegmentedMeter percent={42} />)
    expect(queryByTestId("segmented-meter-notch")).not.toBeInTheDocument()
  })

  it("hides itself from the accessibility tree", () => {
    // The figure beside the meter carries the reading; the circles would
    // announce as noise.
    const { container } = render(<SegmentedMeter percent={42} />)
    expect(container.firstElementChild).toHaveAttribute("aria-hidden", "true")
  })
})
