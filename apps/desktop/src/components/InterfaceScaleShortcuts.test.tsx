import { StrictMode } from "react"
import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import { InterfaceScaleShortcuts } from "./InterfaceScaleShortcuts"

const mocks = vi.hoisted(() => ({ shell: true, macOS: false, save: vi.fn() }))
vi.mock("../lib/ipc", () => ({
  hasShell: () => mocks.shell,
  setInterfaceScale: mocks.save,
}))
vi.mock("../lib/platform", () => ({ isMacOS: () => mocks.macOS }))

describe("interface scale shortcut boundary", () => {
  beforeEach(() => {
    mocks.shell = true
    mocks.macOS = false
    mocks.save.mockReset().mockResolvedValue(undefined)
  })

  it("owns each Windows/Linux shortcut once and releases the listener on unmount", () => {
    const { unmount } = render(
      <StrictMode>
        <InterfaceScaleShortcuts />
      </StrictMode>,
    )
    const event = new KeyboardEvent("keydown", { key: "=", ctrlKey: true, cancelable: true })
    fireEvent(document, event)
    expect(event.defaultPrevented).toBe(true)
    expect(mocks.save).toHaveBeenCalledExactlyOnceWith({ kind: "increase" }, "shortcut")
    unmount()
    fireEvent.keyDown(document, { key: "-", ctrlKey: true })
    expect(mocks.save).toHaveBeenCalledTimes(1)
  })

  it.each(["macOS", "browser"])("leaves %s shortcuts to their owner", (owner) => {
    mocks.macOS = owner === "macOS"
    mocks.shell = owner !== "browser"
    render(<InterfaceScaleShortcuts />)
    const event = new KeyboardEvent("keydown", {
      key: "+",
      ctrlKey: true,
      metaKey: mocks.macOS,
      cancelable: true,
    })
    fireEvent(document, event)
    expect(event.defaultPrevented).toBe(false)
    expect(mocks.save).not.toHaveBeenCalled()
  })

  it("does not consume a shortcut already handled by a nested control", () => {
    render(<InterfaceScaleShortcuts />)
    const event = new KeyboardEvent("keydown", { key: "0", ctrlKey: true, cancelable: true })
    event.preventDefault()
    fireEvent(document, event)
    expect(mocks.save).not.toHaveBeenCalled()
  })

  it("exposes a failed save and clears the error after a successful retry", async () => {
    mocks.save.mockRejectedValueOnce(new Error("save failed"))
    render(<InterfaceScaleShortcuts />)
    fireEvent.keyDown(document, { key: "+", ctrlKey: true })
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Could not change interface size",
    )
    fireEvent.keyDown(document, { key: "+", ctrlKey: true })
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull())
    expect(mocks.save).toHaveBeenCalledTimes(2)
  })
})
