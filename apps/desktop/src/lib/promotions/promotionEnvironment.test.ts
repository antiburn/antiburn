import { afterEach, describe, expect, it, vi } from "vitest"
vi.mock("../ipc", () => ({
  getMainWindowVisible: async () => true,
  onMainWindowVisibilityChanged: async () => () => {},
}))
afterEach(() => localStorage.clear())
describe("promotion environment", () => {
  it("does not wait for retired setup state", async () => {
    localStorage.setItem("antiburn.promotions.ftue.v1", JSON.stringify({ completedAt: null }))
    const { promotionEnvironment: env } = await import("./promotionEnvironment")
    expect(env.isManualReady()).toBe(true)
    expect(await env.loadReady()).toBe(true)
    const listener = vi.fn()
    const stop = await env.onReady(listener)
    expect(listener).toHaveBeenCalledWith(true)
    stop()
  })
  it("keeps launcher history independent of automatic seen history", async () => {
    const { promotionEnvironment: env } = await import("./promotionEnvironment")
    env.writeLauncherOpened('["claude-mod@1"]')
    expect(env.readSeen()).toBeNull()
    env.writeSeen('["claude-mod@2"]')
    expect(env.readLauncherOpened()).toBe('["claude-mod@1"]')
  })
})
