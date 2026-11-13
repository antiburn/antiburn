import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
const native = vi.hoisted(() => ({
  readiness: vi.fn(),
  changed: undefined as (() => void) | undefined,
  stop: vi.fn(),
}))
vi.mock("../ipc", () => ({
  getMainWindowVisible: async () => true,
  onMainWindowVisibilityChanged: async () => () => {},
  getPromotionReadiness: native.readiness,
  onOnboardingChanged: async (listener: () => void) => {
    native.changed = listener
    return native.stop
  },
}))
beforeEach(() => {
  vi.resetModules()
  native.readiness.mockReset().mockResolvedValue({ manual: true, automatic: false })
  native.stop.mockReset()
})
afterEach(() => localStorage.clear())
describe("promotion environment", () => {
  it("permits deliberate opens while automatic display waits for native eligibility", async () => {
    const { promotionEnvironment: env } = await import("./promotionEnvironment")
    expect(env.isManualReady()).toBe(false)
    expect(await env.loadReady()).toBe(false)
    expect(env.isManualReady()).toBe(true)
    const listener = vi.fn()
    const stop = await env.onReady(listener)
    await vi.waitFor(() => expect(listener).toHaveBeenLastCalledWith(false))
    native.readiness.mockResolvedValue({ manual: true, automatic: true })
    window.dispatchEvent(new Event("focus"))
    await vi.waitFor(() => expect(listener).toHaveBeenLastCalledWith(true))
    native.readiness.mockResolvedValue({ manual: false, automatic: false })
    native.changed!()
    expect(env.isManualReady()).toBe(false)
    await vi.waitFor(() => expect(listener).toHaveBeenLastCalledWith(false))
    stop()
    expect(native.stop).toHaveBeenCalledOnce()
    const calls = native.readiness.mock.calls.length
    window.dispatchEvent(new Event("focus"))
    expect(native.readiness).toHaveBeenCalledTimes(calls)
  })
  it("fails closed when eligibility cannot load", async () => {
    native.readiness.mockRejectedValue(new Error("unavailable"))
    const { promotionEnvironment: env } = await import("./promotionEnvironment")
    expect(await env.loadReady()).toBe(false)
    expect(env.isManualReady()).toBe(false)
  })
  it("does not apply an old eligible response after onboarding resets", async () => {
    let resolve!: (value: { manual: boolean; automatic: boolean }) => void
    native.readiness.mockReturnValueOnce(
      new Promise((done) => {
        resolve = done
      }),
    )
    const { promotionEnvironment: env } = await import("./promotionEnvironment")
    const old = env.loadReady()
    native.readiness.mockResolvedValue({ manual: false, automatic: false })
    expect(await env.loadReady()).toBe(false)
    resolve({ manual: true, automatic: true })
    expect(await old).toBe(false)
    expect(env.isManualReady()).toBe(false)
  })
  it("does not publish a superseded subscription response over a newer initial load", async () => {
    let resolve!: (value: { manual: boolean; automatic: boolean }) => void
    native.readiness.mockReturnValueOnce(
      new Promise((done) => {
        resolve = done
      }),
    )
    const { promotionEnvironment: env } = await import("./promotionEnvironment")
    const listener = vi.fn()
    const stop = await env.onReady(listener)
    native.readiness.mockResolvedValue({ manual: true, automatic: true })
    expect(await env.loadReady()).toBe(true)
    resolve({ manual: true, automatic: true })
    await new Promise((done) => setTimeout(done, 0))
    expect(listener).not.toHaveBeenCalled()
    expect(env.isManualReady()).toBe(true)
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
