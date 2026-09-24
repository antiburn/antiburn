import { render, screen } from "@testing-library/react"
import { describe, expect, it } from "vitest"

import type { AllowanceWindowLevelsPayload } from "../../../lib/providerUsageIpc"
import { OverviewRadialTooltip, type RadialData } from "./OverviewRadialTooltip"
import type { UsageSession } from "./usageSessions"

const HOUR = 3600
const WEEK = 7 * 24 * HOUR
const week = {
  startsAtEpoch: 0,
  resetsAtEpoch: WEEK,
  points: [{ atEpoch: 0, percent: 0 }],
} as AllowanceWindowLevelsPayload

function session(key: string, atEpoch: number, fiveHourPercent: number): UsageSession {
  return { key, atEpoch, title: key, weeklyPercent: 1, fiveHourPercent }
}

function data(sessions: UsageSession[]): RadialData {
  return {
    clock: week,
    current: week,
    past: [],
    weeks: [week],
    spokes: [
      {
        key: "s",
        weekStart: 0,
        current: true,
        from: 0,
        to: 0.03,
        startsAtEpoch: 10 * HOUR,
        resetsAtEpoch: 15 * HOUR,
        peakPercent: 100,
        points: [],
      },
    ],
    rolling: null,
    limits: [
      {
        weekStart: 0,
        current: true,
        from: 0.5,
        to: 1,
        hitAtEpoch: 50 * HOUR,
        untilEpoch: WEEK,
      },
    ],
    placed: [],
    config: [],
    sessions,
  }
}

describe("OverviewRadialTooltip", () => {
  it("lists a 5-hour window's sessions by their share of 5 hours", () => {
    render(
      <OverviewRadialTooltip
        focus={{ kind: "short", key: "s" }}
        fraction={null}
        data={data([
          session("Early", 9 * HOUR, 50),
          session("Small", 11 * HOUR, 5),
          session("Big", 12 * HOUR, 60),
        ])}
        style={{}}
      />,
    )
    expect(screen.getByText("Top sessions, est. share of 5 hours")).toBeInTheDocument()
    expect(screen.getAllByText(/^(Big|Small|Early)$/).map((node) => node.textContent)).toEqual([
      "Big",
      "Small",
    ])
  })

  it("lists the sessions active around a limit hit", () => {
    render(
      <OverviewRadialTooltip
        focus={{ kind: "limit", weekStart: 0 }}
        fraction={null}
        data={data([session("Before", 46 * HOUR, 1), session("Long before", 40 * HOUR, 1)])}
        style={{}}
      />,
    )
    expect(
      screen.getByText("Active around the hit, est. share of the week"),
    ).toBeInTheDocument()
    expect(screen.getByText("Before")).toBeInTheDocument()
    expect(screen.queryByText("Long before")).not.toBeInTheDocument()
  })
})
