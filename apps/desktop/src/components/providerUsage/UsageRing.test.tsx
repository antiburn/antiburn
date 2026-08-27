import { render } from "@testing-library/react"
import { describe, expect, it } from "vitest"

import { UsageRing } from "./UsageRing"

/** The arc's total length, which the dash offset is measured against. */
const CIRCUMFERENCE = 2 * Math.PI * 13

function arcOffset(container: HTMLElement): number {
  const arc = container.querySelector('[data-testid="usage-ring-arc"]')
  return Number(arc?.getAttribute("stroke-dashoffset"))
}

const SQUARE = {
  path: "M0 0h24v24H0z",
  viewBox: "0 0 24 24",
  provenance: {
    package: "test",
    icon: "square",
    license: "CC0-1.0",
    source: "https://example.test",
  },
}

describe("UsageRing", () => {
  it("fills the arc in proportion to what is consumed", () => {
    const { container } = render(<UsageRing percent={75} />)
    // Three quarters gone leaves a quarter of the circumference hidden.
    expect(arcOffset(container)).toBeCloseTo(CIRCUMFERENCE * 0.25, 5)
  })

  it("draws an empty ring rather than an arc at zero when nothing was stated", () => {
    // A ring at 0% is a claim that nothing has been used. A dashed track with
    // no arc is visibly a ring with no reading in it, which is the truth.
    const { container } = render(<UsageRing percent={null} />)
    expect(container.querySelector('[data-testid="usage-ring-arc"]')).toBeNull()
    expect(container.querySelector("circle")).toHaveAttribute("stroke-dasharray", "2 2")
  })

  it("renders a stated zero as a real, empty arc", () => {
    const { container } = render(<UsageRing percent={0} />)
    expect(arcOffset(container)).toBeCloseTo(CIRCUMFERENCE, 5)
    // Solid track, because this reading exists.
    expect(container.querySelector("circle")).not.toHaveAttribute("stroke-dasharray", "2 2")
  })

  it("draws the remainder behind a stated arc, so a small share reads as one", () => {
    // The usage bar states no figure beside the ring any more. An arc with
    // nothing behind it states a length; an arc on a track states a share.
    const { container } = render(<UsageRing percent={14} />)
    expect(container.querySelector('[data-testid="usage-ring-track"]')).toBeInTheDocument()
  })

  it("draws no second track under the indeterminate ring", () => {
    const { container } = render(<UsageRing percent={null} />)
    expect(container.querySelector('[data-testid="usage-ring-track"]')).toBeNull()
  })

  it("clamps rather than overdrawing a figure past its own limit", () => {
    const { container } = render(<UsageRing percent={140} />)
    expect(arcOffset(container)).toBeCloseTo(0, 5)
  })

  it("keeps the provider’s identity inside the ring", () => {
    // A provider pill must identify the provider that owns the limit.
    const { container } = render(<UsageRing percent={40} glyph="A" />)
    expect(container.querySelector('[data-testid="usage-ring-glyph"]')).toHaveTextContent("A")
    expect(
      render(<UsageRing percent={40} />).container.querySelector(
        '[data-testid="usage-ring-glyph"]',
      ),
    ).toBeNull()
  })

  it("prefers a brand mark over the letter when one exists", () => {
    // The letter is the fallback for providers with no rights-cleared mark,
    // not a second thing to draw alongside one.
    const { container } = render(<UsageRing percent={40} glyph="A" mark={SQUARE} />)
    expect(container.querySelector('[data-testid="usage-ring-mark"]')).not.toBeNull()
    expect(container.querySelector('[data-testid="usage-ring-glyph"]')).toBeNull()
  })

  /** The drawn width of a mark, in the ring's 32-unit units. */
  function drawnExtent(mark: { path: string; viewBox: string }, edge: number): number {
    const { container } = render(
      <UsageRing percent={40} mark={{ ...mark, provenance: SQUARE.provenance }} />,
    )
    const t = container
      .querySelector('[data-testid="usage-ring-mark"]')
      ?.getAttribute("transform")
    return Number(/scale\(([\d.]+)\)/.exec(t ?? "")?.[1]) * edge
  }

  it("insets every mark from the arc, whatever box its source drew it in", () => {
    // The track and arc take the outer 2.5 units of a radius-13 circle, so a
    // mark that fills the interior edge-to-edge crowds it. Marks do not share
    // one box — simple-icons draws at 24, other sources do not — so a scale
    // derived from the ring instead of the mark silently resizes half the set.
    expect(drawnExtent({ path: "M0 0h24v24H0z", viewBox: "0 0 24 24" }, 24)).toBeCloseTo(
      16.8,
      5,
    )
    expect(drawnExtent({ path: "M0 0h256v260H0z", viewBox: "0 0 256 260" }, 260)).toBeCloseTo(
      16.8,
      5,
    )
  })

  it("is invisible to a screen reader, because its caller names it", () => {
    // The ring is a shape with no text. Every call site puts the figure into
    // the accessible name of the control around it instead.
    const { container } = render(<UsageRing percent={40} />)
    expect(container.querySelector("svg")).toHaveAttribute("aria-hidden", "true")
  })
})
