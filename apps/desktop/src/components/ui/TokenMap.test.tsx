import { fireEvent, render } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

import { deriveTokenMap } from "../../lib/tokenMap"
import { TokenMap } from "./TokenMap"

describe("TokenMap", () => {
  it("draws one frame per session and one circle per dot", () => {
    const layout = deriveTokenMap({
      nowEpoch: 1_000,
      windowSecs: 300,
      spend: null,
      sessions: [
        {
          agent: "claude-code",
          sessionId: "s1",
          title: null,
          lastTurnEpoch: 995,
          tokensPerMin: 1_000,
          modes: {
            looking: 2_500,
            running: 0,
            changing: 2_500,
            delegating: 0,
            thinking: 0,
            talking: 0,
            other: 0,
          },
          subagents: [
            {
              subagentId: "sub",
              tokensPerMin: 500,
              modes: {
                looking: 0,
                running: 0,
                changing: 0,
                delegating: 0,
                thinking: 0,
                talking: 0,
                other: 2_500,
              },
            },
          ],
        },
      ],
    })
    const { container } = render(<TokenMap layout={layout} />)
    const frame = container.querySelector("rect")!
    expect(container.querySelectorAll("rect")).toHaveLength(1)
    // Each corner hugs a dot: the radius is the LED radius plus the frame pad.
    expect(Number(frame.getAttribute("rx"))).toBe(4.5)
    const circles = container.querySelectorAll("circle")
    expect(circles).toHaveLength(6)
    // Dots are LED-sized, sub-agent dots smaller.
    const radii = [...circles].map((circle) => Number(circle.getAttribute("r")))
    expect(radii).toEqual([3, 3, 3, 3, 2, 2])
    expect(container.querySelectorAll('circle[data-mode="looking"]')).toHaveLength(2)
    expect(container.querySelectorAll('circle[data-mode="other"]')).toHaveLength(2)
    expect(container.querySelectorAll("circle.token-map-live")).toHaveLength(1)
    expect(container.querySelector("svg")?.getAttribute("data-dot-value")).toBe("250")
  })

  it("reports the blob under the pointer and null when it leaves", () => {
    const layout = deriveTokenMap({
      nowEpoch: 1_000,
      windowSecs: 300,
      spend: null,
      sessions: [
        {
          agent: "claude-code",
          sessionId: "s1",
          title: null,
          lastTurnEpoch: 1_000,
          tokensPerMin: 1_000,
          modes: {
            looking: 1,
            running: 0,
            changing: 0,
            delegating: 0,
            thinking: 0,
            talking: 0,
            other: 0,
          },
          subagents: [],
        },
      ],
    })
    const onHoverBlob = vi.fn()
    const { container } = render(<TokenMap layout={layout} onHoverBlob={onHoverBlob} />)
    const blob = container.querySelector("g[data-blob]")!
    fireEvent.mouseEnter(blob)
    expect(onHoverBlob).toHaveBeenLastCalledWith("claude-code:s1")
    fireEvent.mouseLeave(blob)
    expect(onHoverBlob).toHaveBeenLastCalledWith(null)
  })

  it("names the sub-agent under the pointer and clears it on leave", () => {
    const layout = deriveTokenMap({
      nowEpoch: 1_000,
      windowSecs: 300,
      spend: null,
      sessions: [
        {
          agent: "claude-code",
          sessionId: "s1",
          title: null,
          lastTurnEpoch: 1_000,
          tokensPerMin: 1_000,
          modes: {
            looking: 1,
            running: 0,
            changing: 0,
            delegating: 0,
            thinking: 0,
            talking: 0,
            other: 0,
          },
          subagents: [
            {
              subagentId: "sub-1",
              tokensPerMin: 500,
              modes: {
                looking: 0,
                running: 1,
                changing: 0,
                delegating: 0,
                thinking: 0,
                talking: 0,
                other: 0,
              },
            },
          ],
        },
      ],
    })
    const onHoverBlob = vi.fn()
    const { container } = render(<TokenMap layout={layout} onHoverBlob={onHoverBlob} />)
    const dot = container.querySelector('circle[data-subagent="sub-1"]')!
    expect(dot).not.toBeNull()
    fireEvent.mouseEnter(dot)
    expect(onHoverBlob).toHaveBeenLastCalledWith("claude-code:s1", "sub-1")
    fireEvent.mouseLeave(dot)
    // The leave clears the owner; the group may report its own leave after.
    expect(onHoverBlob).toHaveBeenCalledWith("claude-code:s1", null)
  })

  it("crops the square to the rows in use", () => {
    const layout = deriveTokenMap({
      nowEpoch: 1_000,
      windowSecs: 300,
      spend: null,
      sessions: [
        {
          agent: "claude-code",
          sessionId: "s1",
          title: null,
          lastTurnEpoch: 1_000,
          tokensPerMin: 1_000,
          modes: {
            looking: 1,
            running: 0,
            changing: 0,
            delegating: 0,
            thinking: 0,
            talking: 0,
            other: 0,
          },
          subagents: [],
        },
      ],
    })
    const { container } = render(<TokenMap layout={layout} />)
    // Four dots pack two by two: 140px wide, two LED rows tall.
    expect(container.querySelector("svg")?.getAttribute("viewBox")).toBe("0 0 140 15")
  })
})
