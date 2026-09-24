import { fireEvent, render, screen } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

// jsdom has no layout, so the chart gets a fixed frame.
vi.mock("../../../lib/useElementWidth", () => ({
  useElementWidth: () => 640,
  useElementHeight: () => 320,
}))

import type { SessionListEntry } from "../../../components/session/SessionList"
import type {
  AllowanceUsageAccountPayload,
  SessionLimitAllocationPayload,
} from "../../../lib/providerUsageIpc"
import { OverviewAllowanceWeeks } from "./OverviewAllowanceWeeks"
import type { UsageInput } from "./usageSessions"
import type { WastePin, WasteMarks } from "./wasteMarks"

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

function pin(atEpoch: number, title: string, handle: string): WastePin {
  return {
    detector: "cacheChurn",
    label: "Cache churn",
    atEpoch,
    title,
    navigationHandle: handle,
    repo: "web",
    agent: "Claude",
    models: ["opus-4-5"],
    costUsd: 2.5,
    alsoFailed: ["Session overdepth"],
  }
}

const waste: WasteMarks = {
  pins: [pin(2 * WEEK + 1800, "Fix the parser", "h1")],
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

function session(sessionId: string, atEpoch: number, title: string) {
  return {
    agent: "claude",
    sessionId,
    repo: "web",
    timestamp: new Date(atEpoch * 1000).toISOString(),
    title,
  } as SessionListEntry
}

function share(
  sessionId: string,
  metric: SessionLimitAllocationPayload["metric"],
  percent: number,
) {
  return {
    agent: "claude",
    sessionId,
    wslDistro: null,
    metric,
    provider: "claude",
    accountKey: "a",
    percent,
  } as SessionLimitAllocationPayload
}

// Two sessions on the first day of this week, well after the oldest week.
const usage: UsageInput = {
  entries: [
    session("s1", 2 * WEEK + 3000, "Write the release notes"),
    session("s2", 2 * WEEK + 6000, "Refactor the parser"),
  ],
  allocations: [share("s1", "weekly", 3), share("s2", "weekly", 0.4)],
}

function renderChart(marks?: WasteMarks, sessions?: UsageInput) {
  return render(
    <OverviewAllowanceWeeks
      account={account()}
      rangeEndEpoch={2 * WEEK + 3600}
      waste={marks}
      usage={sessions}
      action={<button type="button">Optimise</button>}
    />,
  )
}

describe("OverviewAllowanceWeeks", () => {
  it("shows the session facts and its other failed checks on a pin", () => {
    const { container } = renderChart(waste)
    const head = container.querySelector("[data-waste-pin] .cursor-pointer")!
    fireEvent.pointerMove(head.closest("svg")!, { clientX: 40, clientY: 40 })
    fireEvent.pointerEnter(head)
    expect(screen.getByText("web · Claude · opus-4-5 · $2.50")).toBeInTheDocument()
    expect(screen.getByText("Session overdepth")).toBeInTheDocument()
  })

  it("shows the avoidable share and the suggested change on a check", () => {
    const marks: WasteMarks = {
      ...waste,
      pins: [pin(2 * WEEK + 1800, "Fix the parser", "h1"), pin(2 * WEEK + 2400, "Tidy", "h2")],
      checks: [
        { detector: "cacheChurn", burnBasisPoints: 1_250, change: "opus-4-5 → sonnet-4-5" },
      ],
    }
    const { container } = renderChart(marks)
    const callout = container.querySelector("[data-pin-callout=cacheChurn]")!
    fireEvent.pointerMove(callout.closest("svg")!, { clientX: 40, clientY: 40 })
    fireEvent.pointerEnter(callout)
    expect(screen.getByText("12% of tokens")).toBeInTheDocument()
    expect(screen.getByText("opus-4-5 → sonnet-4-5")).toBeInTheDocument()
  })

  it("lists the top sessions of a day by their share of the week", () => {
    const { container } = renderChart(undefined, usage)
    const day = container.querySelector("[data-week-day='0']")!
    fireEvent.pointerMove(day.closest("svg")!, { clientX: 40, clientY: 40 })
    fireEvent.pointerEnter(day)
    expect(screen.getByText("Top sessions, est. share of the week")).toBeInTheDocument()
    const rows = screen
      .getAllByText(/^(~\d+%|<1%)$/)
      .map((node) => node.parentElement!.textContent)
    expect(rows).toEqual(["Write the release notes~3%", "Refactor the parser<1%"])
  })

  it("says when a week is older than the session detail", () => {
    const { container } = renderChart(undefined, usage)
    const label = container.querySelector(`[data-week-label="0"]`)!
    fireEvent.pointerMove(label.closest("svg")!, { clientX: 40, clientY: 40 })
    fireEvent.pointerEnter(label)
    expect(screen.getByText("No session detail this far back")).toBeInTheDocument()
  })

  it("draws every week on one shared week, in one colour, with a label", () => {
    const { container } = renderChart()
    const bands = container.querySelectorAll("[data-week]")
    expect(bands).toHaveLength(3)
    expect([...bands].map((band) => band.getAttribute("class"))).toEqual([
      "week-band",
      "week-band",
      "week-band",
    ])
    // Each week is named at its line and has its own key entry.
    expect(screen.getAllByText("This week")).toHaveLength(2)
    expect(screen.getAllByText("Last week")).toHaveLength(2)
    expect(screen.getAllByText("2 weeks ago")).toHaveLength(2)
    expect(screen.queryByText("Past weeks")).not.toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "One colour" })).not.toBeInTheDocument()
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

  it("breaks the weeks apart into rows and puts them back together", () => {
    const { container } = renderChart()
    const band = () => container.querySelector<SVGGElement>(`[data-week="0"]`)!
    expect(band().style.transform).toContain("scaleY(1.0000)")

    const toggle = screen.getByRole("button", { name: "Break apart" })
    expect(toggle).toHaveAttribute("aria-pressed", "false")
    fireEvent.click(toggle)
    expect(band().style.transform).not.toContain("scaleY(1.0000)")
    expect(screen.getByRole("button", { name: "Put together" })).toHaveAttribute(
      "aria-pressed",
      "true",
    )
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
    // The pin and the config list share one key entry.
    expect(screen.getByText("Failed check")).toBeInTheDocument()
  })

  it("lists the config checks to the left of the plot, which sits in the middle", () => {
    const { container } = renderChart(waste)
    const texts = [...container.querySelectorAll("[data-week-config] text")]
    expect(texts.map((text) => Number(text.getAttribute("x")))).toEqual([0, 0])
    // The frame is 640 wide. The plot has 152 on each side, so the week
    // labels start 8 past its right edge.
    const label = container.querySelector<SVGGElement>(`[data-week-label="0"]`)!
    expect(label.style.transform).toMatch(/^translate\(496px, /)
  })

  it("lights the failed checks from the action, not from the row around it", () => {
    const { container } = renderChart(waste)
    const callout = () => container.querySelector("[data-pin-callout=cacheChurn]")!
    fireEvent.pointerEnter(container.querySelector("[data-week-config=unusedMcpServers]")!)
    expect(callout()).toHaveStyle({ opacity: "0.25" })
    fireEvent.pointerEnter(container.querySelector("[data-chart-action]")!.parentElement!)
    expect(callout()).toHaveStyle({ opacity: "0.25" })
    fireEvent.pointerEnter(container.querySelector("[data-chart-action]")!)
    expect(callout()).toHaveStyle({ opacity: "1" })
  })

  it("names each failed check in a callout, and lights its pins from it", () => {
    const { container } = renderChart(waste)
    const callout = container.querySelector("[data-pin-callout=cacheChurn]")!
    expect(callout).toHaveTextContent("Cache churnFix the parser")
    fireEvent.pointerEnter(container.querySelector("[data-week-config=unusedMcpServers]")!)
    expect(callout).toHaveStyle({ opacity: "0.25" })
    fireEvent.pointerEnter(callout)
    expect(container.querySelector("[data-waste-pin]")).toHaveStyle({ opacity: "1" })
    // Apart, the callouts above the plot hide and each row names its pins.
    const rows = () => container.querySelector("[data-row-callouts]")!
    expect(rows()).toHaveStyle({ opacity: "0" })
    fireEvent.click(screen.getByRole("button", { name: "Break apart" }))
    expect(container.querySelector("[data-pin-callouts]")).toHaveStyle({ opacity: "0" })
    expect(rows()).toHaveStyle({ opacity: "1" })
    expect(container.querySelector("[data-row-callout=cacheChurn]")).toHaveTextContent(
      "Cache churnFix the parser",
    )
  })

  it("lights a week's annotations from its key entry", () => {
    const { container } = renderChart(waste)
    const callout = () => container.querySelector("[data-pin-callout=cacheChurn]")!
    const key = (name: string) => screen.getAllByText(name).at(-1)!.closest("[role=listitem]")!
    fireEvent.pointerEnter(key("Last week"))
    expect(callout()).toHaveStyle({ opacity: "0.25" })
    fireEvent.pointerEnter(key("2 weeks ago"))
    expect(callout()).toHaveStyle({ opacity: "0.25" })
    fireEvent.pointerEnter(key("This week"))
    expect(callout()).toHaveStyle({ opacity: "1" })
  })

  it("keeps each week line's width when it takes the focus", () => {
    const { container } = renderChart()
    const edge = () =>
      container
        .querySelector(`[data-week="${WEEK}"] > path:last-of-type`)!
        .getAttribute("stroke-width")
    const before = edge()
    fireEvent.pointerEnter(container.querySelector(`[data-week-label="${WEEK}"]`)!)
    expect(edge()).toBe(before)
  })

  it("puts the action under the key", () => {
    renderChart()
    const action = screen.getByRole("button", { name: "Optimise" })
    expect(screen.getByRole("list", { name: "Layers" }).compareDocumentPosition(action)).toBe(
      Node.DOCUMENT_POSITION_FOLLOWING,
    )
  })
})
