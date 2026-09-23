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

function account(): AllowanceUsageAccountPayload {
  return {
    provider: "claude",
    displayName: "Claude",
    accountKey: "a",
    plan: null,
    utilization: null,
    chart: {
      shortWindows: [{ startsAtEpoch: 3600, resetsAtEpoch: 3600 * 6, peakPercent: 40 }],
      weeklyWindows: [week(0, 80), week(WEEK, 60), week(2 * WEEK, 30)],
      rolling: [{ atEpoch: 0, percent: 25 }],
    },
  } as unknown as AllowanceUsageAccountPayload
}

describe("OverviewAllowanceRadial", () => {
  it("draws every weekly window as an overlapping petal and marks the current week", () => {
    const { container } = render(
      <OverviewAllowanceRadial account={account()} rangeEndEpoch={2 * WEEK + 3600} />,
    )
    // Two past petals and one current petal. Each petal has an area and an edge.
    expect(container.querySelectorAll("path")).toHaveLength(6)
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
    })
    const waste: WasteMarks = {
      // Three sessions in this week and one from last week, all near one time of week.
      pins: [
        pin(2 * WEEK + 3600, "a"),
        pin(2 * WEEK + 3700, "b"),
        pin(2 * WEEK + 3800, "c"),
        pin(WEEK + 3600, "d"),
      ],
      config: [{ detector: "unusedMcpServers", label: "Unused MCP servers", share: 0.42 }],
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
    expect([...pins].map((line) => line.getAttribute("opacity"))).toEqual([
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
  })
})
