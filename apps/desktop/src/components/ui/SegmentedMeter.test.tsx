import { render } from "@testing-library/react"
import { describe, expect, it } from "vitest"

import { SegmentedMeter } from "./SegmentedMeter"

function segments(container: HTMLElement): HTMLElement[] {
  return Array.from(container.querySelectorAll<HTMLElement>("span.rounded-full"))
}

// The full-strength zone fills; an unlit segment carries a /25 tint instead.
const FILL_CLASSES = ["bg-brand-tint", "bg-system-red-tint"]

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
    // At 95% the fill crosses into the red zone: the orange zone (0–90%) is
    // fully lit and the red zone has one lit segment before its tinted tail.
    // No segment is yellow, because the scale holds no warning step.
    expect(all.filter((node) => node.classList.contains("bg-brand-tint"))).toHaveLength(29)
    expect(all.filter((node) => node.classList.contains("bg-system-yellow-tint"))).toHaveLength(
      0,
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

  it("blinks the next segment to light while a session is live", () => {
    // 50% lights 16, so the seventeenth (index 16) is the one that can
    // alternate between brand and its zone's track tint.
    const { container } = render(<SegmentedMeter percent={50} blinkNext />)
    const all = segments(container)
    expect(container.querySelectorAll(".led-blink")).toHaveLength(1)
    expect(all[16]).toHaveClass("led-blink", "bg-brand-unlit/12")
    expect(all[16]?.style.getPropertyValue("--led-rest")).toBe(
      "color-mix(in srgb, var(--color-brand-unlit) 12%, transparent)",
    )
  })

  it("marks the blinking segment with its stagger step, capped at the last keyframe set", () => {
    const { container: first } = render(<SegmentedMeter percent={50} blinkNext />)
    expect(segments(first)[16]?.dataset["ledStep"]).toBeUndefined()
    const { container: third } = render(<SegmentedMeter percent={50} blinkNext blinkStep={2} />)
    expect(segments(third)[16]?.dataset["ledStep"]).toBe("2")
    const { container: ninth } = render(<SegmentedMeter percent={50} blinkNext blinkStep={8} />)
    expect(segments(ninth)[16]?.dataset["ledStep"]).toBe("5")
    const { container: still } = render(<SegmentedMeter percent={50} blinkStep={2} />)
    expect(still.querySelector("[data-led-step]")).toBeNull()
  })

  it("blinks the first segment at zero and the last at full", () => {
    const { container: zero } = render(<SegmentedMeter percent={0} blinkNext />)
    expect(segments(zero)[0]).toHaveClass("led-blink")
    const { container: full } = render(<SegmentedMeter percent={100} blinkNext />)
    expect(segments(full)[31]).toHaveClass("led-blink")
    expect(segments(full)[31]?.style.getPropertyValue("--led-rest")).toBe(
      "color-mix(in srgb, var(--color-system-red-unlit) 12%, transparent)",
    )
  })

  it("blinks the segment past the mark on its own side when it fills from the right", () => {
    // 95% from the right lights the last two; the next to light is index 29.
    const { container } = render(<SegmentedMeter percent={95} fillFrom="end" blinkNext />)
    expect(segments(container)[29]).toHaveClass("led-blink")
  })

  it("does not blink without a live session", () => {
    const { container } = render(<SegmentedMeter percent={50} />)
    expect(container.querySelector(".led-blink")).toBeNull()
  })

  it("hides itself from the accessibility tree", () => {
    // The figure beside the meter carries the reading; the circles would
    // announce as noise.
    const { container } = render(<SegmentedMeter percent={42} />)
    expect(container.firstElementChild).toHaveAttribute("aria-hidden", "true")
  })
})
