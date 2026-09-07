import { fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it } from "vitest"

import { MainWindowView } from "./MainWindowView"

const userAgent = Object.getOwnPropertyDescriptor(window.navigator, "userAgent")
const innerWidth = Object.getOwnPropertyDescriptor(window, "innerWidth")

function setUserAgent(value: string): void {
  Object.defineProperty(window.navigator, "userAgent", { configurable: true, value })
}

function setWindowWidth(value: number): void {
  Object.defineProperty(window, "innerWidth", { configurable: true, value })
}

afterEach(() => {
  if (userAgent) Object.defineProperty(window.navigator, "userAgent", userAgent)
  if (innerWidth) Object.defineProperty(window, "innerWidth", innerWidth)
})

describe("MainWindowView", () => {
  it("keeps the macOS title-bar clearance as the drag region", () => {
    setUserAgent("Mozilla/5.0 (Macintosh; Intel Mac OS X)")
    setWindowWidth(900)

    const { container } = render(<MainWindowView />)

    expect(screen.getByRole("main", { name: "antiburn main window" })).toHaveClass(
      "main-window",
    )
    expect(container.querySelector("[data-tauri-drag-region]")).toHaveClass(
      "main-window-titlebar",
    )
  })

  it("keeps native title bars clear on other platforms", () => {
    setUserAgent("Mozilla/5.0 (Windows NT 10.0)")
    setWindowWidth(900)

    const { container } = render(<MainWindowView />)

    expect(container.querySelector("[data-tauri-drag-region]")).toBeNull()
  })

  it("shows only Activity in the persistent sidebar", () => {
    setWindowWidth(800)
    render(<MainWindowView />)
    expect(screen.getAllByRole("tab")).toHaveLength(1)
    expect(screen.getByRole("tab", { name: "Activity" })).toHaveAttribute(
      "aria-selected",
      "true",
    )
    expect(screen.getByRole("tabpanel", { name: "Activity" })).toBeVisible()
    expect(screen.queryByRole("button", { name: "Settings" })).toBeNull()
    setWindowWidth(900)
    fireEvent(window, new Event("resize"))
    expect(screen.getByRole("tablist", { name: "Main sections" })).toBeVisible()
    expect(screen.queryByRole("button", { name: "Open navigation" })).toBeNull()
  })
})
