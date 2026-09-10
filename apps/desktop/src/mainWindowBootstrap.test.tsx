import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { MainWindowThemeSession } from "./mainWindowBootstrap"

const getSettings = vi.hoisted(() => vi.fn())
const onSettingsChanged = vi.hoisted(() => vi.fn())
const applyTheme = vi.hoisted(() => vi.fn())
const visibilityState = Object.getOwnPropertyDescriptor(document, "visibilityState")
type ThemeListener = (settings: { theme: "system" | "light" | "dark" }) => void

vi.mock("./lib/ipc", () => {
  return { getSettings, mainWindowReady: vi.fn(), onSettingsChanged, windowReady: vi.fn() }
})
vi.mock("./lib/appearance", () => ({ applyTheme }))

describe("MainWindowThemeSession", () => {
  beforeEach(() => {
    getSettings.mockReset()
    onSettingsChanged.mockReset()
    applyTheme.mockReset()
    onSettingsChanged.mockImplementation(async () => () => {})
  })

  afterEach(() => {
    if (visibilityState) Object.defineProperty(document, "visibilityState", visibilityState)
    else Reflect.deleteProperty(document, "visibilityState")
  })

  it("sets the stored theme before the main window mounts", async () => {
    getSettings.mockResolvedValue({ theme: "dark" })

    await new MainWindowThemeSession().start()

    expect(applyTheme).toHaveBeenCalledWith("dark")
  })

  it.each(["visible", "hidden"] as const)(
    "applies settings changes while the retained window is %s",
    async (visibility) => {
      const subscription: { listener: ThemeListener | null } = { listener: null }
      onSettingsChanged.mockImplementation(async (next) => {
        subscription.listener = next
        return () => {}
      })
      getSettings.mockResolvedValue({ theme: "light" })
      Object.defineProperty(document, "visibilityState", {
        configurable: true,
        value: visibility,
      })

      await new MainWindowThemeSession().start()
      subscription.listener?.({ theme: "dark" })

      expect(applyTheme).toHaveBeenLastCalledWith("dark")
    },
  )

  it("keeps a settings event when a stale initial snapshot arrives later", async () => {
    const subscription: { listener: ThemeListener | null } = { listener: null }
    let resolveSettings: (settings: { theme: "light" }) => void
    getSettings.mockImplementation(
      () => new Promise<{ theme: "light" }>((resolve) => (resolveSettings = resolve)),
    )
    onSettingsChanged.mockImplementation(async (next) => {
      subscription.listener = next
      return () => {}
    })
    const session = new MainWindowThemeSession()
    const start = session.start()

    await vi.waitFor(() => expect(subscription.listener).not.toBeNull())
    subscription.listener?.({ theme: "dark" })
    resolveSettings!({ theme: "light" })
    await start

    expect(applyTheme).toHaveBeenCalledTimes(1)
    expect(applyTheme).toHaveBeenCalledWith("dark")
  })

  it("registers one listener when startup runs again", async () => {
    getSettings.mockResolvedValue({ theme: "light" })
    const session = new MainWindowThemeSession()

    await Promise.all([session.start(), session.start()])

    expect(onSettingsChanged).toHaveBeenCalledTimes(1)
  })

  it("stops applying changes after page teardown", async () => {
    const unlisten = vi.fn()
    const subscription: { listener: ThemeListener | null } = { listener: null }
    onSettingsChanged.mockImplementation(async (next) => {
      subscription.listener = next
      return unlisten
    })
    getSettings.mockResolvedValue({ theme: "light" })
    const session = new MainWindowThemeSession()

    await session.start()
    session.dispose()
    subscription.listener?.({ theme: "dark" })

    expect(unlisten).toHaveBeenCalledOnce()
    expect(applyTheme).toHaveBeenCalledTimes(1)
    expect(applyTheme).toHaveBeenCalledWith("light")
  })

  it("keeps the system theme when settings are unavailable", async () => {
    getSettings.mockRejectedValue(new Error("shell unavailable"))

    await new MainWindowThemeSession().start()

    expect(applyTheme).not.toHaveBeenCalled()
  })
})
