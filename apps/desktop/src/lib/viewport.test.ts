import { act, renderHook } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { useViewportWidth } from "./viewport"

describe("CSS viewport subscription", () => {
  afterEach(() => vi.unstubAllGlobals())

  it("observes resize and native zoom events without using physical display pixels", () => {
    vi.stubGlobal("innerWidth", 1100)
    vi.stubGlobal("devicePixelRatio", 2)
    const { result, unmount } = renderHook(useViewportWidth)
    expect(result.current).toBe(1100)
    act(() => {
      vi.stubGlobal("innerWidth", 550)
      window.dispatchEvent(new Event("antiburn:interface-scale-changed"))
    })
    expect(result.current).toBe(550)
    act(() => {
      vi.stubGlobal("innerWidth", 700)
      window.dispatchEvent(new Event("resize"))
    })
    expect(result.current).toBe(700)
    const remove = vi.spyOn(window, "removeEventListener")
    unmount()
    expect(remove).toHaveBeenCalledWith("resize", expect.any(Function))
    expect(remove).toHaveBeenCalledWith(
      "antiburn:interface-scale-changed",
      expect.any(Function),
    )
  })
})
