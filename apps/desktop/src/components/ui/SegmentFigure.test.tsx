import { render, screen } from "@testing-library/react"
import { describe, expect, it } from "vitest"

import { SegmentFigure } from "./SegmentFigure"

describe("SegmentFigure", () => {
  it("gives every digit the same width, so a figure does not shift as it ticks", () => {
    render(<SegmentFigure>42%</SegmentFigure>)
    expect(screen.getByText("42%")).toHaveClass("tabular-nums")
  })

  it("sets no font family, so the figure takes the type style beside it", () => {
    // A second typeface for a two-character percentage read as a different
    // kind of thing from its own label, and the review removed it.
    render(<SegmentFigure>42%</SegmentFigure>)
    expect(screen.getByText("42%").className).not.toMatch(/font-mono/)
  })

  it("keeps the caller's own classes", () => {
    render(<SegmentFigure className="text-label">42%</SegmentFigure>)
    const figure = screen.getByText("42%")
    expect(figure).toHaveClass("text-label")
    expect(figure).toHaveClass("tabular-nums")
  })
})
