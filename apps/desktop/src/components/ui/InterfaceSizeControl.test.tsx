import { fireEvent, render, screen } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

import { INTERFACE_SCALE_PRESETS } from "../../lib/interfaceScale"
import { InterfaceSizeControl } from "./InterfaceSizeControl"

describe("InterfaceSizeControl", () => {
  it("exposes every supported preset and delegates changes without applying zoom", () => {
    const onChange = vi.fn()
    const onReset = vi.fn()
    render(
      <InterfaceSizeControl
        percent={125}
        presets={INTERFACE_SCALE_PRESETS}
        disabled={false}
        onChange={onChange}
        onReset={onReset}
      />,
    )
    expect(screen.getAllByRole("option").map((option) => option.textContent)).toEqual(
      INTERFACE_SCALE_PRESETS.map((percent) => `${percent}%`),
    )
    fireEvent.change(screen.getByRole("combobox", { name: "Interface size" }), {
      target: { value: "175" },
    })
    expect(onChange).toHaveBeenCalledWith(175)
    expect(screen.getByRole("combobox")).toHaveValue("125")
    fireEvent.click(screen.getByRole("button", { name: "Reset to 100%" }))
    expect(onReset).toHaveBeenCalledOnce()
  })

  it("disables changes while loading or saving and disables an unnecessary reset", () => {
    const props = {
      percent: 100,
      presets: INTERFACE_SCALE_PRESETS,
      onChange: vi.fn(),
      onReset: vi.fn(),
    }
    const { rerender } = render(<InterfaceSizeControl {...props} disabled={false} />)
    expect(screen.getByRole("combobox")).toBeEnabled()
    expect(screen.getByRole("button")).toBeDisabled()
    rerender(<InterfaceSizeControl {...props} percent={200} disabled />)
    expect(screen.getByRole("combobox")).toBeDisabled()
    expect(screen.getByRole("button")).toBeDisabled()
  })
})
