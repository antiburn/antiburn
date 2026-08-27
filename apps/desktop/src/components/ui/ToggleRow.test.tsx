import { fireEvent, render, screen } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

import { ToggleRow } from "./ToggleRow"

describe("ToggleRow", () => {
  it("names the switch after the row label so the label is never repeated", () => {
    render(
      <ToggleRow
        label="Scan on launch"
        description="Look for new sessions as soon as the app starts."
        checked={false}
        onChange={vi.fn()}
      />,
    )

    expect(screen.getByRole("switch", { name: "Scan on launch" })).toBeInTheDocument()
    // The visible label is the row's, not the switch's: exactly one node
    // carries the text, so a screen reader announces it once.
    expect(screen.getByText("Scan on launch").tagName).toBe("P")
    expect(
      screen.getByText("Look for new sessions as soon as the app starts."),
    ).toBeInTheDocument()
  })

  it("reports the next state on toggle", () => {
    const onChange = vi.fn()
    render(<ToggleRow label="Scan on launch" checked={false} onChange={onChange} />)

    fireEvent.click(screen.getByRole("switch", { name: "Scan on launch" }))
    expect(onChange).toHaveBeenCalledWith(true)
  })

  it("renders a checked row", () => {
    render(<ToggleRow label="Scan on launch" checked onChange={vi.fn()} />)

    expect(screen.getByRole("switch", { name: "Scan on launch" })).toBeChecked()
  })

  it("forwards the dimmed and disabled treatments", () => {
    const onChange = vi.fn()
    const { container } = render(
      <ToggleRow label="Scan on launch" checked onChange={onChange} dimmed disabled />,
    )

    expect(container.firstElementChild?.classList.contains("opacity-50")).toBe(true)
    const control = screen.getByRole("switch", { name: "Scan on launch" })
    expect(control).toBeDisabled()
    fireEvent.click(control)
    expect(onChange).not.toHaveBeenCalled()
  })
})
