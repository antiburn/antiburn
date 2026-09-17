import { act, renderHook, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import { DEFAULT_SETTINGS, type AppSettings } from "../../lib/ipc"
import type * as ipc from "../../lib/ipc"
import { saveInterfaceScale, useAppSettings } from "./useAppSettings"

const mocks = vi.hoisted(() => ({
  save: vi.fn(),
  change: null as ((settings: AppSettings) => void) | null,
}))

vi.mock("../../lib/ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof ipc>()
  return {
    ...actual,
    getSettings: async () => actual.DEFAULT_SETTINGS,
    onSettingsChanged: async (callback: (settings: AppSettings) => void) => {
      mocks.change = callback
      return () => {
        mocks.change = null
      }
    },
    setInterfaceScale: mocks.save,
  }
})

describe("settings interface scale boundary", () => {
  beforeEach(() => mocks.save.mockReset())

  it("publishes only confirmed saves and preserves the old preference on failure", async () => {
    const { result } = renderHook(useAppSettings)
    await waitFor(() => expect(mocks.change).not.toBeNull())
    const saved = { ...DEFAULT_SETTINGS, interfaceScalePercent: 150 }
    mocks.save.mockResolvedValueOnce(saved)
    await act(() => saveInterfaceScale({ kind: "set", percent: 150 }))
    expect(mocks.save).toHaveBeenCalledWith({ kind: "set", percent: 150 }, "settings")
    expect(result.current.settings.interfaceScalePercent).toBe(150)
    mocks.save.mockRejectedValueOnce(new Error("storage failed"))
    await act(async () => {
      await expect(saveInterfaceScale({ kind: "increase" })).rejects.toThrow("storage failed")
    })
    expect(result.current.settings.interfaceScalePercent).toBe(150)
  })

  it("does not replace a newer cross-window broadcast with an older command response", async () => {
    const { result } = renderHook(useAppSettings)
    await waitFor(() => expect(mocks.change).not.toBeNull())
    let resolve!: (settings: AppSettings) => void
    mocks.save.mockReturnValueOnce(
      new Promise<AppSettings>((done) => {
        resolve = done
      }),
    )
    let pending!: Promise<void>
    act(() => {
      pending = saveInterfaceScale({ kind: "increase" })
    })
    act(() => mocks.change?.({ ...DEFAULT_SETTINGS, interfaceScalePercent: 200 }))
    await act(async () => {
      resolve({ ...DEFAULT_SETTINGS, interfaceScalePercent: 110 })
      await pending
    })
    expect(result.current.settings.interfaceScalePercent).toBe(200)
  })
})
