import { isTauri } from "@tauri-apps/api/core"
import { getCurrentWindow } from "@tauri-apps/api/window"
import type { MouseEvent } from "react"
import { isMacOS } from "../../lib/platform"

const INTERACTIVE = [
  "button",
  "a",
  "input",
  "select",
  "textarea",
  "label",
  "summary",
  "[contenteditable]:not([contenteditable='false'])",
  "[draggable='true']",
  "[data-no-window-drag]",
  ".ui-scrollbar",
  ".recharts-wrapper",
  "[role='button']",
  "[role='link']",
  "[role='tab']",
  "[role='checkbox']",
  "[role='radio']",
  "[role='switch']",
  "[role='slider']",
  "[role='combobox']",
  "[role='textbox']",
  "[role='option']",
  "[role='menuitem']",
  "[tabindex]:not([tabindex='-1']):not([role='tabpanel'])",
].join(",")

const pendingDoubleClicks = new WeakMap<HTMLDivElement, { x: number; y: number }>()

function isDraggableHeader(event: MouseEvent<HTMLDivElement>): boolean {
  if (event.defaultPrevented || event.button !== 0 || !isTauri()) return false
  if (!(event.target instanceof Element) || event.target.closest(INTERACTIVE)) return false
  const header = event.currentTarget
    .closest(".main-window")
    ?.querySelector(".main-window-titlebar")
  if (!header) return false
  const bounds = header.getBoundingClientRect()
  return event.clientY >= bounds.top && event.clientY < bounds.bottom
}

/** Share the top window region with view controls without an input-blocking overlay. */
export function dragViewHeader(event: MouseEvent<HTMLDivElement>): void {
  pendingDoubleClicks.delete(event.currentTarget)
  if (!isDraggableHeader(event) || (event.detail !== 1 && event.detail !== 2)) return
  event.preventDefault()
  if (isMacOS() && event.detail === 2) {
    pendingDoubleClicks.set(event.currentTarget, { x: event.clientX, y: event.clientY })
    return
  }
  const window = getCurrentWindow()
  const action = event.detail === 2 ? window.toggleMaximize() : window.startDragging()
  void action.catch(() => console.error("Could not move or resize the window."))
}

/** Complete macOS double-clicks on release, unless the pointer leaves its starting position. */
export function finishViewHeaderClick(event: MouseEvent<HTMLDivElement>): void {
  const start = pendingDoubleClicks.get(event.currentTarget)
  pendingDoubleClicks.delete(event.currentTarget)
  if (
    !start ||
    event.detail !== 2 ||
    event.clientX !== start.x ||
    event.clientY !== start.y ||
    !isDraggableHeader(event)
  )
    return
  event.preventDefault()
  void getCurrentWindow()
    .toggleMaximize()
    .catch(() => {
      console.error("Could not move or resize the window.")
    })
}
