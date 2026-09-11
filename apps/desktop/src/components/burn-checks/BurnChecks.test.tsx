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
    expect(screen.getByLabelText(value.accessibleDescription)).toHaveClass(
      "gap-x-2",
      "font-mono",
      "type-footnote",
      "tabular-nums",
      "text-label-secondary",
    )
    expect(screen.getByLabelText(value.accessibleDescription)).not.toHaveClass("type-callout")
    expect(screen.getByLabelText(value.accessibleDescription)).not.toHaveClass(
      "tracking-tight!",
    )
    expect(screen.getByLabelText(value.accessibleDescription)).not.toHaveClass("gap-x-1.5")
    expect(screen.getByLabelText(value.accessibleDescription)).not.toHaveClass("text-label")
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
    expect(screen.getByText("1 failed")).toHaveClass(
      "font-semibold!",
      "text-burn-check-failure-text",
    )
    expect(screen.getByText("2 passed")).toHaveClass("text-burn-check-pass-fill")
    expect(screen.getByText("2 passed")).not.toHaveClass("font-semibold!")
    expect(screen.getByText("1 not assessed")).toBeInTheDocument()
    const separators = container.querySelectorAll("[data-burn-check-separator]")
    expect(separators).toHaveLength(2)
    expect(separators[0]).toHaveTextContent("·")
    expect(separators[0]?.textContent).toBe("·")
    expect(separators[0]).toHaveClass("mx-0.5", "inline-block")
    expect(container.querySelector("[data-burn-check-indicator-wrap]")).toHaveClass(
      "translate-y-px",
    )
  })

  it("uses terminal marks only for complete all-pass and all-fail states", () => {
    const passed = render(<BurnCheckIndicator presentation={presentation("clean")} size={16} />)
    const passMark = passed.container.querySelector('[data-burn-check-indicator="pass"]')
    expect(passMark).not.toBeNull()
    expect(passMark).toHaveAttribute("width", "15")
    passed.unmount()

    const failed = render(
      <BurnCheckIndicator presentation={presentation("finding")} size={16} />,
    )
    expect(
      failed.container.querySelector('[data-burn-check-indicator="fail"]'),
    ).toHaveAttribute("width", "15")
  })

  it("uses a lighter compact segmented dial", () => {
    const { container } = render(
      <BurnCheckIndicator presentation={presentation("finding", "clean")} size={16} />,
    )
    const dial = container.querySelector("[data-segmented-radial-dial]")
    expect(dial).toHaveAttribute("width", "14")
    expect(dial?.querySelector("circle")).toHaveAttribute("stroke-width", "1.5")
    const [failed, passed] = dial?.querySelectorAll("circle") ?? []
    const renderedGap =
      Number(passed?.getAttribute("data-start-angle")) -
      Number(failed?.getAttribute("data-arc-angle"))
    expect(renderedGap).toBeGreaterThan(26)
  })

  it("renders summary hierarchy and a trailing slot without a second status dial", () => {
    const value = presentation("finding", "clean", "notAssessed")
    const { container } = render(
      <BurnCheckSummary presentation={value} trailing={<span>12% token burn</span>} />,
    )
    expect(container.querySelector("svg")).toBeNull()
    const headline = screen.getByTestId("burn-check-headline")
    expect(headline).toHaveClass("font-semibold!", "text-label")
    expect(headline).toHaveTextContent("All burn checks")
    expect(screen.getByText("30 days")).toHaveClass("type-footnote", "text-label-tertiary")
    expect(screen.queryByText("incomplete")).not.toBeInTheDocument()
    expect(screen.getByText("1 failed")).toHaveClass("text-burn-check-failure-text")
    expect(screen.getByText("1 passed")).toHaveClass("font-mono", "text-burn-check-pass-fill")
    expect(screen.getByText("1 failed")).toHaveClass("font-mono", "font-semibold!")
    expect(screen.getByText("1 not assessed")).toBeInTheDocument()
    expect(screen.queryByText("Evidence incomplete")).not.toBeInTheDocument()
    expect(screen.getByLabelText(value.accessibleDescription)).toHaveAttribute(
      "aria-label",
      expect.stringContaining("Evidence incomplete"),
    )
    expect(screen.getByText("12% token burn")).toBeInTheDocument()
    expect(screen.getByLabelText(value.accessibleDescription)).toHaveClass(
      "min-h-10",
      "px-[var(--space-md)]",
      "py-[var(--space-sm)]",
    )
  })

  it("can label a standalone indicator", () => {
    const value = presentation("clean", "notAssessed")
    render(<BurnCheckIndicator presentation={value} size={24} labelled />)
    expect(screen.getByRole("img", { name: value.accessibleDescription })).toBeInTheDocument()
  })
})
