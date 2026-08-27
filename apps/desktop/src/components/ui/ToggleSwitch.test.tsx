import { fireEvent, render, screen } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

import { ToggleSwitch } from "./ToggleSwitch"

describe("ToggleSwitch", () => {
  it("exposes a labelled switch that reports its state", () => {
    render(<ToggleSwitch checked onCheckedChange={vi.fn()} aria-label="Scan on launch" />)

    const control = screen.getByRole("switch", { name: "Scan on launch" })
    expect(control).toBeChecked()
    // The look is entirely CSS: the track and thumb classes are the contract
    // with styles/platform-controls.css.
    expect(control.className).toContain("ui-switch")
    expect(control.querySelector(".ui-switch-thumb")).not.toBeNull()
  })

  it("reports the next state, not the current one", () => {
    const onCheckedChange = vi.fn()
    render(
      <ToggleSwitch
        checked={false}
        onCheckedChange={onCheckedChange}
        aria-label="Scan on launch"
      />,
    )

    fireEvent.click(screen.getByRole("switch", { name: "Scan on launch" }))
    expect(onCheckedChange).toHaveBeenCalledWith(true)
  })

  it("stays controlled: a click alone does not flip the rendered state", () => {
    render(
      <ToggleSwitch checked={false} onCheckedChange={vi.fn()} aria-label="Scan on launch" />,
    )

    const control = screen.getByRole("switch", { name: "Scan on launch" })
    fireEvent.click(control)
    expect(control).not.toBeChecked()
  })

  it("does not fire while disabled", () => {
    const onCheckedChange = vi.fn()
    render(
      <ToggleSwitch
        checked={false}
        onCheckedChange={onCheckedChange}
        aria-label="Scan on launch"
        disabled
      />,
    )

    const control = screen.getByRole("switch", { name: "Scan on launch" })
    expect(control).toBeDisabled()
    fireEvent.click(control)
    expect(onCheckedChange).not.toHaveBeenCalled()
  })
})
