import { afterEach, describe, expect, it, vi } from "vitest"

import {
  DEFAULT_POPOVER_HEIGHT,
  POPOVER_HEIGHTS,
  popoverHeightFor,
  prefersReducedMotion,
} from "./popoverHeight"

describe("popover heights", () => {
  it("keeps every main popover surface at the shell contract height", () => {
    expect(DEFAULT_POPOVER_HEIGHT).toBe(700)
    expect(popoverHeightFor("activity")).toBe(DEFAULT_POPOVER_HEIGHT)
    expect(popoverHeightFor("session")).toBe(DEFAULT_POPOVER_HEIGHT)
    expect(Object.keys(POPOVER_HEIGHTS)).toEqual(["activity", "session"])
  })

  it("has no surface left that wants less than the contract", () => {
    // The short one was the first-run flow, and it has its own window now.
    // Nothing else in this popover is a centred screen, so a height
    // below the resting size would mean a surface is being under-served
    // rather than deliberately compact.
    for (const height of Object.values(POPOVER_HEIGHTS)) {
      expect(height).toBeGreaterThanOrEqual(DEFAULT_POPOVER_HEIGHT)
    }
  })
})

describe("prefersReducedMotion", () => {
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it("follows the media query when the platform has one", () => {
    const matchMedia = vi.fn(() => ({ matches: true }))
    vi.stubGlobal("window", { ...globalThis.window, matchMedia })
    expect(prefersReducedMotion()).toBe(true)
    expect(matchMedia).toHaveBeenCalledWith("(prefers-reduced-motion: reduce)")
  })

  it('treats a platform without the query as "no preference"', () => {
    vi.stubGlobal("window", {})
    // The browser default, and the safe one: a missing query is not consent to
    // remove motion the reader never asked to lose.
    expect(prefersReducedMotion()).toBe(false)
  })
})
