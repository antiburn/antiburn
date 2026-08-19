// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { describe, expect, it } from "vitest"

import {
  clusterMarkers,
  dotSize,
  markerX,
  type SegmentSpan,
  type TimelineMarker,
} from "./markerCluster"

// A single full-width span makes the active-time axis linear: progress p maps
// straight to x = p * 100.
const ONE_SPAN: SegmentSpan[] = [{ x0: 0, x1: 100, cumStartMs: 0, activeMs: 1000 }]

/** A marker carrying a trivial label payload — clustering never reads `data`. */
function m(progress: number, label = ""): TimelineMarker<string> {
  return { progress, data: label }
}

describe("markerX", () => {
  it("returns 0 for empty geometry", () => {
    expect(markerX(0.5, [])).toBe(0)
  })

  it("maps progress linearly across a single full-width span", () => {
    expect(markerX(0, ONE_SPAN)).toBe(0)
    expect(markerX(0.5, ONE_SPAN)).toBe(50)
    expect(markerX(1, ONE_SPAN)).toBe(100)
  })

  it("clamps out-of-range progress into the chart", () => {
    expect(markerX(-0.5, ONE_SPAN)).toBe(0)
    expect(markerX(1.5, ONE_SPAN)).toBe(100)
  })

  it("falls back to plain progress when total active ms is zero", () => {
    const zero: SegmentSpan[] = [{ x0: 0, x1: 100, cumStartMs: 0, activeMs: 0 }]
    expect(markerX(0.3, zero)).toBeCloseTo(30)
  })

  it("interpolates within the containing segment's painted span", () => {
    // The first span is painted narrow but covers little active time; the
    // second fills the rest. A marker lands in the segment that owns its
    // active-time, then interpolates inside that segment's paint width.
    const geom: SegmentSpan[] = [
      { x0: 0, x1: 20, cumStartMs: 0, activeMs: 100 },
      { x0: 20, x1: 100, cumStartMs: 100, activeMs: 900 },
    ]
    expect(markerX(0.05, geom)).toBeCloseTo(10)
    expect(markerX(0.55, geom)).toBeCloseTo(60)
  })
})

describe("clusterMarkers", () => {
  it("keeps well-separated markers as individual clusters", () => {
    const out = clusterMarkers([m(0, "a"), m(0.5, "b")], ONE_SPAN, 0)
    expect(out).toHaveLength(2)
    expect(out.map((c) => c.x)).toEqual([0, 50])
  })

  it("merges near-coincident markers and reports the count-weighted mean x", () => {
    const out = clusterMarkers([m(0.5, "a"), m(0.51, "b")], ONE_SPAN, 0)
    expect(out).toHaveLength(1)
    expect(out[0]?.members).toHaveLength(2)
    expect(out[0]?.x).toBeCloseTo(50.5)
  })

  it("merges size-scaled dots whose circles would collide once width is known", () => {
    // x 50 vs 55 clears the same-instant pass, but on a 100px chart the 22px
    // dots' centers are 5px apart, well under the 24px they need.
    expect(clusterMarkers([m(0.5, "a"), m(0.55, "b")], ONE_SPAN, 100)).toHaveLength(1)
    // Without a measured width the size-aware pass is skipped.
    expect(clusterMarkers([m(0.5, "a"), m(0.55, "b")], ONE_SPAN, 0)).toHaveLength(2)
  })

  it("re-checks the left neighbour after a merge without over-merging", () => {
    // b and c merge; the merge widens the dot, so the loop steps back to
    // re-test a against the merged dot — which is still far enough away.
    const out = clusterMarkers([m(0.1, "a"), m(0.4, "b"), m(0.6, "c")], ONE_SPAN, 100)
    expect(out).toHaveLength(2)
    expect(out[0]?.members).toHaveLength(1)
    expect(out[0]?.x).toBeCloseTo(10)
    expect(out[1]?.members).toHaveLength(2)
    expect(out[1]?.x).toBeCloseTo(50)
  })

  it("collapses a tight chain of dots into a single cluster", () => {
    const out = clusterMarkers([m(0.5), m(0.54), m(0.58), m(0.62)], ONE_SPAN, 100)
    expect(out).toHaveLength(1)
    expect(out[0]?.members).toHaveLength(4)
  })

  it("returns nothing for no markers", () => {
    expect(clusterMarkers([], ONE_SPAN, 100)).toEqual([])
  })
})

describe("dotSize", () => {
  it("is the base diameter for a lone dot", () => {
    expect(dotSize(1)).toBe(22)
  })

  it("grows by a fixed step per extra member", () => {
    expect(dotSize(2)).toBe(22.5)
    expect(dotSize(3)).toBe(23)
  })

  it("caps at the legibility ceiling for a large fan-out", () => {
    expect(dotSize(100)).toBe(24)
  })
})
