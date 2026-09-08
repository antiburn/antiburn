import { render, screen } from "@testing-library/react"
import { describe, expect, it } from "vitest"

import { sessionBurnCheckPresentation } from "../../lib/presentation/burnChecks"
import { BurnCheckIndicator } from "./BurnCheckIndicator"
import { BurnCheckStatus } from "./BurnCheckStatus"
import { BurnCheckSummary } from "./BurnCheckSummary"

function presentation(...statuses: Array<"finding" | "clean" | "notAssessed">) {
  return sessionBurnCheckPresentation(
    statuses.map((status) => ({ status })),
    "ready",
  )
}

describe("Burn Check components", () => {
  it("renders matching dial segments and count phrases", () => {
    const value = presentation("finding", "clean", "clean", "notAssessed")
    const { container } = render(<BurnCheckStatus presentation={value} />)
    expect(container.querySelectorAll("circle")).toHaveLength(3)
    const failedAngle = Number(
      container.querySelector('[data-segment-id="failed"]')?.getAttribute("data-arc-angle"),
    )
    const passedAngle = Number(
      container.querySelector('[data-segment-id="passed"]')?.getAttribute("data-arc-angle"),
    )
    const unassessedAngle = Number(
      container.querySelector('[data-segment-id="unassessed"]')?.getAttribute("data-arc-angle"),
    )
    expect(passedAngle).toBeCloseTo(failedAngle * 2, 5)
    expect(unassessedAngle).toBeCloseTo(failedAngle, 5)
    expect(screen.getByText("1 Burn Check failed")).toHaveClass("text-burn-check-failure-text")
    expect(screen.getByText("2 passed")).not.toHaveClass("text-burn-check-pass-fill")
    expect(screen.queryByText(/not assessed/)).toBeNull()
  })

  it("uses terminal marks only for complete all-pass and all-fail states", () => {
    const passed = render(<BurnCheckIndicator presentation={presentation("clean")} size={16} />)
    expect(passed.container.querySelector('[data-burn-check-indicator="pass"]')).not.toBeNull()
    passed.unmount()

    const failed = render(
      <BurnCheckIndicator presentation={presentation("finding")} size={16} />,
    )
    expect(failed.container.querySelector('[data-burn-check-indicator="fail"]')).not.toBeNull()
  })

  it("renders summary hierarchy and a trailing slot", () => {
    const value = presentation("finding", "clean", "notAssessed")
    render(<BurnCheckSummary presentation={value} trailing={<span>12% token burn</span>} />)
    expect(screen.getByText("Burn Checks incomplete")).toHaveClass(
      "text-burn-check-failure-text",
    )
    expect(screen.getByText("1 failed")).toHaveClass("text-burn-check-failure-text")
    expect(screen.getByText("1 passed")).not.toHaveClass("text-burn-check-failure-text")
    expect(screen.getByText("1 not assessed")).toBeInTheDocument()
    expect(screen.getByText("12% token burn")).toBeInTheDocument()
  })

  it("can label a standalone indicator", () => {
    const value = presentation("clean", "notAssessed")
    render(<BurnCheckIndicator presentation={value} size={24} labelled />)
    expect(screen.getByRole("img", { name: value.accessibleDescription })).toBeInTheDocument()
  })
})
