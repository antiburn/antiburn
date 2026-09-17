import { describe, expect, it } from "vitest"

import { islandSpendFigure } from "./hudIsland"
import type { HudSpendRate } from "./hudIpc"

function spend(usdPerMinute: number, pricedShare = 1): HudSpendRate {
  return { usdPerMinute, windowSecs: 300, pricedShare }
}

describe("islandSpendFigure", () => {
  it("shows nothing without a rate, without priced tokens, or below half a cent", () => {
    expect(islandSpendFigure(null)).toBeNull()
    expect(islandSpendFigure(spend(1.5, 0))).toBeNull()
    expect(islandSpendFigure(spend(0))).toBeNull()
    expect(islandSpendFigure(spend(0.004))).toBeNull()
  })

  it("fits the figure in four characters", () => {
    expect(islandSpendFigure(spend(0.05))).toBe("$.05")
    expect(islandSpendFigure(spend(0.5))).toBe("$.50")
    expect(islandSpendFigure(spend(0.994))).toBe("$.99")
    expect(islandSpendFigure(spend(1.23))).toBe("$1.2")
    expect(islandSpendFigure(spend(9.94))).toBe("$9.9")
    expect(islandSpendFigure(spend(12.6))).toBe("$13")
    expect(islandSpendFigure(spend(120))).toBe("$120")
  })

  it("never rounds into a longer form", () => {
    // 0.995 rounds to 1.00 at two decimals, which is too wide: it moves up a form.
    expect(islandSpendFigure(spend(0.995))).toBe("$1.0")
    expect(islandSpendFigure(spend(9.96))).toBe("$10")
  })

  it("keeps a floor rate as a figure", () => {
    // A partly priced window is a floor. The wing has no room for the hedge.
    expect(islandSpendFigure(spend(2, 0.5))).toBe("$2.0")
  })
})
