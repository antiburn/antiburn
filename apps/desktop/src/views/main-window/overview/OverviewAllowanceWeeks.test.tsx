import { fireEvent, render, screen } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

// jsdom has no layout, so the chart gets a fixed frame.
vi.mock("../../../lib/useElementWidth", () => ({
  useElementWidth: () => 640,
  useElementHeight: () => 320,
}))

import type { AllowanceUsageAccountPayload } from "../../../lib/providerUsageIpc"
import { OverviewAllowanceWeeks } from "./OverviewAllowanceWeeks"
import type { WasteMarks } from "./wasteMarks"

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

const waste: WasteMarks = {
  pins: [
    {
      detector: "cacheChurn",
      label: "Cache churn",
      atEpoch: 2 * WEEK + 1800,
      title: "Fix the parser",
      navigationHandle: "h1",
    },
  ],
  config: [
    {
      detector: "unusedMcpServers",
      label: "Unused MCP servers",
      share: 0.8,
      finding: 8,
      sessions: 10,
    },
  ],
}

function renderChart(marks?: WasteMarks) {
  return render(
    <OverviewAllowanceWeeks
      account={account()}
      rangeEndEpoch={2 * WEEK + 3600}
      waste={marks}
      action={<button type="button">Optimise</button>}
    />,
  )
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

  it("runs each pin down to its week's line and flags the config checks", () => {
    const { container } = renderChart(waste)
    const stem = container.querySelector<SVGLineElement>("[data-waste-pin] .week-stem")!
    expect(Number(/scaleY\((.+)\)/.exec(stem.style.transform)?.[1])).toBeGreaterThan(0)

    const row = container.querySelector("[data-week-config=unusedMcpServers]")!
    expect(row).toHaveTextContent("80%Unused MCP servers")
    fireEvent.pointerEnter(row)
    // The config row takes the focus, so the session pin steps back.
    expect(container.querySelector("[data-waste-pin]")).toHaveStyle({ opacity: "0.25" })
    // The pin and the flag share one key entry.
    expect(screen.getByText("Failed check")).toBeInTheDocument()
  })

  it("names each failed check in a callout, and lights its pins from it", () => {
    const { container } = renderChart(waste)
    const callout = container.querySelector("[data-pin-callout=cacheChurn]")!
    expect(callout).toHaveTextContent("Cache churnFix the parser")
    fireEvent.pointerEnter(container.querySelector("[data-week-config=unusedMcpServers]")!)
    expect(callout).toHaveStyle({ opacity: "0.25" })
    fireEvent.pointerEnter(callout)
    expect(container.querySelector("[data-waste-pin]")).toHaveStyle({ opacity: "1" })
    // The callouts hide when the weeks break apart.
    fireEvent.click(screen.getByRole("button", { name: "Break apart" }))
    expect(container.querySelector("[data-pin-callouts]")).toHaveStyle({ opacity: "0" })
  })

  it("puts the action under the key", () => {
    renderChart()
    const action = screen.getByRole("button", { name: "Optimise" })
    expect(screen.getByRole("list", { name: "Layers" }).compareDocumentPosition(action)).toBe(
      Node.DOCUMENT_POSITION_FOLLOWING,
    )
  })
})
