import { fireEvent, render, screen } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

import { ListDisplayToolbar } from "./ListDisplayToolbar"

const OPTIONS = [
  { value: "cost", label: "Cost" },
  { value: "weeklyPercent", label: "Week %" },
] as const

describe("ListDisplayToolbar", () => {
  it("renders a low-chrome display choice without a redundant visible label", () => {
    const onChange = vi.fn()
    render(
      <ListDisplayToolbar
        label="Today"
        options={OPTIONS}
        value="cost"
        onChange={onChange}
        ariaLabel="Session metric"
      />,
    )

    expect(screen.queryByRole("heading")).not.toBeInTheDocument()
    expect(screen.getByText("Today")).toHaveClass("text-label-tertiary")
    const control = screen.getByRole("radiogroup", { name: "Session metric" })
    expect(control).toHaveAttribute("data-variant", "text-tabs")
    expect(control).not.toHaveTextContent("Show")

    fireEvent.click(screen.getByRole("radio", { name: "Week %" }))
    expect(onChange).toHaveBeenCalledWith("weeklyPercent")
  })

  it("lets a window host opt the toolbar into its drag region", () => {
    const { container } = render(
      <ListDisplayToolbar
        options={OPTIONS}
        value="cost"
        onChange={() => {}}
        ariaLabel="Session metric"
        dragRegion
      />,
    )

    expect(container.firstElementChild).toHaveAttribute("data-tauri-drag-region", "deep")
    for (const button of screen.getAllByRole("radio")) {
      expect(button).not.toHaveAttribute("data-tauri-drag-region")
    }
  })
})
