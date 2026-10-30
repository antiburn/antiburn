import { describe, expect, it } from "vitest"
import { PromotionTiming, PROMOTION_FTUE_DELAY_MS } from "./PromotionTiming"

const firstRun = { mode: "firstRun", flow: "welcome" } as const
const completed = { mode: "firstRun", flow: "done" } as const
const steady = { mode: "steady", flow: "done" } as const

function clock() {
  let stored: string | null = null
  let now = 1000
  const storage = {
    read: () => stored,
    write: (value: string) => {
      stored = value
    },
    now: () => now,
  }
  return {
    storage,
    advance: (milliseconds: number) => {
      now += milliseconds
    },
  }
}

describe("PromotionTiming", () => {
  it("keeps existing users eligible and waits for resolved onboarding state", () => {
    const { storage } = clock()
    const timing = new PromotionTiming(storage)
    expect(timing.ready({ mode: "pending", flow: "done" })).toBe(false)
    expect(timing.ready(steady)).toBe(true)
    expect(storage.read()).toBeNull()
  })

  it("waits 24 hours from completion and requires a later app session", () => {
    const { storage, advance } = clock()
    const initial = new PromotionTiming(storage)
    expect(initial.ready(firstRun)).toBe(false)
    advance(PROMOTION_FTUE_DELAY_MS)
    expect(initial.ready(completed)).toBe(false)
    const next = new PromotionTiming(storage)
    expect(next.ready(steady)).toBe(false)
    advance(PROMOTION_FTUE_DELAY_MS - 1)
    expect(next.ready(steady)).toBe(false)
    advance(1)
    expect(initial.ready(completed)).toBe(false)
    expect(initial.ready(steady)).toBe(false)
    expect(next.ready(steady)).toBe(true)
    expect(new PromotionTiming(storage).ready(steady)).toBe(true)
  })

  it("keeps incomplete onboarding deferred across restarts", () => {
    const { storage, advance } = clock()
    expect(new PromotionTiming(storage).ready(firstRun)).toBe(false)
    advance(PROMOTION_FTUE_DELAY_MS * 2)
    const resumed = new PromotionTiming(storage)
    expect(resumed.ready(firstRun)).toBe(false)
    expect(resumed.ready(completed)).toBe(false)
    expect(new PromotionTiming(storage).ready(steady)).toBe(false)
  })

  it("starts a conservative delay when completion occurs while the app is closed", () => {
    const { storage, advance } = clock()
    new PromotionTiming(storage).ready(firstRun)
    const resumed = new PromotionTiming(storage)
    expect(resumed.ready(steady)).toBe(false)
    advance(PROMOTION_FTUE_DELAY_MS)
    expect(resumed.ready(steady)).toBe(false)
    expect(new PromotionTiming(storage).ready(steady)).toBe(true)
  })

  it("restarts the delay when onboarding is reset", () => {
    const { storage, advance } = clock()
    const initial = new PromotionTiming(storage)
    initial.ready(completed)
    advance(PROMOTION_FTUE_DELAY_MS)
    const reset = new PromotionTiming(storage)
    expect(reset.ready(steady)).toBe(true)
    expect(reset.ready(firstRun)).toBe(false)
    expect(reset.ready(completed)).toBe(false)
    expect(new PromotionTiming(storage).ready(steady)).toBe(false)
  })

  it("keeps the current onboarding session blocked when storage fails", () => {
    const timing = new PromotionTiming({
      read: () => {
        throw new Error("unavailable")
      },
      write: () => {
        throw new Error("unavailable")
      },
      now: () => 1000,
    })
    expect(timing.ready(firstRun)).toBe(false)
    expect(timing.ready(completed)).toBe(false)
    expect(timing.ready(steady)).toBe(false)
  })
})
