import { fireEvent, render, screen, within } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { AllowanceUsageAccountPayload } from "../../../lib/providerUsageIpc"
import { OverviewAllowanceChart } from "./OverviewAllowanceChart"

const DAY = 86400
const start = new Date(2026, 8, 1).getTime() / 1000
const end = start + 29.5 * DAY
const account: AllowanceUsageAccountPayload = {
  provider: "anthropic",
  displayName: "Claude",
  accountKey: "one",
  plan: null,
  utilization: null,
  chart: {
    shortWindows: [
      {
        startsAtEpoch: start,
        resetsAtEpoch: start + 18000,
        peakPercent: 30,
        points: [
          { atEpoch: start, percent: 0 },
          { atEpoch: start + 18000, percent: 30 },
        ],
      },
    ],
    weeklyWindows: [
      {
        lane: "weekly",
        startsAtEpoch: start,
        resetsAtEpoch: start + 7 * DAY,
        points: [
          { atEpoch: start, percent: 0 },
          { atEpoch: start + 7 * DAY, percent: 75 },
        ],
      },
    ],
    rolling: [{ atEpoch: start + 7 * DAY, percent: 40 }],
  },
}

function buttons() {
  return within(
    screen.getByRole("group", { name: "Allowance for the past 30 days" }),
  ).getAllByRole("button")
}

beforeEach(() => {
  vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(634)
  vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(224)
})
afterEach(() => vi.restoreAllMocks())

describe("OverviewAllowanceChart", () => {
  it("shows the quota windows and rolling utilization for the selected account", () => {
    const { container } = render(
      <OverviewAllowanceChart account={account} rangeStartEpoch={start} rangeEndEpoch={end} />,
    )
    expect(
      screen.getByText(
        /Claude: rolling subscription utilization ends this range at 40 percent/,
      ),
    ).toBeInTheDocument()
    expect(screen.getByText("5-hour window")).toBeVisible()
    expect(screen.getByText("Week")).toBeVisible()
    expect(screen.getByText("Average usage")).toBeVisible()
    const rect = container.querySelector("g[clip-path] > rect")!
    expect(Number(rect.getAttribute("height"))).toBeCloseTo(60)
    expect(Number(rect.getAttribute("width"))).toBeGreaterThan(0)
    expect(container.querySelector("path[stroke-linejoin]")).toHaveAttribute(
      "d",
      expect.stringContaining("M140,128"),
    )
    expect(buttons()).toHaveLength(30)
    expect(buttons()[29]).toHaveAccessibleName("Today, Average usage: 40%")
    fireEvent.focus(buttons()[29]!)
    expect(screen.getByRole("tooltip")).toHaveTextContent("Today · Average usage: 40%")
  })

  it("calls history before the first rolling point unknown", () => {
    render(
      <OverviewAllowanceChart account={account} rangeStartEpoch={start} rangeEndEpoch={end} />,
    )
    expect(buttons()[0]).toHaveAccessibleName(/Average usage: not enough history yet/)
    expect(buttons()[0]).not.toHaveAccessibleName(/0%/)
  })

  it("breaks the rolling line when history expires and resumes when new history arrives", () => {
    const { container } = render(
      <OverviewAllowanceChart
        account={{
          ...account,
          chart: {
            ...account.chart,
            rolling: [
              { atEpoch: start, percent: 40 },
              { atEpoch: start + DAY, percent: null },
              { atEpoch: start + 3 * DAY, percent: 40 },
              { atEpoch: start + 4 * DAY, percent: null },
            ],
          },
        }}
        rangeStartEpoch={start}
        rangeEndEpoch={end}
      />,
    )
    const path = container.querySelector("path[stroke-linejoin]")!.getAttribute("d")!
    expect(path.match(/[ML]/g)).toEqual(["M", "L", "M", "L"])
    const coordinates = path.match(/[\d.]+/g)!.map(Number)
    const expected = [0, 128, 20, 128, 60, 128, 80, 128]
    coordinates.forEach((value, index) => expect(value).toBeCloseTo(expected[index]!))
    expect(buttons()[1]).toHaveAccessibleName(/not enough history yet/)
    expect(buttons()[2]).toHaveAccessibleName(/40%/)
    expect(buttons()[29]).toHaveAccessibleName(/not enough history yet/)
    expect(screen.getByText(/Claude: not enough window history/)).toBeInTheDocument()
  })

  it("walks the days with arrow keys, Home, and End", () => {
    render(
      <OverviewAllowanceChart account={account} rangeStartEpoch={start} rangeEndEpoch={end} />,
    )
    const days = buttons()
    expect(days[29]).toHaveAttribute("tabindex", "0")
    fireEvent.keyDown(days[29]!, { key: "ArrowLeft" })
    expect(document.activeElement).toBe(days[28])
    fireEvent.keyDown(days[28]!, { key: "Home" })
    expect(document.activeElement).toBe(days[0])
    fireEvent.keyDown(days[0]!, { key: "ArrowLeft" })
    expect(document.activeElement).toBe(days[0])
    fireEvent.keyDown(days[0]!, { key: "End" })
    expect(document.activeElement).toBe(days[29])
  })

  it("measures the plot when an account arrives after loading", () => {
    const { rerender } = render(
      <OverviewAllowanceChart
        account={null}
        rangeStartEpoch={start}
        rangeEndEpoch={end}
        loading
      />,
    )
    expect(screen.getByRole("region", { name: "Allowance chart" })).toHaveAttribute(
      "aria-busy",
      "true",
    )
    expect(screen.queryByRole("group")).not.toBeInTheDocument()
    rerender(
      <OverviewAllowanceChart account={account} rangeStartEpoch={start} rangeEndEpoch={end} />,
    )
    expect(buttons()).toHaveLength(30)
  })
})
