import { describe, expect, it } from "vitest"

import { BurnWakeTracker, activityWake, WAKE_QUIET_SECS } from "./hudWake"
import { SPEND_CEIL_USD_PER_MIN } from "./ledPeriod"

describe("activityWake", () => {
  it("never wakes on the first write it sees", () => {
    expect(activityWake(null, 10_000)).toBe(false)
  })

  it("wakes only after more than an hour of quiet", () => {
    expect(activityWake(10_000, 10_000 + WAKE_QUIET_SECS)).toBe(false)
    expect(activityWake(10_000, 10_000 + WAKE_QUIET_SECS + 1)).toBe(true)
  })
})

describe("BurnWakeTracker", () => {
  const high = SPEND_CEIL_USD_PER_MIN
  const low = SPEND_CEIL_USD_PER_MIN / 2

  it("waits for two polls at the ceiling", () => {
    const tracker = new BurnWakeTracker()
    expect(tracker.observe(high)).toBe(false)
    expect(tracker.observe(high)).toBe(true)
  })

  it("stays quiet below the ceiling and on cold start", () => {
    const tracker = new BurnWakeTracker()
    expect(tracker.observe(null)).toBe(false)
    expect(tracker.observe(low)).toBe(false)
    expect(tracker.observe(high)).toBe(false)
    expect(tracker.observe(low)).toBe(false)
    expect(tracker.observe(high)).toBe(false)
  })

  it("wakes again only after the rate drops", () => {
    const tracker = new BurnWakeTracker()
    tracker.observe(high)
    expect(tracker.observe(high)).toBe(true)
    expect(tracker.observe(high)).toBe(false)
    expect(tracker.observe(high)).toBe(false)
    tracker.observe(low)
    tracker.observe(high)
    expect(tracker.observe(high)).toBe(true)
  })
})
