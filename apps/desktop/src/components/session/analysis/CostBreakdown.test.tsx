import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import {
  inclusiveCostSubject,
  subagentsCostSubject,
  topLevelCostSubject,
  type LocalSessionCost,
} from "../../../lib/presentation/sessionCosts"
import type { SubagentMember } from "../../../lib/types/session"
import { CostBreakdown } from "./CostBreakdown"
import { subagentsExpandedStore } from "./subagentsExpandedStore"

afterEach(cleanup)

// The expanded flag lives in a module-level store, shared by every test in
// this file. Start each test from the same collapsed state so earlier tests
// cannot leak their expanded choice into later ones.
beforeEach(() => {
  subagentsExpandedStore.set(false)
})

function result(totalCostUsd = 2.4, over: Partial<LocalSessionCost> = {}): LocalSessionCost {
  return {
    subject: inclusiveCostSubject("claude-code", "parent"),
    inputTokens: 1,
    outputTokens: 2,
    cacheReadTokens: 3,
    cacheCreationTokens: 4,
    totalTokens: 10,
    inputCostUsd: 0.3,
    outputCostUsd: 0.8,
    cacheReadCostUsd: 1.1,
    cacheWriteCostUsd: 0.2,
    totalCostUsd,
    isActive: false,
    ...over,
  }
}

describe("CostBreakdown", () => {
  it("renders the total and its four billable component rows", () => {
    render(<CostBreakdown cost={result()} />)
    expect(screen.getByText("Input")).toBeTruthy()
    expect(screen.getByText("Output")).toBeTruthy()
    expect(screen.getByText("Cache read")).toBeTruthy()
    expect(screen.getByText("Cache write")).toBeTruthy()
    expect(screen.getByText("$0.30")).toBeTruthy()
    expect(screen.getByText("$1.10")).toBeTruthy()
    expect(screen.getByText("$2.40")).toBeTruthy()
  })

  it("labels a live subject as projected and a settled one as estimated", () => {
    const { unmount } = render(<CostBreakdown cost={result(2.4, { isActive: true })} />)
    expect(screen.getByText("Projected cost")).toBeTruthy()
    unmount()

    render(<CostBreakdown cost={result(2.4)} />)
    expect(screen.getByText("Estimated cost")).toBeTruthy()
  })

  it("shows the inclusive parent/sub-agent split above the components", () => {
    render(
      <CostBreakdown
        cost={result(41.45)}
        split={{
          parent: result(32.95, { subject: topLevelCostSubject("claude-code", "parent") }),
          subagents: result(8.5, { subject: subagentsCostSubject("claude-code", "parent") }),
          subagentCount: 3,
          members: [],
          sessionStartedAtEpoch: null,
        }}
      />,
    )
    expect(screen.getByText("Parent agent")).toBeTruthy()
    expect(screen.getByText("3 sub-agents")).toBeTruthy()
    expect(screen.getByText("$32.95")).toBeTruthy()
    expect(screen.getByText("$8.50")).toBeTruthy()
    expect(screen.getByText("$41.45")).toBeTruthy()
  })

  it('says "sub-agent" in the singular for a fan-out of one', () => {
    render(
      <CostBreakdown
        cost={result(41.45)}
        split={{
          parent: result(32.95),
          subagents: result(8.5),
          subagentCount: 1,
          members: [],
          sessionStartedAtEpoch: null,
        }}
      />,
    )
    expect(screen.getByText("1 sub-agent")).toBeTruthy()
  })

  it("omits the split entirely for a non-orchestration subject", () => {
    render(<CostBreakdown cost={result()} />)
    expect(screen.queryByText("Parent agent")).toBeNull()
  })

  it("shows each row's abbreviated token count and its percent share of the total", () => {
    render(
      <CostBreakdown
        cost={result(2.4, {
          inputTokens: 950,
          outputTokens: 1_200,
          cacheReadTokens: 14_000,
          cacheCreationTokens: 2_100_000,
          totalTokens: 2_116_150,
        })}
      />,
    )
    expect(screen.getByText("950")).toBeTruthy()
    expect(screen.getByText("1.2k")).toBeTruthy()
    expect(screen.getByText("14k")).toBeTruthy()
    // Cache write tokens and the footer's total both round to the same "2.1M".
    expect(screen.getAllByText("2.1M")).toHaveLength(2)
    // Input is $0.30 of a $2.40 total.
    expect(screen.getByText("13%")).toBeTruthy()
    // The footer always reads the full share.
    expect(screen.getByText("100%")).toBeTruthy()
  })

  it("shows each split row's own token count", () => {
    render(
      <CostBreakdown
        cost={result(41.45, { totalTokens: 500_000 })}
        split={{
          parent: result(32.95, {
            subject: topLevelCostSubject("claude-code", "parent"),
            totalTokens: 400_000,
          }),
          subagents: result(8.5, {
            subject: subagentsCostSubject("claude-code", "parent"),
            totalTokens: 100_000,
          }),
          subagentCount: 3,
          members: [],
          sessionStartedAtEpoch: null,
        }}
      />,
    )
    expect(screen.getByText("400k")).toBeTruthy()
    expect(screen.getByText("100k")).toBeTruthy()
  })

  it("reads the percent share as a dash when the total is zero", () => {
    render(
      <CostBreakdown
        cost={result(0, {
          inputCostUsd: 0,
          outputCostUsd: 0,
          cacheReadCostUsd: 0,
          cacheWriteCostUsd: 0,
        })}
      />,
    )
    // Four component rows plus the footer, all sharing an undefined percent.
    expect(screen.getAllByText("—")).toHaveLength(5)
  })
})

