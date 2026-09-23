import { fireEvent, render, screen } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

// jsdom has no layout, so the chart gets a fixed frame.
vi.mock("../../../lib/useElementWidth", () => ({
  useElementWidth: () => 640,
  useElementHeight: () => 320,
}))

import type { AllowanceUsageAccountPayload } from "../../../lib/providerUsageIpc"
import { OverviewAllowanceWeeks } from "./OverviewAllowanceWeeks"

const WEEK = 7 * 86400

function week(start: number, top: number) {
  return {
    lane: "weekly",
    startsAtEpoch: start,
    resetsAtEpoch: start + WEEK,
    points: [
      { atEpoch: start, percent: 0 },
      { atEpoch: start + WEEK / 2, percent: top / 2 },
      { atEpoch: start + WEEK - 3600, percent: top },
    ],
  }
}

function account(): AllowanceUsageAccountPayload {
  return {
    provider: "claude",
    displayName: "Claude",
    accountKey: "a",
    plan: null,
    utilization: null,
    chart: {
      shortWindows: [
        {
          startsAtEpoch: 3600,
          resetsAtEpoch: 3600 * 6,
          peakPercent: 40,
          points: [
            { atEpoch: 3600, percent: 0 },
            { atEpoch: 3600 * 3, percent: 30 },
            { atEpoch: 3600 * 6, percent: 40 },
          ],
        },
      ],
      weeklyWindows: [week(0, 100), week(WEEK, 60), week(2 * WEEK, 30)],
      rolling: [{ atEpoch: 0, percent: 25 }],
    },
  } as unknown as AllowanceUsageAccountPayload
}

function renderChart() {
  return render(<OverviewAllowanceWeeks account={account()} rangeEndEpoch={2 * WEEK + 3600} />)
}

describe("OverviewAllowanceWeeks", () => {
  it("draws every week on one shared week, each in its own colour, with a label", () => {
    const { container } = renderChart()
    const bands = container.querySelectorAll("[data-week]")
    expect(bands).toHaveLength(3)
    expect([...bands].map((band) => band.getAttribute("class"))).toEqual([
      "week-band week-tone-2",
      "week-band week-tone-1",
      "week-band week-tone-0",
    ])
    // Each week is named at its line and in the key.
    expect(screen.getAllByText("This week")).toHaveLength(2)
    expect(screen.getAllByText("Last week")).toHaveLength(2)
    // The oldest week reached its limit, so its label says so.
    expect(screen.getByText("· limit")).toBeInTheDocument()
    expect(
      screen.getByText(
        "Claude: 3 weekly windows drawn over one week. Average usage is 25 percent. The weekly limit was hit in 1 of these weeks.",
      ),
    ).toBeInTheDocument()
  })

  it("draws a 5-hour window as a curve, and lights it with its week", () => {
    const { container } = renderChart()
    const edge = container.querySelector("[data-short] path:last-child")!
    expect(edge.getAttribute("d")?.split(" L")).toHaveLength(3)
    expect(edge).toHaveClass("stroke-(--week)/25")

    fireEvent.pointerEnter(container.querySelector(`[data-week-label="0"]`)!)
    expect(edge).toHaveClass("stroke-(--week)")
    // The other weeks fade.
    expect(container.querySelector(`[data-week="${WEEK}"]`)).toHaveStyle({ opacity: "0.25" })
  })

  it("lights a week and its 5-hour windows from its key entry", () => {
    const { container } = renderChart()
    fireEvent.pointerEnter(screen.getAllByText("2 weeks ago")[1]!.closest("[role=listitem]")!)
    expect(container.querySelector("[data-short] path:last-child")).toHaveClass(
      "stroke-(--week)",
    )
  })

  it("keeps every week on the context colour with one colour", () => {
    const { container } = renderChart()
    fireEvent.click(screen.getByRole("button", { name: "One colour" }))
    expect(container.querySelector("[data-week]")).toHaveAttribute("class", "week-band")
    expect(screen.getByText("Past weeks")).toBeInTheDocument()
  })

  it("breaks the weeks apart into rows and puts them back together", () => {
    const { container } = renderChart()
    const band = () => container.querySelector<SVGGElement>(`[data-week="0"]`)!
    expect(band().style.transform).toContain("scaleY(1.0000)")

    fireEvent.click(screen.getByRole("button", { name: "Break apart" }))
    expect(band().style.transform).not.toContain("scaleY(1.0000)")
    expect(screen.queryByText("Average usage")).not.toBeInTheDocument()

    fireEvent.click(screen.getByRole("button", { name: "Put together" }))
    expect(band().style.transform).toContain("scaleY(1.0000)")
  })
})
