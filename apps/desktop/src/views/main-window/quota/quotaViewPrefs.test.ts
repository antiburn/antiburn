import { afterEach, describe, expect, it, vi } from "vitest"

import { readQuotaViewPrefs, writeQuotaViewPrefs } from "./quotaViewPrefs"

const KEY = "antiburn.quota.view.v1"

afterEach(() => {
  localStorage.clear()
  vi.restoreAllMocks()
})

describe("quotaViewPrefs", () => {
  it("round-trips a write and merges a later partial write", () => {
    writeQuotaViewPrefs({ provider: "anthropic", accountKey: "acct-1" })
    writeQuotaViewPrefs({
      lane: "weekly",
      rangePreset: "thisWeek",
      axisMode: "date",
      showPace: false,
    })
    expect(readQuotaViewPrefs()).toEqual({
      provider: "anthropic",
      accountKey: "acct-1",
      lane: "weekly",
      rangePreset: "thisWeek",
      axisMode: "date",
      showPace: false,
    })
  })

  it("reads an empty object when nothing is saved", () => {
    expect(readQuotaViewPrefs()).toEqual({})
  })

  it("reads an empty object for a malformed stored value", () => {
    localStorage.setItem(KEY, "not json")
    expect(readQuotaViewPrefs()).toEqual({})

    localStorage.setItem(KEY, JSON.stringify([1, 2, 3]))
    expect(readQuotaViewPrefs()).toEqual({})

    localStorage.setItem(KEY, JSON.stringify("a string"))
    expect(readQuotaViewPrefs()).toEqual({})
  })

  it("returns an empty object and does not throw when reading storage throws", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("no storage")
    })
    expect(() => readQuotaViewPrefs()).not.toThrow()
    expect(readQuotaViewPrefs()).toEqual({})
  })

  it("does not throw when writing storage throws", () => {
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("no storage")
    })
    expect(() => writeQuotaViewPrefs({ provider: "anthropic" })).not.toThrow()
  })
})
