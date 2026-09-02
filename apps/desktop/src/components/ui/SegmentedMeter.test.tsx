import { render } from "@testing-library/react"
import { describe, expect, it } from "vitest"

import { SegmentedMeter } from "./SegmentedMeter"

function segments(container: HTMLElement): HTMLElement[] {
  return Array.from(container.querySelectorAll<HTMLElement>("span.rounded-full"))
}

// The full-strength zone fills; an unlit segment carries a /25 tint instead.
const FILL_CLASSES = ["bg-brand-tint", "bg-system-yellow-tint", "bg-system-red-tint"]

function filled(container: HTMLElement): number {
  return segments(container).filter((node) =>
    FILL_CLASSES.some((cls) => node.classList.contains(cls)),
  ).length
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

  it("colors each segment by its zone, like a VU meter", () => {
    const { container } = render(<SegmentedMeter percent={95} />)
    const all = segments(container)
    // At 95% the fill crosses into the red zone: the yellow zone (80–90%)
    // is fully lit and the red zone has one lit segment before its tinted
    // tail.
    expect(all.filter((node) => node.classList.contains("bg-system-yellow-tint"))).toHaveLength(
      3,
    )
    expect(all.filter((node) => node.classList.contains("bg-system-red-tint"))).toHaveLength(1)
    expect(
      all.filter((node) => node.classList.contains("bg-system-red-unlit/12")),
    ).toHaveLength(2)
  })

  it("lights the track from the right down to the mark when it fills from the right", () => {
    const { container } = render(<SegmentedMeter percent={95} fillFrom="end" />)
    const all = segments(container)
    // The reading keeps its place on the track: 95% is two segments from the
    // right end, so only those two light, and they light in the red zone.
    expect(all.filter((node) => node.classList.contains("bg-system-red-tint"))).toHaveLength(2)
    expect(all[0]?.classList.contains("bg-brand-unlit/12")).toBe(true)
    expect(all[31]?.classList.contains("bg-system-red-tint")).toBe(true)
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
