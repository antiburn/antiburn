import { fireEvent, render, screen } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

import { SegmentedControl } from "./SegmentedControl"

const TWO = [
  { value: "left", label: "Left" },
  { value: "right", label: "Right" },
] as const

const FOUR = [
  { value: "a", label: "A" },
  { value: "b", label: "B" },
  { value: "c", label: "C" },
  { value: "d", label: "D" },
] as const

describe("SegmentedControl", () => {
  it("exposes a labelled radiogroup with one radio per option", () => {
    render(
      <SegmentedControl options={TWO} value="left" onChange={() => {}} ariaLabel="Placement" />,
    )

    expect(screen.getByRole("radiogroup", { name: "Placement" })).toBeTruthy()
    expect(screen.getAllByRole("radio")).toHaveLength(2)
  })

  it("marks only the selected option as checked and reports changes (2 options)", () => {
    const onChange = vi.fn()
    render(
      <SegmentedControl options={TWO} value="left" onChange={onChange} ariaLabel="Placement" />,
    )

    expect(screen.getByRole("radio", { name: "Left" }).getAttribute("aria-checked")).toBe(
      "true",
    )
    expect(screen.getByRole("radio", { name: "Right" }).getAttribute("aria-checked")).toBe(
      "false",
    )

    fireEvent.click(screen.getByRole("radio", { name: "Right" }))
    expect(onChange).toHaveBeenCalledWith("right")
  })

  it("works unchanged with four options", () => {
    const onChange = vi.fn()
    render(<SegmentedControl options={FOUR} value="c" onChange={onChange} ariaLabel="Mode" />)

    expect(screen.getAllByRole("radio")).toHaveLength(4)
    expect(screen.getByRole("radio", { name: "C" }).getAttribute("aria-checked")).toBe("true")
    for (const name of ["A", "B", "D"]) {
      expect(screen.getByRole("radio", { name }).getAttribute("aria-checked")).toBe("false")
    }

    fireEvent.click(screen.getByRole("radio", { name: "D" }))
    expect(onChange).toHaveBeenCalledWith("d")
  })

  it("supports arrow-key navigation with roving focus", () => {
    const onChange = vi.fn()
    render(<SegmentedControl options={FOUR} value="b" onChange={onChange} ariaLabel="Mode" />)

    const selected = screen.getByRole("radio", { name: "B" })
    selected.focus()
    fireEvent.keyDown(selected, { key: "ArrowRight" })
    expect(onChange).toHaveBeenCalledWith("c")
    expect(document.activeElement).toBe(screen.getByRole("radio", { name: "C" }))
  })

  it("supports tab semantics and the restrained selection indicator", () => {
    render(
      <SegmentedControl
        options={FOUR}
        value="c"
        onChange={() => {}}
        ariaLabel="Session filter"
        equalWidth
        animatedIndicator
        semantics="tabs"
        idPrefix="sessions"
      />,
    )

    expect(screen.getByRole("tablist", { name: "Session filter" })).toBeTruthy()
    expect(screen.getByRole("tab", { name: "C" }).getAttribute("aria-selected")).toBe("true")
    expect(screen.getByRole("tab", { name: "C" }).getAttribute("aria-controls")).toBe(
      "sessions-panel",
    )
    expect(document.querySelector(".duration-\\[var\\(--duration-fast\\)\\]")).toBeTruthy()
    // The reduced-motion fallback fill is always in the DOM; CSS decides which
    // of the two indicators paints.
    expect(document.querySelector(".ui-segmented-reduced-fill")).toBeTruthy()
  })

  it("renders content-width text tabs without segmented-control chrome", () => {
    const onChange = vi.fn()
    render(
      <SegmentedControl
        options={FOUR}
        value="b"
        onChange={onChange}
        ariaLabel="Session filter"
        semantics="tabs"
        idPrefix="sessions"
        variant="text-tabs"
      />,
    )

    const tablist = screen.getByRole("tablist", { name: "Session filter" })
    expect(tablist.getAttribute("data-variant")).toBe("text-tabs")
    expect(tablist.className).toContain("inline-flex")
    expect(tablist.className).toContain("gap-3")
    for (const chrome of ["grid", "overflow-hidden", "border", "bg-surface-secondary"]) {
      expect(tablist.className).not.toContain(chrome)
    }
    expect(tablist.getAttribute("style")).toBeNull()
    expect(document.querySelector(".bg-accent-fill")).toBeNull()
    expect(document.querySelector(".duration-\\[var\\(--duration-fast\\)\\]")).toBeNull()

    const selected = screen.getByRole("tab", { name: "B" })
    expect(selected.className).toContain("font-medium")
    expect(selected.className).toContain("text-label")
    expect(selected.querySelector(".segmented-control-text-indicator")?.className).toContain(
      "opacity-100",
    )
    expect(
      screen.getByRole("tab", { name: "A" }).querySelector(".segmented-control-text-indicator")
        ?.className,
    ).toContain("opacity-0")

    selected.focus()
    fireEvent.keyDown(selected, { key: "End" })
    expect(onChange).toHaveBeenLastCalledWith("d")
    expect(document.activeElement).toBe(screen.getByRole("tab", { name: "D" }))
    fireEvent.keyDown(screen.getByRole("tab", { name: "D" }), { key: "Home" })
    expect(onChange).toHaveBeenLastCalledWith("a")
    expect(document.activeElement).toBe(screen.getByRole("tab", { name: "A" }))
  })
})
