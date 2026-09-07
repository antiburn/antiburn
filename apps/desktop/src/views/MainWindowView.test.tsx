import { fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { Activity } from "lucide-react"
import { CollectionDetailPane } from "./main-window/CollectionDetailPane"
import type * as IpcModule from "../lib/ipc"
import capability from "../../src-tauri/capabilities/main.json"
import { openSettingsWindow } from "../lib/ipc"
import { MainWindowView } from "./MainWindowView"

vi.mock("../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof IpcModule>()),
  openSettingsWindow: vi.fn().mockResolvedValue(undefined),
}))

const userAgent = Object.getOwnPropertyDescriptor(window.navigator, "userAgent")
const innerWidth = Object.getOwnPropertyDescriptor(window, "innerWidth")

function setUserAgent(value: string): void {
  Object.defineProperty(window.navigator, "userAgent", { configurable: true, value })
}

function setWindowWidth(value: number): void {
  Object.defineProperty(window, "innerWidth", { configurable: true, value })
}

afterEach(() => {
  vi.clearAllMocks()
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
    setWindowWidth(1000)
    render(<MainWindowView />)
    expect(screen.getAllByRole("tab")).toHaveLength(1)
    expect(screen.getByRole("tab", { name: "Activity" })).toHaveAttribute(
      "aria-selected",
      "true",
    )
    expect(screen.getByRole("tabpanel", { name: "Activity" })).toBeVisible()
    expect(screen.getByRole("button", { name: "Settings" })).toBeVisible()
    setWindowWidth(900)
    fireEvent(window, new Event("resize"))
    expect(screen.getByRole("tablist", { name: "Main sections" })).toBeVisible()
    expect(screen.queryByRole("button", { name: "Open navigation" })).toBeNull()
  })
  it("opens the existing Settings window without changing the selected section", () => {
    render(<MainWindowView />)
    fireEvent.click(screen.getByRole("button", { name: "Settings" }))
    expect(openSettingsWindow).toHaveBeenCalledExactlyOnceWith()
    expect(screen.getByRole("tab", { name: "Activity" })).toHaveAttribute(
      "aria-selected",
      "true",
    )
    expect(capability.permissions).toContain("allow-open-settings-window")
  })

  it("shows a recoverable Settings error", async () => {
    vi.mocked(openSettingsWindow).mockRejectedValueOnce(new Error("Unavailable"))
    render(<MainWindowView />)
    fireEvent.click(screen.getByRole("button", { name: "Settings" }))
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not open Settings")
    fireEvent.click(screen.getByRole("button", { name: "Settings" }))
    expect(screen.queryByRole("alert")).toBeNull()
  })

  it.each(["metaKey", "ctrlKey"])(
    "opens Settings with %s+comma from the detail pane",
    (modifier) => {
      render(<MainWindowView />)
      fireEvent.keyDown(screen.getByRole("tabpanel", { name: "Activity" }), {
        key: ",",
        [modifier]: true,
      })
      expect(openSettingsWindow).toHaveBeenCalledExactlyOnceWith()
      fireEvent.keyDown(document, { key: ",", [modifier]: true, repeat: true })
      fireEvent.keyDown(document, { key: "," })
      fireEvent.keyDown(document, { key: ",", [modifier]: true, shiftKey: true })
      expect(openSettingsWindow).toHaveBeenCalledTimes(1)
    },
  )

  it("opens a section's collection and detail without mounting unvisited sections", () => {
    const other = vi.fn(() => <p>Other workspace</p>)
    const sections = [
      {
        id: "activity",
        label: "Activity",
        icon: Activity,
        render: () => (
          <CollectionDetailPane
            title="Items"
            items={[{ id: "test", label: "Example" }]}
            emptyMessage="Empty"
            detailEmptyMessage="Choose an item"
            renderDetail={() => <p>Example detail</p>}
          />
        ),
      },
      { id: "other", label: "Other", icon: Activity, render: other },
    ]
    render(<MainWindowView sections={sections} />)
    expect(other).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole("option", { name: "Example" }))
    expect(screen.getByText("Example detail")).toBeVisible()
    fireEvent.click(screen.getByRole("tab", { name: "Activity" }))
    expect(screen.getByText("Example detail")).toBeVisible()
    fireEvent.click(screen.getByRole("tab", { name: "Other" }))
    expect(screen.getByText("Other workspace")).toBeVisible()
    fireEvent.click(screen.getByRole("tab", { name: "Activity" }))
    expect(screen.getByText("Example detail")).toBeVisible()
  })
})
