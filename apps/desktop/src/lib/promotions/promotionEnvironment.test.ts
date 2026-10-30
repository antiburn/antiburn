import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

const onboarding = vi.hoisted(() => ({
  progress: { mode: "pending", flow: "welcome" } as {
    mode: "pending" | "firstRun" | "steady"
    flow: string
  },
}))
vi.mock("../../views/main-window/overview/overviewProgressStore", () => ({
  overviewProgress: () => onboarding.progress,
  subscribeOverviewProgress: () => () => {},
}))
vi.mock("../ipc", () => ({
  getMainWindowVisible: async () => true,
  onMainWindowVisibilityChanged: async () => () => {},
}))

beforeEach(() => {
  vi.resetModules()
  localStorage.clear()
  vi.useFakeTimers()
  vi.setSystemTime(new Date("2026-10-27T00:00:00Z"))
  onboarding.progress = { mode: "pending", flow: "welcome" }
})
afterEach(() => {
  vi.useRealTimers()
  localStorage.clear()
})

describe("promotion environment readiness", () => {
  it("allows manual opening immediately after the final FTUE step while automatic display waits", async () => {
    const { promotionEnvironment: env } = await import("./promotionEnvironment")
    expect(env.isManualReady()).toBe(false)
    expect(await env.loadReady()).toBe(false)
    onboarding.progress = { mode: "firstRun", flow: "welcome" }
    expect(env.isManualReady()).toBe(false)
    expect(await env.loadReady()).toBe(false)
    onboarding.progress = { mode: "firstRun", flow: "done" }
    expect(env.isManualReady()).toBe(true)
    expect(await env.loadReady()).toBe(false)
    onboarding.progress = { mode: "steady", flow: "done" }
    vi.setSystemTime(Date.now() + 24 * 60 * 60 * 1000)
    expect(env.isManualReady()).toBe(true)
    expect(await env.loadReady()).toBe(false)
    vi.resetModules()
    const { promotionEnvironment: later } = await import("./promotionEnvironment")
    expect(await later.loadReady()).toBe(true)
  })

  it("preserves the automatic cooldown and keeps sidebar history independent of seen state", async () => {
    localStorage.setItem(
      "antiburn.promotions.ftue.v1",
      JSON.stringify({ completedAt: Date.now() }),
    )
    onboarding.progress = { mode: "steady", flow: "done" }
    const { promotionEnvironment: env } = await import("./promotionEnvironment")
    expect(env.isManualReady()).toBe(true)
    expect(await env.loadReady()).toBe(false)
    env.writeLauncherOpened('["claude-mod@1"]')
    expect(env.readLauncherOpened()).toBe('["claude-mod@1"]')
    expect(env.readSeen()).toBeNull()
    env.writeSeen('["claude-mod@2"]')
    expect(env.readLauncherOpened()).toBe('["claude-mod@1"]')
    vi.setSystemTime(Date.now() + 24 * 60 * 60 * 1000)
    expect(await env.loadReady()).toBe(true)
  })
})
