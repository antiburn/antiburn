import { fireEvent, render, screen } from "@testing-library/react"
import { ChevronRight, ExternalLink } from "lucide-react"
import { describe, expect, it, vi } from "vitest"

import { PushButton } from "./PushButton"

describe("PushButton", () => {
  it("renders the standard push button and calls back on click", () => {
    const onClick = vi.fn()
    render(<PushButton onClick={onClick}>Open</PushButton>)

    const button = screen.getByRole("button", { name: "Open" })
    expect(button.className).toContain("ui-push-button")
    expect(button.className).not.toContain("bg-accent-fill")

    fireEvent.click(button)
    expect(onClick).toHaveBeenCalledTimes(1)
  })

  it("fills with the accent color in the primary variant", () => {
    render(
      <PushButton onClick={() => {}} variant="primary">
        Install
      </PushButton>,
    )

    const button = screen.getByRole("button", { name: "Install" })
    expect(button.className).toContain("bg-accent-fill")
    expect(button.className).toContain("text-white")
  })

  it("keeps a trailing glyph decorative and out of the accessible name", () => {
    render(
      <PushButton onClick={() => {}} trailingIcon={ExternalLink}>
        Open
      </PushButton>,
    )

    const button = screen.getByRole("button", { name: "Open" })
    const glyph = button.querySelector("svg")!
    expect(glyph.getAttribute("aria-hidden")).toBe("true")
    expect(glyph.getAttribute("width")).toBe("12")
  })

  it("takes an explicit accessible name when the label alone is ambiguous", () => {
    render(
      <PushButton
        onClick={() => {}}
        ariaLabel="Open widget settings"
        trailingIcon={ChevronRight}
      >
        Manage…
      </PushButton>,
    )

    expect(screen.getByRole("button", { name: "Open widget settings" })).toBeTruthy()
  })

  it("does not fire while disabled", () => {
    const onClick = vi.fn()
    render(
      <PushButton onClick={onClick} disabled>
        Open
      </PushButton>,
    )

    const button = screen.getByRole("button", { name: "Open" }) as HTMLButtonElement
    expect(button.disabled).toBe(true)
    fireEvent.click(button)
    expect(onClick).not.toHaveBeenCalled()
  })
})
