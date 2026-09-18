import { fireEvent, render, screen, within } from "@testing-library/react"
import { describe, expect, it } from "vitest"

import type {
  AllowanceDayPayload,
  AllowanceUsageAccountPayload,
} from "../../../lib/providerUsageIpc"
import { OverviewAllowanceChart } from "./OverviewAllowanceChart"

function day(offset: number, usedPercent: number | null, blockCount = 0): AllowanceDayPayload {
  const date = new Date(2026, 8, 14 - (29 - offset))
  const localDate = `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`
  return { localDate, usedPercent, blockCount }
}

const days = Array.from({ length: 30 }, (_, index) =>
  day(index, index === 3 ? null : index * 0.5, index === 10 ? 2 : 0),
)
const previousDays = Array.from({ length: 30 }, (_, index) => day(index, 1))

function account(overrides: Partial<AllowanceUsageAccountPayload> = {}) {
  return {
    provider: "anthropic",
    displayName: "Claude",
    accountKey: "account",
    utilization: null,
    burst: null,
    overage: { blockCount: 0, waitedSeconds: 0, blocksWithoutWait: 0, lastBlockAt: null },
    days,
    previousDays,
    ...overrides,
  } satisfies AllowanceUsageAccountPayload
}

function dayButtons(): HTMLElement[] {
  const group = screen.getByRole("group", { name: "Allowance for the past 30 days" })
  return within(group).getAllByRole("button")
}

describe("OverviewAllowanceChart", () => {
  it("reads each day in allowance points and names the account", () => {
    render(<OverviewAllowanceChart accounts={[account()]} />)
    const buttons = dayButtons()
    expect(buttons).toHaveLength(30)
    const today = "Today · Claude 15 points"
    expect(buttons[29]).toHaveAttribute("aria-label", today)
    fireEvent.focus(buttons[29]!)
    expect(screen.getByRole("tooltip")).toHaveTextContent(today)
  })

  it("calls a day with no reading unknown, never zero", () => {
    render(<OverviewAllowanceChart accounts={[account()]} />)
    // The gap states no figure rather than a zero.
    expect(dayButtons()[3]).toHaveAttribute("aria-label", expect.stringContaining("no reading"))
  })

  it("marks a day that carried a limit hit", () => {
    const { container } = render(<OverviewAllowanceChart accounts={[account()]} />)
    expect(dayButtons()[10]).toHaveAttribute(
      "aria-label",
      expect.stringContaining("2 limit hits"),
    )
    expect(container.querySelectorAll(".overview-block-mark")).toHaveLength(1)
  })

  it("draws every account on one chart, each at its own weight", () => {
    // Two accounts both read in percent of their own plan, so one scale holds
    // them both. Color is the only thing that names which is which.
    render(
      <OverviewAllowanceChart
        accounts={[
          account(),
          account({ provider: "openai", displayName: "Codex", accountKey: "other" }),
        ]}
      />,
    )
    expect(screen.getAllByRole("region", { name: /Allowance by day/ })).toHaveLength(1)
    const buttons = dayButtons()
    expect(buttons).toHaveLength(30)
    expect(buttons[29]).toHaveAttribute(
      "aria-label",
      "Today · Claude 15 points · Codex 15 points",
    )
    expect(buttons[29]!.querySelectorAll(".overview-series")).toHaveLength(2)
    expect(buttons[29]!.querySelector(".bg-series-1")).toBeTruthy()
    expect(buttons[29]!.querySelector(".bg-series-2")).toBeTruthy()
  })

  it("draws one column for a date only one account knows", () => {
    // Two accounts can start metering on different days. A column always
    // holds the same date in every series.
    render(
      <OverviewAllowanceChart
        accounts={[
          account({ days: days.slice(28) }),
          account({ provider: "openai", displayName: "Codex", accountKey: "other" }),
        ]}
      />,
    )
    const buttons = dayButtons()
    expect(buttons).toHaveLength(30)
    expect(buttons[0]).toHaveAttribute(
      "aria-label",
      expect.stringContaining("Claude no reading"),
    )
  })

  it("says antiburn has no readings rather than drawing an empty chart", () => {
    render(<OverviewAllowanceChart accounts={[account({ days: [], previousDays: [] })]} />)
    expect(screen.getByRole("region", { name: "Allowance by day" }).textContent).toContain(
      "no meter readings",
    )
  })

  it("walks the days with the arrow keys", () => {
    render(<OverviewAllowanceChart accounts={[account()]} />)
    const buttons = dayButtons()
    buttons[29]!.focus()
    fireEvent.focus(buttons[29]!)
    fireEvent.keyDown(buttons[29]!, { key: "ArrowLeft" })
    expect(document.activeElement).toBe(buttons[28])
    fireEvent.keyDown(buttons[28]!, { key: "Home" })
    expect(document.activeElement).toBe(buttons[0])
    fireEvent.keyDown(buttons[0]!, { key: "End" })
    expect(document.activeElement).toBe(buttons[29])
  })
})
