import { fireEvent, render, screen } from "@testing-library/react"
import { beforeEach, describe, expect, it } from "vitest"

import { ResponsiveNavigation } from "./ResponsiveNavigation"

function Navigation({ compact = true }: { compact?: boolean }) {
  return (
    <ResponsiveNavigation compact={compact} label="Test navigation">
      {(close) => <button onClick={close}>Choose section</button>}
    </ResponsiveNavigation>
  )
}

describe("ResponsiveNavigation", () => {
  beforeEach(() => {
    Object.defineProperty(HTMLDialogElement.prototype, "showModal", {
      configurable: true,
      value(this: HTMLDialogElement) {
        this.open = true
      },
    })
    Object.defineProperty(HTMLDialogElement.prototype, "close", {
      configurable: true,
      value(this: HTMLDialogElement) {
        this.open = false
      },
    })
  })

  it("opens a modal drawer and returns focus after activation or Escape", () => {
    render(<Navigation />)
    const trigger = screen.getByRole("button", { name: "Open Test navigation" })
    fireEvent.click(trigger)
    expect(screen.getByRole("dialog")).toHaveAttribute("open")
    fireEvent.click(screen.getByRole("button", { name: "Choose section" }))
    expect(screen.queryByRole("dialog")).toBeNull()
    expect(trigger).toHaveFocus()
    fireEvent.click(trigger)
    fireEvent(screen.getByRole("dialog"), new Event("cancel", { cancelable: true }))
    expect(screen.queryByRole("dialog")).toBeNull()
    expect(trigger).toHaveFocus()
  })

  it("resets an open drawer when the layout becomes wide", () => {
    const { rerender } = render(<Navigation />)
    fireEvent.click(screen.getByRole("button", { name: "Open Test navigation" }))
    rerender(<Navigation compact={false} />)
    expect(screen.queryByRole("dialog")).toBeNull()
    expect(screen.getByRole("button", { name: "Choose section" })).toBeVisible()
    rerender(<Navigation />)
    expect(screen.queryByRole("dialog")).toBeNull()
    expect(screen.getByRole("button", { name: "Open Test navigation" })).toHaveAttribute(
      "aria-expanded",
      "false",
    )
  })
})
