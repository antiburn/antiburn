import type { KeyboardEvent } from "react"

/** Make the app behind a modal dialog inert while the dialog is mounted. */
export function makeDialogBackgroundInert(node: HTMLDivElement) {
  const appRoot = node.ownerDocument.getElementById("root")
  const wasInert = appRoot?.hasAttribute("inert")
  appRoot?.setAttribute("inert", "")
  return () => {
    if (!wasInert) appRoot?.removeAttribute("inert")
  }
}

/** Close on Escape and keep Tab focus inside the dialog. */
export function trapDialogFocus(event: KeyboardEvent<HTMLElement>, close: () => void) {
  if (event.key === "Escape") return close()
  if (event.key !== "Tab") return
  const controls = Array.from(
    event.currentTarget.querySelectorAll<HTMLElement>(
      "button:not(:disabled), input:not(:disabled), select:not(:disabled)",
    ),
  )
  const first = controls[0]
  const last = controls.at(-1)
  if (event.shiftKey && document.activeElement === first) {
    event.preventDefault()
    last?.focus()
  } else if (!event.shiftKey && document.activeElement === last) {
    event.preventDefault()
    first?.focus()
  }
}