function member(over: Partial<SubagentMember> = {}): SubagentMember {
  return {
    agent: "claude-code",
    subagentId: "sub-1",
    label: "Investigate the build",
    cost: {
      totalUsd: 1.2,
      inputUsd: 0.5,
      outputUsd: 0.5,
      cacheReadUsd: 0.1,
      cacheWriteUsd: 0.1,
    },
    tokens: { inputTokens: 100, outputTokens: 50, cacheReadTokens: 10, cacheCreationTokens: 5 },
    startedAtEpoch: null,
    modelRuns: [{ model: "claude-sonnet-4-6" }],
    ...over,
  }
}

describe("CostBreakdown — sub-agent roster", () => {
  it("starts collapsed, with a chevron marking it as expandable", () => {
    render(
      <CostBreakdown
        cost={result(41.45)}
        split={{
          parent: result(32.95),
          subagents: result(8.5),
          subagentCount: 2,
          members: [member({ subagentId: "a" }), member({ subagentId: "b", label: "b" })],
          sessionStartedAtEpoch: null,
        }}
      />,
    )
    const toggle = screen.getByRole("button", { name: /2 sub-agents/ })
    expect(toggle.getAttribute("aria-expanded")).toBe("false")
    expect(screen.queryByText("Investigate the build")).toBeNull()
  })

  it("keeps the roster expanded across a remount, since the choice is global rather than per-session", () => {
    const split = {
      parent: result(32.95),
      subagents: result(8.5),
      subagentCount: 1,
      members: [member()],
      sessionStartedAtEpoch: null,
    }
    const { unmount } = render(<CostBreakdown cost={result(41.45)} split={split} />)
    fireEvent.click(screen.getByRole("button", { name: /1 sub-agent/ }))
    expect(screen.getByText("Investigate the build")).toBeTruthy()
    unmount()

    render(<CostBreakdown cost={result(41.45)} split={split} />)
    expect(
      screen.getByRole("button", { name: /1 sub-agent/ }).getAttribute("aria-expanded"),
    ).toBe("true")
    expect(screen.getByText("Investigate the build")).toBeTruthy()
  })

  it("expands to show one row per sub-agent, with its start time, label, model, and cost", () => {
    render(
      <CostBreakdown
        cost={result(41.45)}
        split={{
          parent: result(32.95),
          subagents: result(8.5),
          subagentCount: 1,
          members: [member({ startedAtEpoch: 1_700_000_900 })],
          sessionStartedAtEpoch: 1_700_000_000,
        }}
      />,
    )
    const toggle = screen.getByRole("button", { name: /1 sub-agent/ })
    fireEvent.click(toggle)
    expect(toggle.getAttribute("aria-expanded")).toBe("true")

    // 1_700_000_900 - 1_700_000_000 = 900s elapsed, as a clock offset.
    expect(screen.getByText("[00:15:00]")).toBeTruthy()
    expect(screen.getByText("Investigate the build")).toBeTruthy()
    expect(screen.getByText("sonnet-4-6")).toBeTruthy()
    expect(screen.getByText("$1.20")).toBeTruthy()
  })

  it('shows "—" for the start time when the session start is unknown', () => {
    render(
      <CostBreakdown
        cost={result(41.45)}
        split={{
          parent: result(32.95),
          subagents: result(8.5),
          subagentCount: 1,
          members: [member({ startedAtEpoch: 1_700_000_900 })],
          sessionStartedAtEpoch: null,
        }}
      />,
    )
    fireEvent.click(screen.getByRole("button", { name: /1 sub-agent/ }))
    expect(screen.getByText("[—]")).toBeTruthy()
  })

  it('shows "—" for the start time when the sub-agent\'s own start is unknown', () => {
    render(
      <CostBreakdown
        cost={result(41.45)}
        split={{
          parent: result(32.95),
          subagents: result(8.5),
          subagentCount: 1,
          members: [member({ startedAtEpoch: null })],
          sessionStartedAtEpoch: 1_700_000_000,
        }}
      />,
    )
    fireEvent.click(screen.getByRole("button", { name: /1 sub-agent/ }))
    expect(screen.getByText("[—]")).toBeTruthy()
  })

  it('shows "—" for a sub-agent that has no priced cost yet', () => {
    render(
      <CostBreakdown
        cost={result(41.45)}
        split={{
          parent: result(32.95),
          subagents: result(8.5),
          subagentCount: 1,
          members: [member({ cost: null, startedAtEpoch: 1_700_000_900 })],
          sessionStartedAtEpoch: 1_700_000_000,
        }}
      />,
    )
    fireEvent.click(screen.getByRole("button", { name: /1 sub-agent/ }))
    // The row's own cost and percent columns both read as unpriced; its
    // token count still comes through since `tokens` is independent of `cost`.
    expect(screen.getAllByText("—")).toHaveLength(2)
    expect(screen.getByText("165")).toBeTruthy()
  })

  it("opens a sub-agent's analysis when its row is clicked", () => {
    const onOpenSubagent = vi.fn()
    render(
      <CostBreakdown
        cost={result(41.45)}
        onOpenSubagent={onOpenSubagent}
        split={{
          parent: result(32.95),
          subagents: result(8.5),
          subagentCount: 1,
          members: [member({ subagentId: "sub-42", label: "Refactor auth" })],
          sessionStartedAtEpoch: null,
        }}
      />,
    )
    fireEvent.click(screen.getByRole("button", { name: /1 sub-agent/ }))
    fireEvent.click(screen.getByText("Refactor auth"))
    expect(onOpenSubagent).toHaveBeenCalledWith("sub-42", "Refactor auth")
  })

  it("falls back to the plain summary row when the split has no roster", () => {
    render(
      <CostBreakdown
        cost={result(41.45)}
        split={{
          parent: result(32.95),
          subagents: result(8.5),
          subagentCount: 3,
          members: [],
          sessionStartedAtEpoch: null,
        }}
      />,
    )
    expect(screen.queryByRole("button", { name: /3 sub-agents/ })).toBeNull()
    expect(screen.getByText("3 sub-agents")).toBeTruthy()
  })
})
