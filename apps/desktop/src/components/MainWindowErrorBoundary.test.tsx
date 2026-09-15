import type * as IpcModule from "../lib/ipc"
import { act, StrictMode } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { MainWindowErrorBoundary } from "./MainWindowErrorBoundary"
import { WindowReadyBoundary } from "./WindowReadyMarker"
import { rendererHealth, resetRendererHealthForTest } from "../lib/rendererHealth"

const mocks = vi.hoisted(() => ({
  status: vi.fn(),
  failure: vi.fn(),
  recovery: vi.fn(),
}))

vi.mock("../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof IpcModule>()),
  reportMainWindowRenderStatus: mocks.status,
  reportMainWindowRenderFailure: mocks.failure,
  requestMainWindowRecovery: mocks.recovery,
}))

let container: HTMLDivElement
let root: Root

beforeEach(() => {
  vi.clearAllMocks()
  resetRendererHealthForTest()
  mocks.status.mockResolvedValue(undefined)
  mocks.failure.mockResolvedValue(undefined)
  mocks.recovery.mockResolvedValue(undefined)
  Object.defineProperty(window, "__ANTIBURN_WINDOW_GENERATION__", {
    value: 12,
    configurable: true,
  })
  container = document.createElement("div")
  document.body.append(container)
  root = createRoot(container)
})

afterEach(async () => {
  await act(() => root.unmount())
  container.remove()
})

describe("MainWindowErrorBoundary", () => {
  it("reports one healthy commit under StrictMode", async () => {
    await act(() =>
      root.render(
        <StrictMode>
          <MainWindowErrorBoundary>
            <p>Application</p>
          </MainWindowErrorBoundary>
        </StrictMode>,
      ),
    )
    expect(container.textContent).toContain("Application")
    expect(mocks.status).toHaveBeenCalledTimes(1)
    expect(mocks.status).toHaveBeenCalledWith(12, "healthy")
  })

  it("commits its fallback inside the native readiness boundary", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => undefined)
    const reporter = vi.fn().mockResolvedValue(undefined)
    function Broken(): never {
      throw new Error("failed")
    }
    await act(() =>
      root.render(
        <WindowReadyBoundary reporter={reporter}>
          <MainWindowErrorBoundary>
            <Broken />
          </MainWindowErrorBoundary>
        </WindowReadyBoundary>,
      ),
    )
    expect(container.textContent).toContain("The main window could not load")
    expect(reporter).toHaveBeenCalledWith(12)
    consoleError.mockRestore()
  })

  it("does not report healthy when a descendant ref throws during commit", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => undefined)
    function BrokenCommit() {
      return (
        <span
          ref={(node) => {
            if (node) throw new Error("commit failed")
          }}
        >
          Application
        </span>
      )
    }
    await act(() =>
      root.render(
        <MainWindowErrorBoundary>
          <BrokenCommit />
        </MainWindowErrorBoundary>,
      ),
    )
    expect(container.textContent).toContain("The main window could not load")
    expect(rendererHealth()).toBe("fallback")
    expect(mocks.status).toHaveBeenCalledWith(12, "fallback")
    expect(mocks.status).not.toHaveBeenCalledWith(12, "healthy")
    consoleError.mockRestore()
  })

  it("commits a revealable fallback and starts generation-scoped recovery", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => undefined)
    function Broken(): never {
      throw new TypeError("private path")
    }
    await act(() =>
      root.render(
        <MainWindowErrorBoundary>
          <Broken />
        </MainWindowErrorBoundary>,
      ),
    )
    expect(container.textContent).toContain("The main window could not load")
    expect(mocks.status).toHaveBeenCalledWith(12, "fallback")
    expect(mocks.failure).toHaveBeenCalledWith(12, {
      kind: "render_fallback",
      category: "type_error",
      errorName: "TypeError",
    })

    await act(() => container.querySelector("button")?.click())
    expect(mocks.recovery).toHaveBeenCalledWith(12)
    consoleError.mockRestore()
  })
})
