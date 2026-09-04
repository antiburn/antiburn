import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { SessionMeterTabs, type MeterTab } from "./SessionMeterTabs"

afterEach(cleanup)

const TABS: ReadonlyArray<MeterTab<"overview" | "cost" | "tools">> = [
  {
    value: "overview",
    label: "Context",
    figure: "45%",
    percent: 45,
    meterLabel: "Peak context used of the window",
  },
  {
    value: "cost",
    label: "Cost",
    figureLabel: "Estimated cost",
    figure: "$2.40",
    percent: 12,
    meterLabel: "Share of the spend that was rewrite",
  },
  {
    value: "tools",
    label: "Tools",
    figure: "—",
    percent: null,
    meterLabel: "Share of startup context the session never used",
  },
]

function view(value: MeterTab<"overview" | "cost" | "tools">["value"] = "overview") {
  const onChange = vi.fn()
  render(
    <SessionMeterTabs
      tabs={TABS}
      value={value}
      onChange={onChange}
      ariaLabel="Session detail sections"
      idPrefix="session-detail-tabs"
    />,
  )
  return onChange
}

describe("SessionMeterTabs", () => {
  it("names each cell and states its figure and reading", () => {
    view()

    const cost = screen.getByRole("tab", { name: /^Cost/ })
    expect(cost).toHaveTextContent("$2.40")
    expect(cost).toHaveTextContent("Share of the spend that was rewrite: 12 percent")
    // The cell name is short, so the pointer tooltip carries the full one.
    expect(cost.getAttribute("title")).toContain("Estimated cost")
  })

  it("says a cell has no reading rather than reading zero", () => {
    view()

    expect(screen.getByRole("tab", { name: /^Tools/ })).toHaveTextContent(
      "Share of startup context the session never used: no stated figure",
    )
  })

  it("marks the selected cell and leaves the others out of the tab order", () => {
    view("cost")

    expect(screen.getByRole("tab", { name: /^Cost/ }).getAttribute("aria-selected")).toBe(
      "true",
    )
    expect(screen.getByRole("tab", { name: /^Cost/ }).getAttribute("tabindex")).toBe("0")
    expect(screen.getByRole("tab", { name: /^Context/ }).getAttribute("tabindex")).toBe("-1")
  })

  it("moves between cells with the arrow keys and wraps at each end", () => {
    const onChange = view("overview")

    fireEvent.keyDown(screen.getByRole("tab", { name: /^Context/ }), { key: "ArrowRight" })
    expect(onChange).toHaveBeenCalledWith("cost")

    fireEvent.keyDown(screen.getByRole("tab", { name: /^Context/ }), { key: "ArrowLeft" })
    expect(onChange).toHaveBeenCalledWith("tools")

    fireEvent.keyDown(screen.getByRole("tab", { name: /^Context/ }), { key: "End" })
    expect(onChange).toHaveBeenCalledWith("tools")
  })

  it("selects a cell on click", () => {
    const onChange = view()

    fireEvent.click(screen.getByRole("tab", { name: /^Tools/ }))
    expect(onChange).toHaveBeenCalledWith("tools")
  })
})
