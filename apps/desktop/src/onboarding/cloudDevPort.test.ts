import { afterEach, describe, expect, it, vi } from "vitest"
import { cloudDevScenarios, createCloudDevPort } from "./cloudDevPort"
describe("Cloud Dev simulation", () => {
  afterEach(() => vi.useRealTimers())
  it("loads every menu scenario without selecting local folders", async () => {
    for (const scenario of cloudDevScenarios) {
      const port = createCloudDevPort(scenario)
      const value = await port.read()
      expect(value.draft.folders.every((folder) => folder.path.startsWith("/Example/"))).toBe(
        true,
      )
      if (scenario.startsWith("complete-")) expect(await port.agent()).toBe(scenario.slice(9))
    }
  })
  it("cancels a simulated callback and disposes delayed work", async () => {
    vi.useFakeTimers()
    const port = createCloudDevPort("walkthrough")
    const listener = vi.fn()
    const stop = await port.watch(listener)
    expect((await port.begin()).status).toBe("waiting")
    await port.cancel()
    await vi.runAllTimersAsync()
    expect(listener).not.toHaveBeenCalled()
    expect((await port.read()).identity).toBeNull()
    await port.begin()
    stop()
    await vi.runAllTimersAsync()
    expect(listener).not.toHaveBeenCalled()
  })
  it("simulates auth, progress, cancellation and recovery without persistence", async () => {
    vi.useFakeTimers()
    const port = createCloudDevPort("walkthrough")
    const stop = await port.watch(() => {})
    await port.begin()
    await vi.advanceTimersByTimeAsync(1800)
    expect((await port.read()).identity?.login).toBe("example")
    await port.start([], "test")
    await vi.advanceTimersByTimeAsync(1500)
    expect((await port.read()).pendingSync?.result.status).toBe("syncing")
    await port.cancelSync("test")
    await vi.runAllTimersAsync()
    expect((await port.read()).pendingSync?.result.status).toBe("cancelled")
    await port.reconcile("test")
    await vi.runAllTimersAsync()
    expect((await port.read()).pendingSync?.result.status).toBe("syncing")
    stop()
    expect((await createCloudDevPort("walkthrough").read()).identity).toBeNull()
  })
})
