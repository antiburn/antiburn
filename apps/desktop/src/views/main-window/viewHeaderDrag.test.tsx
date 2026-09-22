import { fireEvent, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { dragViewHeader, finishViewHeaderClick } from "./viewHeaderDrag"

const native = vi.hoisted(() => ({ startDragging: vi.fn(), toggleMaximize: vi.fn() }))
vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true }))
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => native }))
afterEach(() => vi.restoreAllMocks())
beforeEach(() => {
  vi.resetAllMocks()
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Windows NT 10.0")
  native.startDragging.mockResolvedValue(undefined)
  native.toggleMaximize.mockResolvedValue(undefined)
})
function setup() {
  const clicked = vi.fn()
  const view = render(
    <div className="main-window">
      <header className="main-window-titlebar" />
      <div onMouseDown={dragViewHeader} onMouseUp={finishViewHeaderClick}>
        <span>Header space</span>
        <button onClick={clicked}>
          <span>View action</span>
        </button>
        <input aria-label="View input" />
        <div role="tab" tabIndex={-1}>
          View tab
        </div>
        <div onMouseDown={(event) => event.preventDefault()}>Custom gesture</div>
      </div>
    </div>,
  )
  vi.spyOn(view.container.querySelector("header")!, "getBoundingClientRect").mockReturnValue({
    top: 0,
    bottom: 40,
  } as DOMRect)
  return clicked
}
it("drags only the shared top region and supports double-click maximize", () => {
  expect(navigator.userAgent).toBe("Windows NT 10.0")
  setup()
  const space = screen.getByText("Header space")
  fireEvent.mouseDown(space, { clientY: 12, button: 0, detail: 1 })
  expect(native.startDragging).toHaveBeenCalledOnce()
  fireEvent.mouseDown(space, { clientY: 12, button: 0, detail: 2 })
  expect(native.toggleMaximize).toHaveBeenCalledOnce()
  fireEvent.mouseDown(space, { clientY: 40, button: 0 })
  fireEvent.mouseDown(space, { clientY: 12, button: 2 })
  expect(native.startDragging).toHaveBeenCalledOnce()
})
it("preserves nested control clicks, inputs, tabs, and custom gestures in the top region", () => {
  const clicked = setup()
  for (const target of [
    screen.getByText("View action"),
    screen.getByRole("textbox"),
    screen.getByRole("tab"),
    screen.getByText("Custom gesture"),
  ]) {
    fireEvent.mouseDown(target, { clientY: 12, button: 0, detail: 1 })
  }
  fireEvent.click(screen.getByText("View action"))
  expect(clicked).toHaveBeenCalledOnce()
  expect(native.startDragging).not.toHaveBeenCalled()
  expect(native.toggleMaximize).not.toHaveBeenCalled()
})

it("maximizes on macOS release and cancels a moved or interactive release", () => {
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Macintosh")
  setup()
  const space = screen.getByText("Header space")
  const click = { clientX: 90, clientY: 12, button: 0, detail: 2 }
  fireEvent.mouseDown(space, click)
  expect(native.toggleMaximize).not.toHaveBeenCalled()
  fireEvent.mouseUp(space, click)
  expect(native.toggleMaximize).toHaveBeenCalledOnce()
  native.toggleMaximize.mockClear()
  fireEvent.mouseDown(space, click)
  fireEvent.mouseUp(space, { ...click, clientX: 95 })
  fireEvent.mouseDown(space, click)
  fireEvent.mouseUp(screen.getByText("View action"), click)
  expect(native.toggleMaximize).not.toHaveBeenCalled()
  expect(native.startDragging).not.toHaveBeenCalled()
})
