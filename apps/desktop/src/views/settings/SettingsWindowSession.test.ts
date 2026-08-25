// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { beforeEach, describe, expect, it, vi } from "vitest"

import { SettingsWindowSession } from "./SettingsWindowSession"

const appInfo = vi.hoisted(() => vi.fn())
const onSettingsPaneRequest = vi.hoisted(() => vi.fn())
const takeSettingsPane = vi.hoisted(() => vi.fn())

vi.mock("../../lib/ipc", () => ({ appInfo, onSettingsPaneRequest, takeSettingsPane }))

describe("SettingsWindowSession", () => {
  beforeEach(() => {
    appInfo.mockReset()
    appInfo.mockResolvedValue(null)
    onSettingsPaneRequest.mockReset()
    takeSettingsPane.mockReset()
  })

  it("starts listening before it takes the pending pane", async () => {
    const order: string[] = []
    onSettingsPaneRequest.mockImplementation(async () => {
      order.push("listen")
      return () => {}
    })
    takeSettingsPane.mockImplementation(async () => {
      order.push("take")
      return "sources"
    })
    const session = new SettingsWindowSession()
    const unsubscribe = session.subscribe(() => {})

    await vi.waitFor(() => expect(session.getSnapshot().pane).toBe("sources"))

    expect(order).toEqual(["listen", "take"])
    unsubscribe()
  })

  it("clears the pending fallback after an event delivers its pane", async () => {
    const delivery: { current: ((pane: string) => void) | null } = { current: null }
    onSettingsPaneRequest.mockImplementation(async (handler: (pane: string) => void) => {
      delivery.current = handler
      return () => {}
    })
    takeSettingsPane.mockResolvedValue(null)
    const session = new SettingsWindowSession()
    const unsubscribe = session.subscribe(() => {})
    await vi.waitFor(() => expect(takeSettingsPane).toHaveBeenCalledTimes(1))
    takeSettingsPane.mockClear()

    delivery.current?.("sources")

    await vi.waitFor(() => expect(session.getSnapshot().pane).toBe("sources"))
    expect(takeSettingsPane).toHaveBeenCalledTimes(1)
    unsubscribe()
  })

  it("keeps a newer event when the pending fallback resolves later", async () => {
    const delivery: { current: ((pane: string) => void) | null } = { current: null }
    let resolvePending: (pane: string) => void = () => {}
    onSettingsPaneRequest.mockImplementation(async (handler: (pane: string) => void) => {
      delivery.current = handler
      return () => {}
    })
    takeSettingsPane.mockImplementationOnce(
      () =>
        new Promise<string>((resolve) => {
          resolvePending = resolve
        }),
    )
    takeSettingsPane.mockResolvedValueOnce(null)
    const session = new SettingsWindowSession()
    const unsubscribe = session.subscribe(() => {})
    await vi.waitFor(() => expect(takeSettingsPane).toHaveBeenCalledTimes(1))

    delivery.current?.("usage")
    resolvePending("sources")

    await vi.waitFor(() => expect(session.getSnapshot().pane).toBe("usage"))
    unsubscribe()
  })

  it("uses the pending pane when listener registration fails", async () => {
    onSettingsPaneRequest.mockRejectedValue(new Error("listener unavailable"))
    takeSettingsPane.mockResolvedValue("sources")
    const session = new SettingsWindowSession()
    const unsubscribe = session.subscribe(() => {})

    await vi.waitFor(() => expect(session.getSnapshot().pane).toBe("sources"))

    expect(takeSettingsPane).toHaveBeenCalledTimes(1)
    unsubscribe()
  })
})
