import { render, screen } from "@testing-library/react"
import { describe, expect, it } from "vitest"

import { SectionGroup } from "./SectionGroup"

describe("SectionGroup", () => {
  it("renders its title as a level-2 heading a step above the rows below it", () => {
    render(
      <SectionGroup title="Scanning">
        <div>body</div>
      </SectionGroup>,
    )

    const heading = screen.getByRole("heading", { level: 2, name: "Scanning" })
    expect(heading.className).toContain("type-title-3")
    // The `!` is load-bearing: the type scale is unlayered, so it outranks the
    // utilities layer and a plain `font-normal` would silently do nothing.
    expect(heading.className).toContain("font-normal!")
  })

  it("pins trailing content beside the title", () => {
    render(
      <SectionGroup title="Scanning" trailing={<span>3 roots</span>}>
        <div>body</div>
      </SectionGroup>,
    )

    const heading = screen.getByRole("heading", { level: 2, name: "Scanning" })
    expect(heading.parentElement?.contains(screen.getByText("3 roots"))).toBe(true)
  })

  it("renders an untitled group as bare children, dropping the header row", () => {
    render(
      <SectionGroup trailing={<span>orphaned</span>}>
        <div>body</div>
      </SectionGroup>,
    )

    expect(screen.queryByRole("heading")).toBeNull()
    // Trailing content belongs to the header, so without a title it has no
    // place to live and must not leak into the body.
    expect(screen.queryByText("orphaned")).toBeNull()
    expect(screen.getByText("body")).toBeInTheDocument()
  })

  it("keeps its own spacing when a caller adds classes", () => {
    const { container } = render(
      <SectionGroup className="pt-2">
        <div>body</div>
      </SectionGroup>,
    )

    const section = container.querySelector("section")!
    expect(section.className).toContain("space-y-2")
    expect(section.className).toContain("pt-2")
  })
})
