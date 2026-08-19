// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { relativeTime } from "./relativeTime"

const NOW = new Date("2026-03-04T12:00:00.000Z")

/** An ISO timestamp `ms` before the frozen clock. */
function ago(ms: number): string {
  return new Date(NOW.getTime() - ms).toISOString()
}

beforeEach(() => {
  vi.useFakeTimers()
  vi.setSystemTime(NOW)
})

afterEach(() => {
  vi.useRealTimers()
})

describe("relativeTime", () => {
  it('reads "never" for a missing or unparseable timestamp', () => {
    expect(relativeTime(null)).toBe("never")
    expect(relativeTime(undefined)).toBe("never")
    expect(relativeTime("")).toBe("never")
    expect(relativeTime("not a date")).toBe("never")
  })

  it('collapses the first five seconds, and any future skew, into "just now"', () => {
    expect(relativeTime(ago(0))).toBe("just now")
    expect(relativeTime(ago(4_999))).toBe("just now")
    expect(relativeTime(new Date(NOW.getTime() + 60_000).toISOString())).toBe("just now")
  })

  it("steps through seconds, minutes, hours, and days", () => {
    expect(relativeTime(ago(12_000))).toBe("12s ago")
    expect(relativeTime(ago(3 * 60_000))).toBe("3m ago")
    expect(relativeTime(ago(2 * 3_600_000))).toBe("2h ago")
    expect(relativeTime(ago(4 * 86_400_000))).toBe("4d ago")
  })

  it("drops the suffix in compact mode", () => {
    expect(relativeTime(ago(12_000), { compact: true })).toBe("12s")
    expect(relativeTime(ago(3 * 60_000), { compact: true })).toBe("3m")
    expect(relativeTime(ago(0), { compact: true })).toBe("now")
  })
})
