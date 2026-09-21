import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, expect, it, vi } from "vitest"
import { WindowControls } from "./WindowControls"
import { WindowResizeHandles } from "./WindowResizeHandles"

const native = vi.hoisted(() => ({
  isMaximized: vi.fn(),
  onResized: vi.fn(),
  minimize: vi.fn(),
  toggleMaximize: vi.fn(),
  close: vi.fn(),
  startResizeDragging: vi.fn(),
}))
vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true }))
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => native }))
beforeEach(() => {
  vi.resetAllMocks()
  native.isMaximized.mockResolvedValue(false)
  native.onResized.mockResolvedValue(vi.fn())
  native.startResizeDragging.mockResolvedValue(undefined)
})
it("operates native caption controls and reflects maximize/restore state", async () => {
  render(<WindowControls />)
  await waitFor(() => expect(native.isMaximized).toHaveBeenCalled())
  fireEvent.click(screen.getByRole("button", { name: "Minimize window" }))
  expect(native.minimize).toHaveBeenCalledOnce()
  native.isMaximized.mockResolvedValue(true)
  fireEvent.click(screen.getByRole("button", { name: "Maximize window" }))
  expect(await screen.findByRole("button", { name: "Restore window" })).toBeVisible()
  fireEvent.click(screen.getByRole("button", { name: "Close window" }))
  expect(native.close).toHaveBeenCalledOnce()
  await act(async () => {})
})
it("offers Linux edge resizing", async () => {
  const { container } = render(<WindowResizeHandles />)
  const edge = container.querySelector('[data-direction="West"]')!
  expect(container.querySelectorAll(".main-window-resize-edge")).toHaveLength(8)
  fireEvent.pointerDown(edge, { button: 0 })
  expect(native.startResizeDragging).toHaveBeenCalledWith("West")
  await act(async () => {})
})
