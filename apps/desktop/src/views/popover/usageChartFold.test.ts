import { describe, expect, it } from "vitest"

import { foldActivityHeader } from "./usageChartFold"

const CHART_HEIGHT = 120

/** jsdom reports every layout box as zero, so each metric is declared. */
function metric(node: HTMLElement, name: string, value: () => number) {
  Object.defineProperty(node, name, { configurable: true, get: value })
}

/**
 * A folded chart over a list, with the layout jsdom cannot supply. The list
 * grows by whatever the fold gives back, which is what the browser does.
 */
function scene(listHeight: number, viewportWhenOpen: number) {
  const wrap = document.createElement("div")
  const chart = document.createElement("div")
  wrap.appendChild(chart)

  metric(chart, "offsetHeight", () => CHART_HEIGHT)
  const folded = () => CHART_HEIGHT - Number.parseFloat(wrap.style.height || `${CHART_HEIGHT}`)
  metric(wrap, "offsetHeight", () => CHART_HEIGHT - folded())

  const viewport = document.createElement("div")
  metric(viewport, "scrollHeight", () => listHeight)
  metric(viewport, "clientHeight", () => viewportWhenOpen + folded())

  return {
    wrap,
    viewport,
    folded,
    /** Scroll the list, then fold — and clamp as the browser would. */
    scrollTo(top: number) {
      const max = () => Math.max(0, listHeight - viewport.clientHeight)
      viewport.scrollTop = Math.min(top, max())
      foldActivityHeader(wrap, viewport)
      return { scrollTop: viewport.scrollTop, clamped: viewport.scrollTop > max() }
    },
  }
}

describe("foldActivityHeader", () => {
  it("folds the whole chart away once a long list scrolls past the range", () => {
    const s = scene(3000, 400)
    s.scrollTo(96)
    expect(s.folded()).toBe(CHART_HEIGHT)
    expect(s.wrap.style.height).toBe("0px")
  })

  it("tracks the scroll offset partway through the range", () => {
    const s = scene(3000, 400)
    s.scrollTo(48)
    expect(s.folded()).toBe(CHART_HEIGHT / 2)
  })

  it("leaves the chart open on a list that fills the viewport exactly", () => {
    const s = scene(400, 400)
    s.scrollTo(96)
    expect(s.folded()).toBe(0)
    expect(s.wrap.style.height).toBe("")
  })

  it("folds only as far as the list can afford", () => {
    // Sixty pixels of overflow. At an offset of 20 the range asks for a
    // 25px fold, and the list can spare 40, so the reader keeps the offset.
    const s = scene(460, 400)
    const at = s.scrollTo(20)
    expect(at.scrollTop).toBe(20)
    expect(at.clamped).toBe(false)
    expect(s.folded()).toBe(25)
  })

  it("keeps the offset when the affordable fold lands between two pixels", () => {
    // Sixty pixels of overflow. At an offset of 29.3 the list can spare 30.7,
    // which is not a whole number of pixels. A height that rounds down folds
    // 31, the reader falls past the end, and the browser jumps the offset
    // back. The height rounds up instead, so the fold stays affordable.
    const s = scene(460, 400)
    const at = s.scrollTo(29.3)
    expect(at.clamped).toBe(false)
    expect(s.folded()).toBe(30)
  })

  it("settles instead of oscillating on a list barely longer than the viewport", () => {
    const s = scene(460, 400)
    const seen: number[] = []
    // Every pass re-folds on the offset the pass before it left. That is the
    // loop the browser drives through its scroll events, and the loop that
    // reopened the chart before the fold was capped.
    for (let pass = 0; pass < 8; pass += 1) {
      const at = s.scrollTo(1000)
      expect(at.clamped).toBe(false)
      seen.push(s.folded())
    }
    expect(seen.at(-1)).toBe(seen.at(-2))
  })

  it("keeps the chart folded during elastic overscroll at the bottom", () => {
    const s = scene(460, 400)
    s.scrollTo(20)
    const folded = s.folded()

    // WebKit can report a temporary offset beyond the real scroll range
    // during the bottom bounce.
    s.viewport.scrollTop = 60
    foldActivityHeader(s.wrap, s.viewport)

    expect(s.folded()).toBe(folded)
  })

  it("clears every override at the top", () => {
    const s = scene(3000, 400)
    s.scrollTo(96)
    s.scrollTo(0)
    expect(s.wrap.style.height).toBe("")
    expect((s.wrap.firstElementChild as HTMLElement).style.opacity).toBe("")
  })
})
