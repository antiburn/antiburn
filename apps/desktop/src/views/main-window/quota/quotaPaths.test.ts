import { describe, expect, it } from "vitest"

import { quotaBandPaths, quotaBandSpecs, quotaStackTopPath } from "./quotaPaths"
import { QUOTA_HUE_COUNT, type QuotaSeriesRow, type QuotaTopSession } from "./quotaSeries"

/** A minimal row: `t` and any session/band values the test needs, with
 *  every other field defaulting to null so a test only states what it
 *  cares about. */
function row(
  t: number,
  values: Record<string, number | null> = {},
  meter: number | null = null,
): QuotaSeriesRow {
  return {
    t,
    index: 0,
    meter,
    other: null,
    unattributed: null,
    unexplained: null,
    ...values,
  }
}

// Identity-ish scales so expected coordinates read straight off the fixture.
const x = (t: number) => t
const yInverted = (v: number) => 100 - v
const yIdentity = (v: number) => v

function quotaBandPath(
  rows: readonly QuotaSeriesRow[],
  keys: readonly string[],
  index: number,
  scaleX: (value: number) => number,
  scaleY: (value: number) => number,
) {
  return quotaBandPaths(rows, keys, scaleX, scaleY)[index]!
}

describe("quotaBandPaths", () => {
  it("ramps between rows with a straight edge, closing to a point where the value returns to zero", () => {
    const rows = [row(0, { a: 0 }), row(1, { a: 10 }), row(2, { a: 20 }), row(3, { a: 0 })]
    const { d, vertices } = quotaBandPath(rows, ["a"], 0, x, yInverted)
    // Top edge ramps 1->2 (10 to 20 percent), then closes at row 3's own
    // (zero) height instead of stepping down after it. Bottom edge stays
    // at zero throughout, so its close lands on the same point.
    expect(d).toBe("M1 90L2 80L3 100L3 100L2 100L1 100Z")
    expect(vertices).toBe(6)
  })

  it("reaches the series' end with no closing point", () => {
    const rows = [row(0, { a: 0 }), row(1, { a: 10 }), row(2, { a: 20 })]
    const { d, vertices } = quotaBandPath(rows, ["a"], 0, x, yInverted)
    expect(d).toBe("M1 90L2 80L2 100L1 100Z")
    expect(vertices).toBe(4)
  })

  it("splits two runs separated by a zero row into two M subpaths", () => {
    const rows = [
      row(0, { a: 0 }),
      row(1, { a: 10 }),
      row(2, { a: 0 }),
      row(3, { a: 5 }),
      row(4, { a: 0 }),
    ]
    const { d } = quotaBandPath(rows, ["a"], 0, x, yInverted)
    expect(d.match(/M/g)).toHaveLength(2)
  })

  it("traces the bottom edge along the stack beneath: the second band's bottom matches the first band's top", () => {
    const rows = [row(0, { a: 0, b: 0 }), row(1, { a: 5, b: 8 }), row(2, { a: 0, b: 0 })]
    const bandA = quotaBandPath(rows, ["a", "b"], 0, x, yInverted)
    const bandB = quotaBandPath(rows, ["a", "b"], 1, x, yInverted)
    expect(bandA.d).toBe("M1 95L2 100L2 100L1 100Z")
    expect(bandB.d).toBe("M1 87L2 100L2 100L1 95Z")
    // Band b's stack sits on band a: the same "1 95" / "2 100" vertices
    // that trace band a's top edge reappear, reversed, as band b's bottom
    // edge.
    expect(bandB.d).toContain("L2 100L1 95")
  })

  it("produces an empty path for a band with no factor (every row null)", () => {
    const rows = [row(0, { a: null }), row(1, { a: null })]
    const { d, vertices } = quotaBandPath(rows, ["a"], 0, x, yInverted)
    expect(d).toBe("")
    expect(vertices).toBe(0)
  })

  it("thins a plateau row that repeats both its neighbors, keeping the run's first and last rows", () => {
    const rows = [
      row(0, { a: 0 }),
      row(1, { a: 10 }),
      row(2, { a: 10 }), // unchanged on both sides: thinned away
      row(3, { a: 10 }),
      row(4, { a: 0 }),
    ]
    const { d, vertices } = quotaBandPath(rows, ["a"], 0, x, yInverted)
    expect(d).toBe("M1 90L3 90L4 100L4 100L3 100L1 100Z")
    expect(vertices).toBe(6)
    // Row 2 (t=2) contributed nothing: its x never appears in the path.
    expect(d).not.toMatch(/(^|[ML])2 /)
  })

  it("keeps a row where a plateau ends and a ramp begins, since it is a real vertex", () => {
    const rows = [
      row(0, { a: 0 }),
      row(1, { a: 10 }),
      row(2, { a: 10 }), // same as row 1, but row 3 climbs: kept
      row(3, { a: 20 }),
      row(4, { a: 0 }),
    ]
    const { d, vertices } = quotaBandPath(rows, ["a"], 0, x, yInverted)
    expect(d).toBe("M1 90L2 90L3 80L4 100L4 100L3 100L2 100L1 100Z")
    expect(vertices).toBe(8)
  })
})

describe("quotaBandSpecs", () => {
  function topSession(hue: number): QuotaTopSession {
    return {
      key: "s",
      agent: "claude",
      sessionId: "s",
      wslDistro: null,
      title: null,
      usd: 0,
      hue,
    }
  }

  it("fills each band from session.hue modulo the hue count", () => {
    const specs = quotaBandSpecs([
      topSession(0),
      topSession(QUOTA_HUE_COUNT),
      topSession(QUOTA_HUE_COUNT + 2),
    ])
    expect(specs[0]!.fill).toBe("var(--color-quota-session-1)")
    // A hue past the palette wraps back into range instead of naming a
    // token the palette does not define.
    expect(specs[1]!.fill).toBe("var(--color-quota-session-1)")
    expect(specs[2]!.fill).toBe("var(--color-quota-session-3)")
    const [other, unattributed] = specs.slice(3)
    expect(other!.fill).toBe("var(--color-chart-rest-strong)")
    expect(unattributed!.fill).toBe("var(--color-chart-rest-faint)")
  })
})

describe("quotaStackTopPath", () => {
  it("sums every band's own value at each row, treating a lone null band as zero", () => {
    const rows = [
      row(0, { a: 3, b: 4 }),
      row(1, { a: 5, b: 5 }),
      // b is null here, but a still holds a value, so the row is not a gap
      // row: the sum counts only a, treating b as zero.
      row(2, { a: 6, b: null }),
    ]
    const { d, vertices } = quotaStackTopPath(rows, ["a", "b"], x, yIdentity)
    expect(d).toBe("M0 7L1 10L2 6")
    expect(vertices).toBe(3)
  })

  it("breaks into a new M at a gap row, where every band's own value is null", () => {
    const rows = [row(0, { a: 5, b: 3 }), row(1, { a: null, b: null }), row(2, { a: 2, b: 1 })]
    const { d, vertices } = quotaStackTopPath(rows, ["a", "b"], x, yIdentity)
    expect(d).toBe("M0 8M2 3")
    expect(d).not.toContain("Z")
    expect(vertices).toBe(2)
  })

  it("draws a lone active row bounded by gap rows as its own single-point M", () => {
    const rows = [row(0, { a: null }), row(1, { a: 5 }), row(2, { a: null })]
    const { d, vertices } = quotaStackTopPath(rows, ["a"], x, yIdentity)
    expect(d).toBe("M1 5")
    expect(vertices).toBe(1)
  })
})
