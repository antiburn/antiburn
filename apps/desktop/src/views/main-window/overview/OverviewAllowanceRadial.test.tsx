import { render, screen } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

// jsdom has no layout, so the chart gets a fixed square.
vi.mock("../../../lib/useElementWidth", () => ({
  useElementWidth: () => 320,
  useElementHeight: () => 320,
}))

import type { AllowanceUsageAccountPayload } from "../../../lib/providerUsageIpc"
import { OverviewAllowanceRadial } from "./OverviewAllowanceRadial"

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
})
