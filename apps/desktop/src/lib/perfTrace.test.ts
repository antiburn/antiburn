import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { traceAsync, traceEvent, traceSpan } from "./perfTrace"

function dump() {
  return window.__antiburnTrace?.dump() ?? []
}

describe("perfTrace", () => {
  beforeEach(() => {
    window.__antiburnTrace?.clear()
  })

  afterEach(() => {
    vi.unstubAllEnvs()
  })

  it("is a no-op outside DEV", () => {
    vi.stubEnv("DEV", false)
    traceEvent("test.event", { a: 1 })
    expect(traceSpan("test.span", {}, () => 42)).toBe(42)
    expect(dump()).toHaveLength(0)
  })

  it("does not record an async no-op outside DEV", async () => {
    vi.stubEnv("DEV", false)
    await expect(traceAsync("test.async", {}, () => Promise.resolve("ok"))).resolves.toBe("ok")
    expect(dump()).toHaveLength(0)
  })

  it("caps the ring buffer at 2000 entries", () => {
    for (let i = 0; i < 2010; i += 1) traceEvent("test.fill", { i })
    const recorded = dump()
    expect(recorded).toHaveLength(2000)
    expect(recorded[0]?.i).toBe(10)
    expect(recorded[recorded.length - 1]?.i).toBe(2009)
  })

  it("records a span's duration and rethrows its error", () => {
    expect(() =>
      traceSpan("test.throw", { note: "boom" }, () => {
        throw new Error("boom")
      }),
    ).toThrow("boom")
    const recorded = dump()
    expect(recorded).toHaveLength(1)
    expect(recorded[0]).toMatchObject({ name: "test.throw", note: "boom", error: true })
    expect(typeof recorded[0]?.durationMs).toBe("number")
  })
})
