import { describe, expect, it, vi } from "vitest"
import { WindowChromeSession } from "./WindowChromeSession"

function setup() {
  let resized = () => {}
  const disconnect = vi.fn()
  const native = {
    isMaximized: vi.fn().mockResolvedValue(false),
    onResized: vi.fn(async (listener: () => void) => {
      resized = listener
      return disconnect
    }),
    minimize: vi.fn().mockResolvedValue(undefined),
    toggleMaximize: vi.fn().mockResolvedValue(undefined),
    close: vi.fn().mockResolvedValue(undefined),
  }
  return {
    native,
    disconnect,
    resized: () => resized(),
    session: new WindowChromeSession(() => native),
  }
}

describe("window chrome", () => {
  it("tracks native maximize changes and releases its listener", async () => {
    const { session, native, resized, disconnect } = setup()
    const stop = session.subscribe(vi.fn())
    await vi.waitFor(() => expect(native.isMaximized).toHaveBeenCalledOnce())
    native.isMaximized.mockResolvedValue(true)
    resized()
    await vi.waitFor(() => expect(session.getSnapshot().maximized).toBe(true))
    stop()
    expect(disconnect).toHaveBeenCalledOnce()
  })
  it("routes close through the native close policy and reports action failures", async () => {
    const { session, native } = setup()
    await session.perform("minimize")
    await session.perform("close")
    expect(native.minimize).toHaveBeenCalledOnce()
    expect(native.close).toHaveBeenCalledOnce()
    native.toggleMaximize.mockRejectedValue(new Error("denied"))
    await session.perform("toggleMaximize")
    expect(session.getSnapshot().error).toContain("Could not change")
    native.toggleMaximize.mockResolvedValue(undefined)
    native.isMaximized.mockResolvedValue(true)
    await session.perform("toggleMaximize")
    expect(session.getSnapshot()).toEqual({ maximized: true, error: "" })
  })
  it("cleans up a listener that arrives after unmount", async () => {
    const { session, native, disconnect } = setup()
    let resolve!: (stop: typeof disconnect) => void
    native.onResized.mockImplementation(
      () =>
        new Promise((done) => {
          resolve = done
        }),
    )
    const stop = session.subscribe(vi.fn())
    stop()
    resolve(disconnect)
    await vi.waitFor(() => expect(disconnect).toHaveBeenCalledOnce())
    expect(native.isMaximized).not.toHaveBeenCalled()
  })
  it("ignores an older maximize response", async () => {
    const { session, native, resized } = setup()
    const stop = session.subscribe(vi.fn())
    await vi.waitFor(() => expect(native.isMaximized).toHaveBeenCalledOnce())
    let resolve!: (value: boolean) => void
    native.isMaximized.mockImplementationOnce(
      () =>
        new Promise((done) => {
          resolve = done
        }),
    )
    resized()
    native.isMaximized.mockResolvedValue(true)
    resized()
    await vi.waitFor(() => expect(session.getSnapshot().maximized).toBe(true))
    resolve(false)
    await Promise.resolve()
    expect(session.getSnapshot().maximized).toBe(true)
    stop()
  })
  it("ignores an older state failure after a successful refresh", async () => {
    const { session, native, resized } = setup()
    const stop = session.subscribe(vi.fn())
    await vi.waitFor(() => expect(native.isMaximized).toHaveBeenCalledOnce())
    let reject!: (reason: Error) => void
    native.isMaximized.mockImplementationOnce(
      () =>
        new Promise((_, fail) => {
          reject = fail
        }),
    )
    resized()
    native.isMaximized.mockResolvedValue(true)
    resized()
    await vi.waitFor(() => expect(session.getSnapshot().maximized).toBe(true))
    reject(new Error("stale"))
    await Promise.resolve()
    expect(session.getSnapshot().error).toBe("")
    stop()
  })
})
