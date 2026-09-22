import { describe, expect, it } from "vitest"

import { HUD_ISLAND_OFF, islandCssGeometry, islandSpendFigure } from "./hudIsland"
import type { HudSpendRate } from "./hudIpc"

function spend(usdPerMinute: number, pricedShare = 1): HudSpendRate {
  return { usdPerMinute, windowSecs: 300, pricedShare }
}

it("keeps camera geometry fixed while converting scaled wings and body to CSS pixels", () => {
  for (const scale of [0.9, 1, 1.1, 1.25, 1.5, 1.75, 2]) {
    const state = {
      ...HUD_ISLAND_OFF,
      island: "collapsed" as const,
      wing: 30 * scale,
      fillet: 19,
      notch: 204,
      height: 32,
      scale,
      bodyWidth: 264 * Math.max(scale, 1),
      bodyMaxHeight: 500,
      headerOffset: 12,
    }
    const css = islandCssGeometry(state)
    expect(css.headerWidth * scale).toBeCloseTo(204 + 60 * scale)
    expect(css.height * scale).toBeCloseTo(32)
    expect(css.wing * scale).toBeCloseTo(30 * scale)
    expect(css.notch * scale).toBeCloseTo(204)
    expect(css.bodyWidth * scale).toBeCloseTo(state.bodyWidth)
    expect(css.bodyMaxHeight * scale).toBeCloseTo(500)
    expect(css.headerOffset * scale).toBeCloseTo(12)
  }
})

it("aligns expanded wings with the body while preserving camera placement", () => {
  for (const scale of [0.9, 1, 1.1, 1.25, 1.5, 1.75, 2]) {
    const wing = 30 * scale
    const compactWidth = 204 + 2 * wing
    const bodyWidth = Math.max(compactWidth, 264 * scale)
    const extra = bodyWidth - compactWidth
    for (const shift of [-extra / 4, 0, extra / 4]) {
      const state = {
        ...HUD_ISLAND_OFF,
        island: "expanded" as const,
        wing,
        fillet: 19,
        notch: 204,
        height: 32,
        scale,
        bodyWidth,
        headerOffset: extra / 2 + shift,
      }
      const css = islandCssGeometry(state)
      expect(css.headerWidth).toBe(css.bodyWidth)
      expect(css.headerOffset).toBe(0)
      expect(css.leftWing * scale).toBeCloseTo(state.headerOffset + wing)
      expect((css.leftWing + css.notch + css.rightWing) * scale).toBeCloseTo(bodyWidth)
      expect(css.notch * scale).toBeCloseTo(204)
      expect(css.height * scale).toBeCloseTo(32)
      expect(css.wing * scale).toBeCloseTo(wing)
      expect(css.leftWing).toBeGreaterThanOrEqual(0)
      expect(css.rightWing).toBeGreaterThanOrEqual(0)
    }
  }
})

it("keeps collapsed and preview wings compact", () => {
  for (const island of ["collapsed", "preview"] as const) {
    const css = islandCssGeometry({
      ...HUD_ISLAND_OFF,
      island,
      scale: 2,
      wing: 60,
      notch: 204,
      bodyWidth: 528,
      headerOffset: 102,
    })
    expect(css.headerWidth).toBe(162)
    expect(css.headerOffset).toBe(51)
    expect(css.leftWing).toBe(30)
    expect(css.rightWing).toBe(30)
  }
})

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
