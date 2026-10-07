import { act, render } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { CountUp } from "./CountUp"

function shownText(container: HTMLElement): string {
  return container.querySelector("[aria-hidden='true']")?.textContent ?? ""
}

function targetText(container: HTMLElement): string {
  return container.querySelector("[data-count-up-value]")?.textContent ?? ""
}

let frames: FrameRequestCallback[] = []
let now = 0

beforeEach(() => {
  frames = []
  now = 0
  vi.spyOn(performance, "now").mockImplementation(() => now)
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
    frames.push(callback)
    return frames.length
  })
  vi.stubGlobal("cancelAnimationFrame", () => {
    frames = []
  })
})

afterEach(() => {
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
})

/** Run one animation frame `ms` after the last one. */
function frame(ms: number) {
  now += ms
  const pending = frames
  frames = []
  act(() => pending.forEach((callback) => callback(now)))
}

describe("CountUp", () => {
  it("shows the first value immediately", () => {
    const { container } = render(<CountUp value={114} />)
    expect(shownText(container)).toBe("114")
    expect(frames).toHaveLength(0)
  })

  it("animates a small change over the slow duration", () => {
    const { container, rerender } = render(<CountUp value={10} />)
    rerender(<CountUp value={13} />)
    expect(targetText(container)).toBe("13")
    expect(shownText(container)).toBe("10")
    frame(100)
    expect(shownText(container)).toBe("11")
    frame(100)
    expect(shownText(container)).toBe("12")
    frame(100)
    expect(shownText(container)).toBe("13")
  })

  it("finishes a large change within the slow duration", () => {
    const { container, rerender } = render(<CountUp value={0} />)
    rerender(<CountUp value={10_000} />)
    frame(299)
    expect(shownText(container)).not.toBe("0")
    expect(shownText(container)).not.toBe("10,000")
    frame(1)
    expect(shownText(container)).toBe("10,000")
  })

  it("continues from the number on screen when the value changes mid-count", () => {
    const { container, rerender } = render(<CountUp value={0} />)
    rerender(<CountUp value={10} />)
    frame(100)
    frame(100)
    expect(shownText(container)).toBe("7")
    rerender(<CountUp value={0} />)
    expect(shownText(container)).toBe("7")
    frame(300)
    expect(shownText(container)).toBe("0")
  })
})
