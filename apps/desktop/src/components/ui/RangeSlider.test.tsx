// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { fireEvent, render, screen } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

import { RangeSlider } from "./RangeSlider"

describe("RangeSlider", () => {
  it("shares the settings slider treatment and accessible value", () => {
    render(
      <RangeSlider
        value={3}
        min={1}
        max={7}
        onChange={vi.fn()}
        ariaLabel="Recent activity days"
        ariaValueText="3 days"
      />,
    )

    const slider = screen.getByRole("slider", { name: "Recent activity days" })
    expect(slider.className).toContain("bg-surface-secondary")
    expect(slider.className).toContain("accent-[var(--color-accent-fill)]")
    expect(slider.classList).toContain("cursor-default")
    expect(slider.classList).not.toContain("cursor-pointer")
    expect(slider.getAttribute("aria-valuetext")).toBe("3 days")
  })

  it("keeps the unavailable cursor treatment when disabled", () => {
    render(
      <RangeSlider
        value={10}
        min={3}
        max={30}
        onChange={vi.fn()}
        ariaLabel="Auto-dismiss time"
        disabled
      />,
    )

    const slider = screen.getByRole("slider", { name: "Auto-dismiss time" })
    expect((slider as HTMLInputElement).disabled).toBe(true)
    expect(slider.classList).toContain("cursor-default")
    expect(slider.classList).toContain("disabled:cursor-not-allowed")
  })

  it("changes continuously and commits once when a gesture settles", () => {
    const onChange = vi.fn()
    const onCommit = vi.fn()
    const { rerender } = render(
      <RangeSlider
        value={3}
        min={1}
        max={7}
        onChange={onChange}
        onCommit={onCommit}
        ariaLabel="Days"
      />,
    )
    const slider = screen.getByRole("slider", { name: "Days" })

    fireEvent.pointerDown(slider)
    fireEvent.change(slider, { target: { value: "5" } })
    rerender(
      <RangeSlider
        value={5}
        min={1}
        max={7}
        onChange={onChange}
        onCommit={onCommit}
        ariaLabel="Days"
      />,
    )
    fireEvent.pointerUp(slider)
    fireEvent.blur(slider)

    expect(onChange).toHaveBeenCalledWith(5)
    expect(onCommit).toHaveBeenCalledOnce()
    expect(onCommit).toHaveBeenCalledWith(5)
  })

  it("does not commit a gesture whose value did not change", () => {
    const onCommit = vi.fn()
    render(
      <RangeSlider
        value={3}
        min={1}
        max={7}
        onChange={vi.fn()}
        onCommit={onCommit}
        ariaLabel="Days"
      />,
    )
    const slider = screen.getByRole("slider", { name: "Days" })

    fireEvent.pointerDown(slider)
    fireEvent.pointerUp(slider)

    expect(onCommit).not.toHaveBeenCalled()
  })
})
