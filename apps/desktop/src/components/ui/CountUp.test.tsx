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

  it("counts through every value in between, one step at a time", () => {
    const { container, rerender } = render(<CountUp value={10} />)
    rerender(<CountUp value={13} />)
    expect(targetText(container)).toBe("13")
    const seen: string[] = [shownText(container)]
    while (frames.length > 0) {
      frame(1_000)
      seen.push(shownText(container))
    }
    expect(seen).toEqual(["10", "11", "12", "13"])
  })

  it("moves at most one step in one frame for a large change", () => {
    const { container, rerender } = render(<CountUp value={0} />)
    rerender(<CountUp value={500} />)
    frame(1_000)
    expect(shownText(container)).toBe("1")
  })

  it("continues from the number on screen when the value changes mid-count", () => {
    const { container, rerender } = render(<CountUp value={0} />)
    rerender(<CountUp value={10} />)
    frame(1_000)
    frame(1_000)
    rerender(<CountUp value={0} />)
    frame(1_000)
    expect(shownText(container)).toBe("1")
  })
})
