import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { listen, UNLISTEN_MAX_ATTEMPTS, UNLISTEN_RETRY_DELAY_MS } from "./tauriEvents"

const tauriListen = vi.hoisted(() => vi.fn())

vi.mock("@tauri-apps/api/event", () => ({ listen: tauriListen }))

/** Tauri's unlisten before the page has the listener: it throws. */
function pageNotReady(): never {
  throw new TypeError("undefined is not an object (evaluating 'listeners[eventId].handlerId')")
}

describe("listen", () => {
  beforeEach(() => {
    vi.useFakeTimers()
    tauriListen.mockReset()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it("passes the event, handler and options to Tauri", async () => {
    tauriListen.mockResolvedValue(vi.fn())
    const handler = vi.fn()

    await listen("scan:status", handler, { target: "main" })

    expect(tauriListen).toHaveBeenCalledWith("scan:status", handler, { target: "main" })
  })

  it("unlistens at once when the page already has the listener", async () => {
    const unlisten = vi.fn().mockResolvedValue(undefined)
    tauriListen.mockResolvedValue(unlisten)

    const stop = await listen("scan:status", vi.fn())
    stop()

    expect(unlisten).toHaveBeenCalledTimes(1)
  })

  it("tries again until the page has the listener", async () => {
    const unlisten = vi
      .fn()
      .mockImplementationOnce(pageNotReady)
      .mockRejectedValueOnce(new TypeError("not ready"))
      .mockResolvedValue(undefined)
    tauriListen.mockResolvedValue(unlisten)

    const stop = await listen("scan:status", vi.fn())
    stop()
    await vi.advanceTimersByTimeAsync(UNLISTEN_RETRY_DELAY_MS * 5)

    expect(unlisten).toHaveBeenCalledTimes(3)
  })

  it("stops trying after the last attempt without a rejection", async () => {
    const unlisten = vi.fn(pageNotReady)
    tauriListen.mockResolvedValue(unlisten)

    const stop = await listen("scan:status", vi.fn())
    stop()
    await vi.advanceTimersByTimeAsync(UNLISTEN_RETRY_DELAY_MS * UNLISTEN_MAX_ATTEMPTS * 2)

    expect(unlisten).toHaveBeenCalledTimes(UNLISTEN_MAX_ATTEMPTS)
  })

  it("ignores a second unlisten", async () => {
    const unlisten = vi.fn().mockResolvedValue(undefined)
    tauriListen.mockResolvedValue(unlisten)

    const stop = await listen("scan:status", vi.fn())
    stop()
    stop()

    expect(unlisten).toHaveBeenCalledTimes(1)
  })
})
