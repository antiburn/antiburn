import { StrictMode } from "react"

import { render, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import { WindowReadyBoundary } from "./WindowReadyMarker"

const windowReady = vi.hoisted(() => vi.fn(async () => {}))
vi.mock("../lib/ipc", () => ({ windowReady }))

describe("WindowReadyMarker", () => {
  beforeEach(() => {
    windowReady.mockReset()
    windowReady.mockResolvedValue(undefined)
    Object.defineProperty(window, "__ANTIBURN_WINDOW_GENERATION__", {
      configurable: true,
      value: 7,
    })
  })

  it("reports readiness after the view callback refs finish", () => {
    const order: string[] = []
    windowReady.mockImplementation(async () => {
      order.push("ready")
    })

    render(
      <StrictMode>
        <WindowReadyBoundary>
          <div
            ref={(node) => {
              if (node) order.push("view")
            }}
          />
        </WindowReadyBoundary>
      </StrictMode>,
    )

    expect(order).toEqual(["view", "ready", "view", "ready"])
    expect(windowReady).toHaveBeenNthCalledWith(1, 7)
    expect(windowReady).toHaveBeenNthCalledWith(2, 7)
    expect(document.querySelector("[data-window-ready-marker]")).not.toBeNull()
  })

  it("handles a readiness command failure", async () => {
    windowReady.mockRejectedValue(new Error("window closed"))

    render(
      <WindowReadyBoundary>
        <div />
      </WindowReadyBoundary>,
    )

    await waitFor(() => expect(windowReady).toHaveBeenCalledTimes(1))
  })

  it("does not report readiness without a native generation", () => {
    Reflect.deleteProperty(window, "__ANTIBURN_WINDOW_GENERATION__")

    render(
      <WindowReadyBoundary>
        <div />
      </WindowReadyBoundary>,
    )

    expect(windowReady).not.toHaveBeenCalled()
  })
})
