import { fireEvent, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { MainWindowLayout } from "./MainWindowLayout"

beforeEach(() => {
  localStorage.clear()
  vi.useFakeTimers()
  vi.stubGlobal("matchMedia", () => ({ matches: true }))
})
afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
})
function setup() {
  const onSearch = vi.fn(),
    onBack = vi.fn(),
    onForward = vi.fn()
  const view = render(
    <MainWindowLayout
      onSearch={onSearch}
      onBack={onBack}
      onForward={onForward}
      canBack
      sidebar={
        <div>
          <button role="tab">Sessions</button>
        </div>
      }
    >
      <input aria-label="Content field" />
    </MainWindowLayout>,
  )
  return { ...view, onSearch, onBack, onForward }
}

describe.each(["Macintosh", "Windows NT 10.0", "X11; Linux x86_64"])(
  "shared chrome on %s",
  (platform) => {
    beforeEach(() => vi.spyOn(navigator, "userAgent", "get").mockReturnValue(platform))
    it("uses platform window controls and shared drag regions", () => {
      const { container } = setup()
      expect(container.querySelector("header")).toHaveAttribute("data-tauri-drag-region")
      const dragSpace = container.querySelector(".main-window-drag-space")
      expect(dragSpace).toHaveAttribute("data-tauri-drag-region")
      expect(dragSpace).toBeEmptyDOMElement()
      expect(container.querySelector(".main-window-titlebar-controls")).toHaveAttribute(
        "data-tauri-drag-region",
      )
      expect(screen.getByRole("button", { name: "Back" })).not.toHaveAttribute(
        "data-tauri-drag-region",
      )
      expect(screen.queryByRole("group", { name: "Window controls" }) !== null).toBe(
        platform !== "Macintosh",
      )
    })
    it("keeps navigation visible despite saved collapse state and old shortcuts", () => {
      localStorage.setItem("antiburn.main.sidebar-collapsed", "true")
      const { container, onSearch } = setup()
      const modifier = platform === "Macintosh" ? { metaKey: true } : { ctrlKey: true }
      expect(screen.queryByRole("button", { name: "Toggle sidebar" })).toBeNull()
      expect(screen.getByRole("tab", { name: "Sessions" })).toBeVisible()
      expect(container.querySelector("#main-sidebar")).not.toHaveAttribute("inert")
      expect(container.querySelector(".main-window-edge")).toBeNull()
      expect(fireEvent.keyDown(document, { key: "b", ...modifier })).toBe(true)
      expect(screen.getByRole("tab", { name: "Sessions" })).toBeVisible()
      expect(localStorage.getItem("antiburn.main.sidebar-collapsed")).toBe("true")
      const search = screen.getByRole("button", { name: "Search antiburn" })
      expect(search.previousElementSibling).toHaveAccessibleName("Forward")
      fireEvent.click(search)
      expect(onSearch).toHaveBeenCalledOnce()
      expect(screen.getByRole("button", { name: "Forward" })).toBeDisabled()
    })
    it("protects editing, composition and modal ownership", () => {
      const { onSearch, onBack } = setup()
      const modifier = platform === "Macintosh" ? { metaKey: true } : { ctrlKey: true }
      const back =
        platform === "Macintosh"
          ? { key: "[", metaKey: true }
          : { key: "ArrowLeft", altKey: true }
      fireEvent.keyDown(document, { key: "k", ...modifier })
      expect(onSearch).toHaveBeenCalledOnce()
      fireEvent.keyDown(screen.getByRole("textbox"), { key: "k", ...modifier })
      fireEvent.keyDown(document, { key: "k", ...modifier, isComposing: true })
      fireEvent.keyDown(document, { key: "k", ...modifier, repeat: true })
      expect(onSearch).toHaveBeenCalledOnce()
      fireEvent.keyDown(document, back)
      expect(onBack).toHaveBeenCalledOnce()
      const modal = document.createElement("dialog")
      modal.open = true
      document.body.append(modal)
      fireEvent.keyDown(document, back)
      expect(onBack).toHaveBeenCalledOnce()
      modal.remove()
    })
  },
)
