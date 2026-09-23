import { afterEach, describe, expect, it, vi } from "vitest"

import { countUp } from "./countUp"

const format = (value: number) => `${Math.round(value)}t`

describe("countUp", () => {
  afterEach(() => {
    vi.unstubAllGlobals()
    vi.restoreAllMocks()
  })

  it("shows the final value at once without animation frames", () => {
    vi.stubGlobal("requestAnimationFrame", undefined)
    const node = document.createElement("span")
    countUp(node, 500, format)
    expect(node.textContent).toBe("500t")
  })

  it("shows the final value at once with reduced motion", () => {
    vi.stubGlobal("requestAnimationFrame", () => 1)
    vi.stubGlobal("matchMedia", () => ({ matches: true }))
    const node = document.createElement("span")
    countUp(node, 500, format)
    expect(node.textContent).toBe("500t")
  })

  it("counts from zero to the value and stops when told", () => {
    const frames: FrameRequestCallback[] = []
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) =>
      frames.push(callback),
    )
    const cancel = vi.fn()
    vi.stubGlobal("cancelAnimationFrame", cancel)
    vi.stubGlobal("matchMedia", () => ({ matches: false }))
    vi.spyOn(performance, "now").mockReturnValue(1000)
    const node = document.createElement("span")
    const stop = countUp(node, 1000, format)
    frames.shift()!(1000)
    expect(node.textContent).toBe("0t")
    // Halfway through, the ease-out curve sits at 87.5%.
    frames.shift()!(1550)
    expect(node.textContent).toBe("875t")
    frames.shift()!(2200)
    expect(node.textContent).toBe("1000t")
    expect(frames).toHaveLength(0)
    stop()
    expect(cancel).toHaveBeenCalled()
  })
})
