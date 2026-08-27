import { fireEvent, render } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

import { ScrollPane } from "./ScrollPane"

describe("ScrollPane top edge fade", () => {
  it("activates the reusable mask only after leaving the top edge", () => {
    const viewportRef = vi.fn()
    const { container } = render(
      <ScrollPane topEdgeFade viewportRef={viewportRef}>
        <div>Scrollable content</div>
      </ScrollPane>,
    )
    const viewport = container.querySelector(".ui-scroll-viewport") as HTMLDivElement

    expect(viewportRef).toHaveBeenCalledWith(viewport)
    expect(viewport.className).toContain("scroll-edge-fade-top")
    expect(viewport.hasAttribute("data-scroll-edge-top")).toBe(false)

    viewport.scrollTop = 2
    fireEvent.scroll(viewport)
    expect(viewport.getAttribute("data-scroll-edge-top")).toBe("active")

    viewport.scrollTop = 1
    fireEvent.scroll(viewport)
    expect(viewport.hasAttribute("data-scroll-edge-top")).toBe(false)
  })

  it("does not add fade behavior unless requested", () => {
    const { container } = render(
      <ScrollPane>
        <div>Scrollable content</div>
      </ScrollPane>,
    )
    const viewport = container.querySelector(".ui-scroll-viewport") as HTMLDivElement

    viewport.scrollTop = 20
    fireEvent.scroll(viewport)

    expect(viewport.className).not.toContain("scroll-edge-fade-top")
    expect(viewport.hasAttribute("data-scroll-edge-top")).toBe(false)
  })
})
