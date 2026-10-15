import { beforeEach, describe, expect, it, vi } from "vitest"

import { EMPTY_LIVE_USAGE, refreshLiveUsage } from "./ipc"

const invoke = vi.hoisted(() => vi.fn())

vi.mock("@tauri-apps/api/core", () => ({ invoke, isTauri: () => true }))

describe("refreshLiveUsage", () => {
  beforeEach(() => {
    invoke.mockReset()
  })

  it("shares one check between calls at the same time", async () => {
    let finish: (value: null) => void = () => undefined
    invoke.mockReturnValue(new Promise<null>((resolve) => (finish = resolve)))

    const first = refreshLiveUsage()
    const second = refreshLiveUsage()
    finish(null)

    await expect(first).resolves.toEqual(EMPTY_LIVE_USAGE)
    await expect(second).resolves.toEqual(EMPTY_LIVE_USAGE)
    expect(invoke).toHaveBeenCalledTimes(1)
  })

  it("starts a new check after the last one ends", async () => {
    invoke.mockResolvedValue(null)
    await refreshLiveUsage()
    await refreshLiveUsage()
    expect(invoke).toHaveBeenCalledTimes(2)
  })

  it("starts a new check after the last one failed", async () => {
    invoke.mockRejectedValueOnce(new Error("synthetic failure"))
    await expect(refreshLiveUsage()).rejects.toThrow("synthetic failure")
    invoke.mockResolvedValue(null)
    await expect(refreshLiveUsage()).resolves.toEqual(EMPTY_LIVE_USAGE)
    expect(invoke).toHaveBeenCalledTimes(2)
  })
})
