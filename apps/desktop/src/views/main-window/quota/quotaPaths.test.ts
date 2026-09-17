import { describe, expect, it } from "vitest"

import { quotaBandPath, quotaBandSpecs, quotaMeterPath } from "./quotaPaths"
import { QUOTA_HUE_COUNT, type QuotaSeriesRow, type QuotaTopSession } from "./quotaSeries"

/** A minimal row: `t` and any session/band values the test needs, with
 *  every other field defaulting to null so a test only states what it
 *  cares about. */
function row(
  t: number,
  values: Record<string, number | null> = {},
  meter: number | null = null,
): QuotaSeriesRow {
  return { t, index: 0, meter, other: null, unattributed: null, ...values }
}

// Identity-ish scales so expected coordinates read straight off the fixture.
const x = (t: number) => t
const yInverted = (v: number) => 100 - v
const yIdentity = (v: number) => v

describe("quotaBandPath", () => {
  it("draws one subpath for a single run, stepped, closing where the value returns to zero", () => {
    const rows = [row(0, { a: 0 }), row(1, { a: 10 }), row(2, { a: 20 }), row(3, { a: 0 })]
    const { d, vertices } = quotaBandPath(rows, ["a"], 0, x, yInverted)
    expect(d).toBe("M1 90L2 90L2 80L3 80L3 100L2 100L2 100L1 100Z")
    expect(vertices).toBe(8)
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

  it("traces the bottom edge along the stack beneath: the second band's bottom equals the first band's top", () => {
    const rows = [row(0, { a: 0, b: 0 }), row(1, { a: 5, b: 8 }), row(2, { a: 0, b: 0 })]
    const bandA = quotaBandPath(rows, ["a", "b"], 0, x, yInverted)
    const bandB = quotaBandPath(rows, ["a", "b"], 1, x, yInverted)
    expect(bandA.d).toBe("M1 95L2 95L2 100L1 100Z")
    expect(bandB.d).toBe("M1 87L2 87L2 95L1 95Z")
    // Band b's stack sits on band a: the same "1 95L2 95" top edge appears
    // (reversed) as band b's bottom edge.
    expect(bandB.d).toContain("L2 95L1 95")
  })

  it("produces an empty path for a band with no factor (every row null)", () => {
    const rows = [row(0, { a: null }), row(1, { a: null })]
    const { d, vertices } = quotaBandPath(rows, ["a"], 0, x, yInverted)
    expect(d).toBe("")
    expect(vertices).toBe(0)
  })

  it("thins an interior row whose top and below repeat the row before it, keeping the run's first and last rows", () => {
    const rows = [
      row(0, { a: 0 }),
      row(1, { a: 10 }),
      row(2, { a: 10 }), // unchanged from row 1: thinned away
      row(3, { a: 20 }),
      row(4, { a: 0 }),
    ]
    const { d, vertices } = quotaBandPath(rows, ["a"], 0, x, yInverted)
    expect(d).toBe("M1 90L3 90L3 80L4 80L4 100L3 100L3 100L1 100Z")
    expect(vertices).toBe(8)
    // Row 2 (t=2) contributed nothing: its x never appears in the path.
    expect(d).not.toMatch(/(^|[ML])2 /)
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
  })
})

describe("quotaMeterPath", () => {
  it("breaks into a new M at each null reading and never closes", () => {
    const rows = [row(0, {}, 10), row(1, {}, null), row(2, {}, 20), row(3, {}, 25)]
    const { d, vertices } = quotaMeterPath(rows, x, yIdentity)
    expect(d).toBe("M0 10M2 20L3 25")
    expect(d).not.toContain("Z")
    expect(vertices).toBe(3)
  })
})
