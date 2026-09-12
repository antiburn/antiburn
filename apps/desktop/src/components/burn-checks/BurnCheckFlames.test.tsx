import { render, screen } from "@testing-library/react"
import { describe, expect, it } from "vitest"

import { BurnCheckFlames } from "./BurnCheckFlames"

describe("BurnCheckFlames", () => {
  it.each([
    [0, [0, 0, 0, 0]],
    [300, [2.76, 0, 0, 0]],
    [2_500, [23, 0, 0, 0]],
    [3_750, [23, 11.5, 0, 0]],
    [10_000, [23, 23, 23, 23]],
  ])("fills four equal portions for %i basis points", (basisPoints, heights) => {
    const { container } = render(<BurnCheckFlames basisPoints={basisPoints} />)
    const fills = container.querySelectorAll("g > rect[fill^='url']")
    expect(fills).toHaveLength(4)
    fills.forEach((fill, index) => {
      expect(Number(fill.getAttribute("height"))).toBeCloseTo(heights[index]!)
    })
    expect(screen.getByRole("meter", { name: "Estimated token burn" })).toHaveAttribute(
      "aria-valuenow",
      String(basisPoints / 100),
    )
  })

  it("keeps mask and gradient references unique across summaries", () => {
    const { container } = render(
      <>
        <BurnCheckFlames basisPoints={300} />
        <BurnCheckFlames basisPoints={1_600} />
      </>,
    )
    const ids = Array.from(container.querySelectorAll("[id]"), (node) => node.id)
    expect(new Set(ids).size).toBe(ids.length)
    for (const svg of container.querySelectorAll("[role='meter'] > svg")) {
      expect(svg.querySelector("g")).toHaveAttribute(
        "mask",
        `url(#${svg.querySelector("mask")!.id})`,
      )
    }
  })
})
