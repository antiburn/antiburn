import { beforeEach, describe, expect, it, vi } from "vitest"

import {
  markFallbackCommitted,
  markHealthyCommitted,
  onRendererHealthSettled,
  rendererHealth,
  resetRendererHealthForTest,
} from "./rendererHealth"

beforeEach(resetRendererHealthForTest)

describe("rendererHealth", () => {
  it("settles mounting listeners once and reports immediately afterward", () => {
    const listener = vi.fn()
    const stop = onRendererHealthSettled(listener)
    expect(markHealthyCommitted()).toBe(true)
    expect(markHealthyCommitted()).toBe(false)
    expect(listener).toHaveBeenCalledOnce()
    expect(listener).toHaveBeenCalledWith("healthy")
    stop()

    const late = vi.fn()
    onRendererHealthSettled(late)
    expect(late).toHaveBeenCalledWith("healthy")
  })

  it("allows fallback after healthy but never reverses fallback", () => {
    expect(markHealthyCommitted()).toBe(true)
    expect(markFallbackCommitted()).toBe(true)
    expect(rendererHealth()).toBe("fallback")
    expect(markHealthyCommitted()).toBe(false)
    expect(rendererHealth()).toBe("fallback")
  })

  it("removes a mounting listener before settlement", () => {
    const listener = vi.fn()
    onRendererHealthSettled(listener)()
    markFallbackCommitted()
    expect(listener).not.toHaveBeenCalled()
  })
})
