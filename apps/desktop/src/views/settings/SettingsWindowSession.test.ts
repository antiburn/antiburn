import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { CloudSession } from "../../onboarding/CloudSession"
import { cloudProjectFixture } from "../../onboarding/__tests__/cloudProjectFixtures"
import { SettingsWindowSession } from "./SettingsWindowSession"

const appInfo = vi.hoisted(() => vi.fn())
const onSessionIndexChanged = vi.hoisted(() => vi.fn())
const onSettingsPaneRequest = vi.hoisted(() => vi.fn())
const onSettingsShown = vi.hoisted(() => vi.fn())
const noteInteraction = vi.hoisted(() => vi.fn())
const takeSettingsPane = vi.hoisted(() => vi.fn())
const isVisible = vi.hoisted(() => vi.fn())
const shell = vi.hoisted(() => ({ present: true }))

vi.mock("../../lib/ipc", () => ({
  appInfo,
  hasShell: () => shell.present,
  onSessionIndexChanged,
  onSettingsPaneRequest,
  onSettingsShown,
  noteInteraction,
  takeSettingsPane,
}))

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ isVisible }),
}))

describe("SettingsWindowSession", () => {
  beforeEach(() => {
    appInfo.mockReset()
    appInfo.mockResolvedValue(null)
    onSessionIndexChanged.mockReset()
    onSessionIndexChanged.mockResolvedValue(() => {})
    onSettingsPaneRequest.mockReset()
    onSettingsPaneRequest.mockResolvedValue(() => {})
    onSettingsShown.mockReset()
    onSettingsShown.mockResolvedValue(() => {})
    noteInteraction.mockReset()
    takeSettingsPane.mockReset()
    isVisible.mockReset()
    isVisible.mockResolvedValue(true)
    shell.present = true
  })

  afterEach(() => {
    delete (document as unknown as Record<string, unknown>)["visibilityState"]
  })

  it("starts listening before it takes the pending pane", async () => {
    const order: string[] = []
    onSettingsPaneRequest.mockImplementation(async () => {
      order.push("listen")
      return () => {}
    })
    takeSettingsPane.mockImplementation(async () => {
      order.push("take")
      return "privacy"
    })
    const session = new SettingsWindowSession()
    const unsubscribe = session.subscribe(() => {})

    await vi.waitFor(() => expect(session.getSnapshot().pane).toBe("privacy"))

    expect(order).toEqual(["listen", "take"])
    expect(noteInteraction).toHaveBeenCalledOnce()
    expect(noteInteraction).toHaveBeenCalledWith({
      kind: "settingsPaneViewed",
      pane: "privacy",
    })
    unsubscribe()
  })

  it("does not report transient General before the pending pane resolves", async () => {
    let resolvePending: (pane: string) => void = () => {}
    onSettingsPaneRequest.mockResolvedValue(() => {})
    takeSettingsPane.mockImplementation(
      () =>
        new Promise<string>((resolve) => {
          resolvePending = resolve
        }),
    )
    const session = new SettingsWindowSession()
    const unsubscribe = session.subscribe(() => {})
    await vi.waitFor(() => expect(takeSettingsPane).toHaveBeenCalledOnce())

    expect(noteInteraction).not.toHaveBeenCalled()
    resolvePending("privacy")

    await vi.waitFor(() =>
      expect(noteInteraction).toHaveBeenCalledWith({
        kind: "settingsPaneViewed",
        pane: "privacy",
      }),
    )
    expect(noteInteraction).not.toHaveBeenCalledWith({
      kind: "settingsPaneViewed",
      pane: "general",
    })
    unsubscribe()
  })

  it("defers the selected pane until the native window is visible", async () => {
    const shown: { current: (() => void) | null } = { current: null }
    isVisible.mockResolvedValue(false)
    onSettingsShown.mockImplementation(async (handler: () => void) => {
      shown.current = handler
      return () => {}
    })
    takeSettingsPane.mockResolvedValue("privacy")
    const session = new SettingsWindowSession()
    const unsubscribe = session.subscribe(() => {})
    await vi.waitFor(() => expect(session.getSnapshot().pane).toBe("privacy"))

    expect(session.getSnapshot().visible).toBe(false)
    expect(noteInteraction).not.toHaveBeenCalled()
    shown.current?.()

    expect(session.getSnapshot().visible).toBe(true)
    expect(noteInteraction).toHaveBeenCalledWith({
      kind: "settingsPaneViewed",
      pane: "privacy",
    })
    unsubscribe()
  })

  it("preloads Cloud folders once the settings window is visible", async () => {
    appInfo.mockResolvedValue({ cloudEnabled: true })
    isVisible.mockResolvedValue(false)
    takeSettingsPane.mockResolvedValue("sessions")
    const { port, ongoing } = cloudProjectFixture()
    const folders = new CloudSession(
      { ...port, watchWorkspace: async () => () => {} },
      false,
      true,
      "automatic",
    )
    const session = new SettingsWindowSession(folders)
    const unsubscribe = session.subscribe(() => {})
    const stopPane = folders.subscribeSnapshot(() => {})

    try {
      await vi.waitFor(() => expect(session.getSnapshot().pane).toBe("sessions"))
      expect(port.read).not.toHaveBeenCalled()
      session.setPane("general")
      const setVisibility = (value: DocumentVisibilityState) => {
        Object.defineProperty(document, "visibilityState", { value, configurable: true })
        document.dispatchEvent(new Event("visibilitychange"))
      }
      setVisibility("visible")
      await vi.waitFor(() => expect(folders.getSnapshot().projects?.loaded).toBe(true))
      expect(session.getSnapshot().pane).toBe("general")
      expect(ongoing.groups).toHaveBeenCalledOnce()
      session.setPane("sessions")

      setVisibility("hidden")
      expect(session.getSnapshot().visible).toBe(false)
      setVisibility("visible")
      await vi.waitFor(() => expect(ongoing.groups).toHaveBeenCalledTimes(2))
      expect(ongoing.enable).not.toHaveBeenCalled()
    } finally {
      stopPane()
      unsubscribe()
    }
  })

  it("does not resume Cloud preload before checking visibility after resubscribe", async () => {
    appInfo.mockResolvedValue({ cloudEnabled: true })
    takeSettingsPane.mockResolvedValue(null)
    const shown: { current: (() => void) | null } = { current: null }
    onSettingsShown.mockImplementation(async (handler: () => void) => {
      shown.current = handler
      return () => {}
    })
    const { port } = cloudProjectFixture()
    const folders = new CloudSession(
      { ...port, watchWorkspace: async () => () => {} },
      false,
      true,
      "automatic",
    )
    const session = new SettingsWindowSession(folders)
    const unsubscribeFirst = session.subscribe(() => {})
    try {
      await vi.waitFor(() => expect(port.read).toHaveBeenCalledOnce())
      expect(session.getSnapshot().visible).toBe(true)
    } finally {
      unsubscribeFirst()
    }
    expect(session.getSnapshot().visible).toBe(false)

    isVisible.mockResolvedValue(false)
    const unsubscribeSecond = session.subscribe(() => {})
    try {
      await vi.waitFor(() => expect(takeSettingsPane).toHaveBeenCalledTimes(2))
      expect(session.getSnapshot().visible).toBe(false)
      expect(port.read).toHaveBeenCalledOnce()
      expect(noteInteraction).toHaveBeenCalledOnce()

      shown.current?.()
      await vi.waitFor(() => expect(port.read).toHaveBeenCalledTimes(2))
      expect(noteInteraction).toHaveBeenCalledTimes(2)
    } finally {
      unsubscribeSecond()
    }
  })

  it("reports the current pane again after a hidden window is shown", async () => {
    const shown: { current: (() => void) | null } = { current: null }
    onSettingsShown.mockImplementation(async (handler: () => void) => {
      shown.current = handler
      return () => {}
    })
    takeSettingsPane.mockResolvedValue(null)
    const session = new SettingsWindowSession()
    const unsubscribe = session.subscribe(() => {})
    await vi.waitFor(() => expect(noteInteraction).toHaveBeenCalledOnce())

    Object.defineProperty(document, "visibilityState", {
      value: "hidden",
      configurable: true,
    })
    document.dispatchEvent(new Event("visibilitychange"))
    shown.current?.()

    expect(noteInteraction).toHaveBeenCalledTimes(2)
    expect(noteInteraction).toHaveBeenLastCalledWith({
      kind: "settingsPaneViewed",
      pane: "general",
    })
    unsubscribe()
  })

  it("reports General after the shell confirms there is no pending pane", async () => {
    onSettingsPaneRequest.mockResolvedValue(() => {})
    takeSettingsPane.mockResolvedValue(null)
    const session = new SettingsWindowSession()
    const unsubscribe = session.subscribe(() => {})

    await vi.waitFor(() =>
      expect(noteInteraction).toHaveBeenCalledWith({
        kind: "settingsPaneViewed",
        pane: "general",
      }),
    )
    unsubscribe()
  })

  it("reports each visible pane transition and suppresses duplicate requests", async () => {
    onSettingsPaneRequest.mockResolvedValue(() => {})
    takeSettingsPane.mockResolvedValue(null)
    const session = new SettingsWindowSession()
    const unsubscribe = session.subscribe(() => {})
    await vi.waitFor(() =>
      expect(noteInteraction).toHaveBeenCalledWith({
        kind: "settingsPaneViewed",
        pane: "general",
      }),
    )

    session.setPane("usage")
    session.setPane("usage")
    session.setPane("general")

    expect(noteInteraction.mock.calls).toEqual([
      [{ kind: "settingsPaneViewed", pane: "general" }],
      [{ kind: "settingsPaneViewed", pane: "usage" }],
      [{ kind: "settingsPaneViewed", pane: "general" }],
    ])
    unsubscribe()
  })

  it("does not report a new view when the same session resubscribes", async () => {
    onSettingsPaneRequest.mockResolvedValue(() => {})
    takeSettingsPane.mockResolvedValue(null)
    const session = new SettingsWindowSession()
    const unsubscribeFirst = session.subscribe(() => {})
    await vi.waitFor(() =>
      expect(noteInteraction).toHaveBeenCalledWith({
        kind: "settingsPaneViewed",
        pane: "general",
      }),
    )
    unsubscribeFirst()

    const unsubscribeSecond = session.subscribe(() => {})
    await vi.waitFor(() => expect(takeSettingsPane).toHaveBeenCalledTimes(2))

    expect(noteInteraction).toHaveBeenCalledOnce()
    unsubscribeSecond()
  })

  it("keeps browser-mode settings visible without a native window", async () => {
    shell.present = false
    takeSettingsPane.mockResolvedValue(null)
    const session = new SettingsWindowSession()
    const unsubscribe = session.subscribe(() => {})

    await vi.waitFor(() => expect(session.getSnapshot().visible).toBe(true))
    expect(noteInteraction).toHaveBeenCalledWith({
      kind: "settingsPaneViewed",
      pane: "general",
    })
    expect(isVisible).not.toHaveBeenCalled()
    unsubscribe()
  })

  it("keeps pane setup running when the native visibility API throws", async () => {
    isVisible.mockImplementation(() => {
      throw new Error("missing window metadata")
    })
    takeSettingsPane.mockResolvedValue("privacy")
    const session = new SettingsWindowSession()
    const unsubscribe = session.subscribe(() => {})

    await vi.waitFor(() => expect(session.getSnapshot().pane).toBe("privacy"))
    expect(noteInteraction).not.toHaveBeenCalled()
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

    delivery.current?.("privacy")

    await vi.waitFor(() => expect(session.getSnapshot().pane).toBe("privacy"))
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
    resolvePending("privacy")

    await vi.waitFor(() => expect(session.getSnapshot().pane).toBe("usage"))
    unsubscribe()
  })

  it("keeps a user pane selection when the pending fallback resolves later", async () => {
    let resolvePending: (pane: string) => void = () => {}
    takeSettingsPane.mockImplementation(
      () =>
        new Promise<string>((resolve) => {
          resolvePending = resolve
        }),
    )
    const session = new SettingsWindowSession()
    const unsubscribe = session.subscribe(() => {})
    await vi.waitFor(() => expect(takeSettingsPane).toHaveBeenCalledOnce())

    session.setPane("usage")
    resolvePending("privacy")

    await vi.waitFor(() => expect(session.getSnapshot().pane).toBe("usage"))
    expect(noteInteraction).toHaveBeenLastCalledWith({
      kind: "settingsPaneViewed",
      pane: "usage",
    })
    unsubscribe()
  })

  it("uses visible General when the pending-pane read fails", async () => {
    takeSettingsPane.mockRejectedValue(new Error("pane unavailable"))
    const session = new SettingsWindowSession()
    const unsubscribe = session.subscribe(() => {})

    await vi.waitFor(() =>
      expect(noteInteraction).toHaveBeenCalledWith({
        kind: "settingsPaneViewed",
        pane: "general",
      }),
    )
    unsubscribe()
  })

  it("uses the pending pane when listener registration fails", async () => {
    onSettingsPaneRequest.mockRejectedValue(new Error("listener unavailable"))
    takeSettingsPane.mockResolvedValue("privacy")
    const session = new SettingsWindowSession()
    const unsubscribe = session.subscribe(() => {})

    await vi.waitFor(() => expect(session.getSnapshot().pane).toBe("privacy"))

    expect(takeSettingsPane).toHaveBeenCalledTimes(1)
    unsubscribe()
  })

  it("refreshes app info after the session index changes", async () => {
    const invalidation: { current: (() => void) | null } = { current: null }
    onSessionIndexChanged.mockImplementation(async (handler: () => void) => {
      invalidation.current = handler
      return () => {}
    })
    onSettingsPaneRequest.mockResolvedValue(() => {})
    takeSettingsPane.mockResolvedValue(null)
    appInfo
      .mockResolvedValueOnce({ indexedSessions: 42 })
      .mockResolvedValueOnce({ indexedSessions: 7 })
    const session = new SettingsWindowSession()
    const unsubscribe = session.subscribe(() => {})
    await vi.waitFor(() => expect(session.getSnapshot().info?.indexedSessions).toBe(42))

    invalidation.current?.()

    await vi.waitFor(() => expect(session.getSnapshot().info?.indexedSessions).toBe(7))
    expect(appInfo).toHaveBeenCalledTimes(2)
    unsubscribe()
  })
})
