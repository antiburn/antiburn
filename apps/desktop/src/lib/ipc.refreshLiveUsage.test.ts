import { beforeEach, describe, expect, it, vi } from "vitest"

import { EMPTY_LIVE_USAGE, refreshLiveUsage } from "./ipc"

const invoke = vi.hoisted(() => vi.fn())

vi.mock("@tauri-apps/api/core", () => ({ invoke, isTauri: () => true }))

describe("refreshLiveUsage", () => {
  beforeEach(() => {
    invoke.mockReset()
  })

  it.each(["settings change", "period reset"])(
    "requests a new check after a %s while an earlier check runs",
    async () => {
      let finish: (value: typeof EMPTY_LIVE_USAGE) => void = () => undefined
      const before = { ...EMPTY_LIVE_USAGE, generatedAt: "2026-10-30T12:00:00Z" }
      const after = { ...EMPTY_LIVE_USAGE, generatedAt: "2026-10-30T12:01:00Z" }
      invoke
        .mockReturnValueOnce(new Promise((resolve) => (finish = resolve)))
        .mockResolvedValueOnce(after)

      const first = refreshLiveUsage()
      const second = refreshLiveUsage()

      expect(invoke).toHaveBeenCalledTimes(2)
      await expect(second).resolves.toEqual(after)
      finish(before)
      await expect(first).resolves.toEqual(before)
    },
  )

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
