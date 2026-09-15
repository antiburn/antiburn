import { render } from "@testing-library/react"
import { describe, expect, it } from "vitest"

import { SegmentedRadialDial } from "./SegmentedRadialDial"

function circles(container: HTMLElement): SVGCircleElement[] {
  return [...container.querySelectorAll<SVGCircleElement>("circle")]
}

function visibleGap(circle: SVGCircleElement, nextCircle: SVGCircleElement): number {
  const radius = Number(circle.getAttribute("r"))
  const strokeWidth = Number(circle.getAttribute("stroke-width"))
  const endAngle = Number(circle.dataset.startAngle) + Number(circle.dataset.arcAngle)
  const centerlineGap = Number(nextCircle.dataset.startAngle) - endAngle
  const endpointDistance = 2 * radius * Math.sin((centerlineGap * Math.PI) / 360)
  return endpointDistance - strokeWidth
}

describe("SegmentedRadialDial", () => {
  it("renders no arcs for an empty set", () => {
    const { container } = render(<SegmentedRadialDial segments={[]} size={16} />)
    expect(circles(container)).toEqual([])
  })

  it("omits zero, negative, and nonfinite values without changing order", () => {
    const { container } = render(
      <SegmentedRadialDial
        size={24}
        segments={[
          { id: "zero", value: 0, className: "zero" },
          { id: "passed", value: 3, className: "passed" },
          { id: "negative", value: -1, className: "negative" },
          { id: "failed", value: 1, className: "failed" },
          { id: "infinite", value: Number.POSITIVE_INFINITY, className: "infinite" },
        ]}
      />,
    )

    expect(circles(container).map((circle) => circle.dataset.segmentId)).toEqual([
      "passed",
      "failed",
    ])
  })

  it("renders one positive segment as a complete circle", () => {
    const { container } = render(
      <SegmentedRadialDial
        size={32}
        gapAngle={200}
        segments={[{ id: "only", value: 2, className: "only" }]}
      />,
    )
    const circle = circles(container)[0]!
    expect(circle.dataset.arcAngle).toBe("360")
    expect(circle).not.toHaveAttribute("stroke-dasharray")
  })

  it("normalizes highly uneven values without overflowing", () => {
    const { container } = render(
      <SegmentedRadialDial
        size={48}
        gapAngle={4}
        segments={[
          { id: "large", value: Number.MAX_VALUE, className: "large" },
          { id: "small", value: 1, className: "small" },
        ]}
      />,
    )
    const angles = circles(container).map((circle) => Number(circle.dataset.arcAngle))
    expect(angles.every(Number.isFinite)).toBe(true)
    expect(angles[1]).toBeGreaterThan(0)
    expect(angles[0]! / (angles[0]! + angles[1]!)).toBeGreaterThan(0.999_999)
  })

  it("bounds excessive gaps and keeps every arc finite", () => {
    const { container } = render(
      <SegmentedRadialDial
        size={24}
        gapAngle={1_000}
        segments={[
          { id: "one", value: 1, className: "one" },
          { id: "two", value: 1, className: "two" },
          { id: "three", value: 1, className: "three" },
        ]}
      />,
    )
    const angles = circles(container).map((circle) => Number(circle.dataset.arcAngle))
    expect(angles).toHaveLength(3)
    expect(angles.every((angle) => Number.isFinite(angle) && angle > 0)).toBe(true)
    expect(angles.reduce((total, angle) => total + angle, 0)).toBeCloseTo(0.001, 5)
  })

  it.each([16, 24])("keeps rounded endpoints visually separate at %spx", (size) => {
    const { container } = render(
      <SegmentedRadialDial
        size={size}
        gapAngle={5}
        segments={[
          { id: "failed", value: 1, className: "failed" },
          { id: "passed", value: 5, className: "passed" },
        ]}
      />,
    )
    const [failed, passed] = circles(container)
    expect(visibleGap(failed!, passed!)).toBeGreaterThan(0)
  })

  it("supports labelled and decorative rendering", () => {
    const segment = [{ id: "one", value: 1, className: "one" }]
    const labelled = render(
      <SegmentedRadialDial segments={segment} size={16} label="Three of six checks passed" />,
    )
    expect(labelled.getByRole("img", { name: "Three of six checks passed" })).toBeTruthy()
    labelled.unmount()

    const decorative = render(<SegmentedRadialDial segments={segment} size={16} />)
    expect(decorative.container.querySelector("svg")).toHaveAttribute("aria-hidden", "true")
    expect(decorative.queryByRole("img")).toBeNull()
  })
})
