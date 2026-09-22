import { fireEvent, render, screen, within } from "@testing-library/react"
import { beforeEach, describe, expect, it } from "vitest"

import type {
  ProviderAgentDayUsagePayload,
  ProviderUsageDayPayload,
} from "../../../lib/providerUsageIpc"
import { OverviewSpendChart } from "./OverviewSpendChart"
import { resetEntrances } from "./overviewEntrance"

function agent(
  agent: string,
  usd: number | null,
  costComplete = usd != null,
): ProviderAgentDayUsagePayload {
  return {
    agent,
    tokensIn: 900,
    tokensOut: 0,
    cacheRead: 0,
    estimatedUsd: usd,
    costComplete,
    sessionCount: 1,
  }
}

function day(
  localDate: string,
  agents: ProviderAgentDayUsagePayload[],
): ProviderUsageDayPayload {
  return {
    localDate,
    agents,
    tokensIn: agents.reduce((sum, usage) => sum + usage.tokensIn, 0),
    tokensOut: 0,
    cacheRead: 0,
    estimatedUsd: agents.some((usage) => usage.estimatedUsd != null)
      ? agents.reduce((sum, usage) => sum + (usage.estimatedUsd ?? 0), 0)
      : null,
    costComplete: agents.every((usage) => usage.costComplete),
    sessionCount: agents.length,
  }
}

const days = [
  day("2026-09-12", [agent("claude-code", 1), agent("codex", 2)]),
  day("2026-09-13", [agent("claude-code", 2), agent("codex", 3)]),
  day("2026-09-14", [agent("claude-code", 3), agent("codex", 4)]),
]

function dayButtons() {
  return within(
    screen.getByRole("group", { name: "Estimated spend for the past 30 days" }),
  ).getAllByRole("button")
}

beforeEach(() => {
  // The entrance plays once a run, so each test starts from a first run.
  resetEntrances()
})

