import { render } from "@testing-library/react"
import { describe, expect, it } from "vitest"

import { deriveTokenMap } from "../../lib/tokenMap"
import { TokenMap } from "./TokenMap"

describe("TokenMap", () => {
  it("draws one frame per session and one circle per dot", () => {
    const layout = deriveTokenMap({
      nowEpoch: 1_000,
      windowSecs: 300,
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
    expect(container.querySelectorAll("rect")).toHaveLength(1)
    const circles = container.querySelectorAll("circle")
    expect(circles).toHaveLength(6)
    expect(container.querySelectorAll('circle[data-mode="looking"]')).toHaveLength(2)
    expect(container.querySelectorAll('circle[data-mode="other"]')).toHaveLength(2)
    expect(container.querySelectorAll("circle.token-map-live")).toHaveLength(1)
    expect(container.querySelector("svg")?.getAttribute("data-dot-value")).toBe("250")
  })
})
