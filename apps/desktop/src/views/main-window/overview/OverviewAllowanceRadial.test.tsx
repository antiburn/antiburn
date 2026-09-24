import { fireEvent, render, screen } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

// jsdom has no layout, so the chart gets a fixed square.
vi.mock("../../../lib/useElementWidth", () => ({
  useElementWidth: () => 320,
  useElementHeight: () => 320,
}))

import type { AllowanceUsageAccountPayload } from "../../../lib/providerUsageIpc"
import { OverviewAllowanceRadial } from "./OverviewAllowanceRadial"
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

function account(
  weeks = [week(0, 80), week(WEEK, 60), week(2 * WEEK, 30)],
  shortPeak = 40,
): AllowanceUsageAccountPayload {
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
          peakPercent: shortPeak,
          points: [
            { atEpoch: 3600, percent: 0 },
            { atEpoch: 3600 * 6, percent: shortPeak },
          ],
        },
      ],
      weeklyWindows: weeks,
      rolling: [{ atEpoch: 0, percent: 25 }],
    },
  } as unknown as AllowanceUsageAccountPayload
}

describe("OverviewAllowanceRadial", () => {
  it("draws every weekly window as an overlapping petal and marks the current week", () => {
    const { container } = render(
      <OverviewAllowanceRadial account={account()} rangeEndEpoch={2 * WEEK + 3600} />,
    )
    // Two past petals and one current petal, each an area and an edge, and one
    // 5-hour segment.
    expect(container.querySelectorAll("path")).toHaveLength(7)
    expect(container.querySelectorAll("path.fill-context-stroke\\/25")).toHaveLength(1)
    expect(
      screen.getByText(
        "Claude: 3 weekly windows drawn on one week. Average usage is 25 percent.",
      ),
    ).toBeInTheDocument()
  })

  it("pins wasteful sessions by time of week and flags config checks at the reset", () => {
    const onOpen = vi.fn()
    const pin = (at: number, handle: string) => ({
      detector: "sessionsOverDepth" as const,
      label: "Oversized agents",
      atEpoch: at,
      title: `Session ${handle}`,
      navigationHandle: handle,
      repo: "web",
      agent: "Claude",
      models: [],
      costUsd: null,
      alsoFailed: [],
    })
    const waste: WasteMarks = {
      // Three sessions in this week and one from last week, all near one time of week.
      pins: [
        pin(2 * WEEK + 3600, "a"),
        pin(2 * WEEK + 3700, "b"),
        pin(2 * WEEK + 3800, "c"),
        pin(WEEK + 3600, "d"),
      ],
      config: [
        {
          detector: "unusedMcpServers",
          label: "Unused MCP servers",
          share: 0.42,
          finding: 21,
          sessions: 50,
        },
      ],
      onOpen,
    }
    const { container } = render(
      <OverviewAllowanceRadial
        account={account()}
        rangeEndEpoch={2 * WEEK + 3600}
        waste={waste}
      />,
    )
    const pins = container.querySelectorAll("[data-waste-pin]")
    expect(pins).toHaveLength(4)
    // This week sits next to the ring at full strength. Past weeks fade.
    expect([...pins].map((pin) => (pin as SVGGElement).style.opacity)).toEqual([
      "1",
      "1",
      "1",
      "0.35",
    ])
    fireEvent.click(pins[0]!)
    expect(onOpen).toHaveBeenCalledWith(waste.pins[2])
    expect(screen.getByText("×4")).toBeInTheDocument()
    expect(container.querySelector("[data-waste-flag]")).toHaveTextContent(
      "Config · share of sessions42%Unused MCP servers",
    )
    expect(screen.getByText("Wasteful session")).toBeInTheDocument()

    // A hovered pin names its session. The card follows the pointer.
    fireEvent.pointerMove(pins[3]!.closest("svg")!, { clientX: 40, clientY: 40 })
    fireEvent.pointerOver(pins[3]!)
    expect(screen.getByText("Session d")).toBeInTheDocument()
    expect(screen.getByText("Click to open the session")).toBeInTheDocument()
    expect((pins[3] as SVGGElement).style.opacity).toBe("1")
    fireEvent.pointerOut(pins[3]!)

    // A config row brings its check forward and fades the pins.
    const row = container.querySelector("[data-radial-config]")!
    fireEvent.pointerMove(row.closest("svg")!, { clientX: 200, clientY: 20 })
    fireEvent.pointerOver(row)
    expect(screen.getByText("21 of 50 sessions (42%)")).toBeInTheDocument()
    expect((pins[0] as SVGGElement).style.opacity).toBe("0.25")
    fireEvent.pointerOut(row)

    // The key entry brings every pin forward, past weeks too.
    fireEvent.pointerOver(screen.getByText("Wasteful session"))
    expect([...pins].map((pin) => (pin as SVGGElement).style.opacity)).toEqual([
      "1",
      "1",
      "1",
      "1",
    ])
  })

  it("marks where a week and a 5-hour window hit their limits", () => {
    const { container } = render(
      <OverviewAllowanceRadial
        account={account([week(0, 100), week(WEEK, 60), week(2 * WEEK, 30)], 100)}
        rangeEndEpoch={2 * WEEK + 3600}
      />,
    )
    expect(
      screen.getByText(
        /The weekly limit was hit in 1 of these weeks\. The 5-hour limit was hit in 1 of the 5-hour windows\./,
      ),
    ).toBeInTheDocument()
    expect(screen.getByText("Limit hit")).toBeInTheDocument()
    const limit = container.querySelector("[data-radial-limit]")!
    fireEvent.pointerMove(limit.closest("svg")!, { clientX: 40, clientY: 40 })
    fireEvent.pointerOver(limit)
    expect(screen.getByText("Hit the weekly limit")).toBeInTheDocument()
    expect(screen.getByText("2 weeks ago")).toBeInTheDocument()
  })

  it("shows no limit marks for weeks under the limit", () => {
    const { container } = render(
      <OverviewAllowanceRadial account={account()} rangeEndEpoch={2 * WEEK + 3600} />,
    )
    expect(container.querySelector("[data-radial-limit]")).toBeNull()
    expect(screen.queryByText("Limit hit")).toBeNull()
  })
})
