import { fireEvent, render } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

import { ScrollPane } from "./ScrollPane"

describe("ScrollPane top edge fade", () => {
  it("updates both edges on scroll and resize, and cleans up the caller's ref", () => {
    const observers: TestResizeObserver[] = []
    class TestResizeObserver {
      readonly callback: () => void
      constructor(callback: () => void) {
        this.callback = callback
        observers.push(this)
      }
      observe = vi.fn()
      disconnect = vi.fn()
    }
    vi.stubGlobal("ResizeObserver", TestResizeObserver)
    const cleanup = vi.fn()
    try {
      const view = render(
        <ScrollPane topEdgeFade bottomEdgeFade viewportRef={() => cleanup}>
          <div>Scrollable content</div>
        </ScrollPane>,
      )
      const viewport = view.container.querySelector(".ui-scroll-viewport") as HTMLDivElement
      let contentHeight = 700
      Object.defineProperties(viewport, {
        clientHeight: { configurable: true, value: 500 },
        scrollHeight: { configurable: true, get: () => contentHeight },
      })
      const observer = observers.find((candidate) =>
        candidate.observe.mock.calls.some(
          ([element]) => element === viewport.firstElementChild,
        ),
      )!
      observer.callback()
      expect(viewport).not.toHaveAttribute("data-scroll-edge-top")
      expect(viewport).toHaveAttribute("data-scroll-edge-bottom", "active")

      viewport.scrollTop = 100
      fireEvent.scroll(viewport)
      expect(viewport).toHaveAttribute("data-scroll-edge-top", "active")
      expect(viewport).toHaveAttribute("data-scroll-edge-bottom", "active")

      viewport.scrollTop = 200
      fireEvent.scroll(viewport)
      expect(viewport).toHaveAttribute("data-scroll-edge-top", "active")
      expect(viewport).not.toHaveAttribute("data-scroll-edge-bottom")

      contentHeight = 800
      observer.callback()
      expect(viewport).toHaveAttribute("data-scroll-edge-bottom", "active")
      contentHeight = 400
      viewport.scrollTop = 0
      observer.callback()
      fireEvent.scroll(viewport)
      expect(viewport).not.toHaveAttribute("data-scroll-edge-top")
      expect(viewport).not.toHaveAttribute("data-scroll-edge-bottom")

      view.unmount()
      expect(observer.disconnect).toHaveBeenCalledOnce()
      expect(cleanup).toHaveBeenCalledOnce()
    } finally {
      vi.unstubAllGlobals()
    }
  })

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
