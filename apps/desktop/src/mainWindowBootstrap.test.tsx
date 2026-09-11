import type { ReactNode } from "react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { mountMainWindow } from "./mainWindowBootstrap"

const mocks = vi.hoisted(() => ({
  mountWindow: vi.fn(),
  installDiagnostics: vi.fn(),
  reportFailure: vi.fn(),
  installResponder: vi.fn(),
  getSettings: vi.fn(),
  onSettingsChanged: vi.fn(),
  disposeDiagnostics: vi.fn(),
  disposeResponder: vi.fn(),
  unlistenSettings: vi.fn(),
}))

vi.mock("./bootstrap", () => ({ mountWindow: mocks.mountWindow }))
vi.mock("./components/MainWindowErrorBoundary", () => ({
  MainWindowErrorBoundary: ({ children }: { children: ReactNode }) => children,
}))
vi.mock("./lib/bootstrapDiagnostics", () => ({
  installBootstrapDiagnostics: mocks.installDiagnostics,
  reportBootstrapFailure: mocks.reportFailure,
}))
vi.mock("./lib/mainWindowHealth", () => ({
  installMainWindowHealthResponder: mocks.installResponder,
}))
vi.mock("./lib/ipc", () => ({
  getSettings: mocks.getSettings,
  mainWindowReady: vi.fn(),
  onSettingsChanged: mocks.onSettingsChanged,
}))
vi.mock("./lib/appearance", () => ({ applyTheme: vi.fn() }))
vi.mock("./views/MainWindowView", () => ({ MainWindowView: () => null }))

beforeEach(() => {
  vi.clearAllMocks()
  mocks.installDiagnostics.mockReturnValue(mocks.disposeDiagnostics)
  mocks.installResponder.mockResolvedValue(mocks.disposeResponder)
  mocks.onSettingsChanged.mockResolvedValue(mocks.unlistenSettings)
  mocks.getSettings.mockResolvedValue(null)
})

afterEach(() => {
  window.dispatchEvent(new Event("pagehide"))
  vi.useRealTimers()
})

describe("mountMainWindow", () => {
  it("installs diagnostics and the health responder before mounting", async () => {
    await mountMainWindow()
    expect(mocks.installDiagnostics).toHaveBeenCalledOnce()
    expect(mocks.installResponder).toHaveBeenCalledOnce()
    expect(mocks.mountWindow).toHaveBeenCalledOnce()
  })

  it("mounts after responder installation rejects", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => undefined)
    mocks.installResponder.mockRejectedValue(new Error("unavailable"))
    await mountMainWindow()
    expect(mocks.reportFailure).toHaveBeenCalledWith(
      "responder_install_failed",
      expect.any(Error),
    )
    expect(mocks.mountWindow).toHaveBeenCalledOnce()
    consoleError.mockRestore()
  })

  it("mounts after the bounded responder deadline", async () => {
    vi.useFakeTimers()
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => undefined)
    mocks.installResponder.mockReturnValue(new Promise(() => undefined))
    const mounted = mountMainWindow()
    await vi.advanceTimersByTimeAsync(1_000)
    await mounted
    expect(mocks.reportFailure).toHaveBeenCalledWith(
      "responder_install_failed",
      expect.any(Error),
    )
    expect(mocks.mountWindow).toHaveBeenCalledOnce()
    consoleError.mockRestore()
  })
})