describe("OverviewSpendChart", () => {
  it.each([
    [1200, ["$250", "$500", "$750", "$1,000", "$1,250"]],
    [1251, ["$500", "$1,000", "$1,500"]],
    [0.7, ["$0.20", "$0.40", "$0.60", "$0.80"]],
    [0, ["$0.20", "$0.40", "$0.60", "$0.80", "$1.00"]],
  ] as const)("fits readable ticks to a stacked peak of %s", (peak, labels) => {
    render(
      <OverviewSpendChart
        days={[day("2026-09-14", [agent("claude-code", peak / 2), agent("codex", peak / 2)])]}
      />,
    )
    for (const label of labels) {
      expect(
        screen.getAllByText(label).some((element) => element.className !== "invisible"),
      ).toBe(true)
    }
    expect(screen.queryByText("$2,000")).toBeNull()
  })

  it("stacks agent costs and removes the comparison period", () => {
    const { container } = render(<OverviewSpendChart days={days} />)
    expect(screen.getByText("Claude Code")).toBeVisible()
    expect(screen.getByText("Codex")).toBeVisible()
    expect(screen.queryByText("30 days before")).toBeNull()
    const codex = container.querySelectorAll('[data-agent="codex"] rect')
    const claude = container.querySelectorAll('[data-agent="claude-code"] rect')
    expect(codex).toHaveLength(3)
    expect(claude).toHaveLength(3)
    expect(codex[2]).toHaveAttribute("y", "50")
    expect(codex[2]).toHaveAttribute("height", "50")
    expect(claude[2]).toHaveAttribute("y", "12.5")
    expect(claude[2]).toHaveAttribute("height", "37.5")
    expect(codex[2]).toHaveAttribute("x", claude[2]!.getAttribute("x"))
    const buttons = dayButtons()
    expect(buttons).toHaveLength(3)
    expect(buttons[2]).toHaveAttribute(
      "aria-label",
      "Today · $7.00 · 1.80k · 2 sessions · Claude Code: $3.00 · Codex: $4.00",
    )
    fireEvent.focus(buttons[2]!)
    expect(screen.getByRole("tooltip")).toHaveTextContent("Codex: $4.00")
  })

  it("walks the days with the arrow keys and retains focus by date", () => {
    const { rerender } = render(<OverviewSpendChart days={days} />)
    const buttons = dayButtons()
    fireEvent.focus(buttons[2]!)
    fireEvent.keyDown(buttons[2]!, { key: "ArrowLeft" })
    expect(document.activeElement).toBe(buttons[1])
    fireEvent.keyDown(buttons[1]!, { key: "Home" })
    expect(document.activeElement).toBe(buttons[0])
    fireEvent.keyDown(buttons[0]!, { key: "ArrowLeft" })
    expect(document.activeElement).toBe(buttons[0])
    fireEvent.keyDown(buttons[0]!, { key: "End" })
    expect(document.activeElement).toBe(buttons[2])
    rerender(<OverviewSpendChart days={[...days, day("2026-09-15", [agent("codex", 2)])]} />)
    expect(dayButtons()[2]).toHaveAttribute("tabindex", "0")
    expect(dayButtons()[3]).toHaveAttribute("tabindex", "-1")
  })

  it("leaves gaps for unknown costs and marks partial totals", () => {
    const data = [days[0]!, day("2026-09-13", [agent("codex", null)]), days[2]!]
    const { container, rerender } = render(<OverviewSpendChart days={data} />)
    expect(container.querySelectorAll('[data-agent="codex"] rect')).toHaveLength(2)
    expect(dayButtons()[1]).toHaveAccessibleName(/not priced/)
    expect(dayButtons()[1]!.querySelector("[data-unpriced]")).not.toBeNull()
    rerender(<OverviewSpendChart days={[day("2026-09-14", [agent("codex", 2, false)])]} />)
    expect(dayButtons()[0]).toHaveAccessibleName(/at least \$2.00/)
    expect(dayButtons()[0]!.querySelector("[data-unpriced]")).not.toBeNull()
  })

  it("keeps missing agents at zero and handles a single day", () => {
    const { container, rerender } = render(
      <OverviewSpendChart days={[days[0]!, day("2026-09-13", []), days[2]!]} />,
    )
    expect(container.querySelectorAll('[data-agent="claude-code"] rect')).toHaveLength(2)
    rerender(<OverviewSpendChart days={[days[0]!]} />)
    const rect = container.querySelector('[data-agent="claude-code"] rect')!
    for (const attr of ["x", "y", "width", "height"]) {
      expect(Number.isFinite(Number(rect.getAttribute(attr)))).toBe(true)
    }
    expect(dayButtons()).toHaveLength(1)
    expect(screen.getByText("Today")).toBeVisible()
  })

  it("does not invent an agent for an older payload without a breakdown", () => {
    const legacy = { ...days[0]! }
    delete legacy.agents
    const { container } = render(<OverviewSpendChart days={[legacy]} />)
    expect(container.querySelector("[data-agent]")).toBeNull()
    expect(dayButtons()[0]).toHaveAccessibleName(/Agent breakdown unavailable/)
  })

  it("shows a placeholder while loading and with no days", () => {
    const { rerender } = render(<OverviewSpendChart days={[]} loading />)
    expect(screen.queryByRole("group")).toBeNull()
    expect(screen.getByRole("region", { name: "Estimated spend by day" })).toHaveAttribute(
      "aria-busy",
      "true",
    )
    rerender(<OverviewSpendChart days={[]} />)
    expect(screen.queryByRole("group")).toBeNull()
  })

  it("stands a flat block in for the chart, and fades the chart in over it", () => {
    const { rerender } = render(<OverviewSpendChart days={[]} loading />)
    const region = screen.getByRole("region", { name: "Estimated spend by day" })
    const placeholder = region.querySelector(".overview-chart-placeholder")
    expect(placeholder).not.toBeNull()
    // A pulse on an empty chart draws the eye to the one thing with nothing
    // to read on it.
    expect(placeholder).not.toHaveClass("animate-pulse")
    expect(region).not.toHaveClass("overview-chart-in")

    rerender(<OverviewSpendChart days={[day("2026-09-20", [agent("claude-code", 1)])]} />)
    expect(region.querySelector(".overview-chart-placeholder")).toBeNull()
    expect(region).toHaveClass("overview-chart-in")
  })

  it("stands the block in the same frame the chart draws in", () => {
    const { rerender } = render(<OverviewSpendChart days={[]} loading />)
    const region = screen.getByRole("region", { name: "Estimated spend by day" })
    const whileLoading = region.children[1]!.className

    rerender(<OverviewSpendChart days={[day("2026-09-20", [agent("claude-code", 1)])]} />)
    // The chart's plot sits in a grid that also reserves the value labels
    // beside it and the day labels below. A block without them is taller than
    // the plot it stands in for, and the page settles as the chart arrives.
    expect(region.children[1]!.className).toBe(whileLoading)
  })

  it("draws itself in once a run, not every time the reader comes back", () => {
    const loaded = [day("2026-09-20", [agent("claude-code", 1)])]
    const { unmount } = render(<OverviewSpendChart days={loaded} />)
    expect(screen.getByRole("region", { name: "Estimated spend by day" })).toHaveClass(
      "overview-chart-in",
    )
    unmount()

    // The Overview is a tab the reader returns to. A reveal that replays on
    // every visit reads as a wait rather than an arrival.
    render(<OverviewSpendChart days={loaded} />)
    expect(screen.getByRole("region", { name: "Estimated spend by day" })).not.toHaveClass(
      "overview-chart-in",
    )
  })
})
