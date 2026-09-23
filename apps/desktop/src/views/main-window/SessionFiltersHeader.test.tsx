import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { useState } from "react"
import { afterEach, describe, expect, it, vi } from "vitest"

import type { SessionFilterCounts, SessionFilters } from "../../lib/sessionFilters"
import * as platform from "../../lib/platform"
import { SessionFiltersHeader, type SessionFiltersHeaderProps } from "./SessionFiltersHeader"

const COUNTS: SessionFilterCounts = {
  all: 12,
  matching: 3,
  agentsAll: 8,
  agents: { codex: 2, "claude-code": 1, "future-agent": 0 },
  result: { all: 6, failing: 3, passing: 2 },
  spend: { all: 5, notable: 3, material: 4 },
}

function props(overrides: Partial<SessionFiltersHeaderProps> = {}): SessionFiltersHeaderProps {
  return {
    filters: { agents: [], result: "all", spend: "all" },
    counts: COUNTS,
    agents: ["claude-code", "codex", "future-agent"],
    onToggleAgent: vi.fn(),
    onResetAgents: vi.fn(),
    onResultChange: vi.fn(),
    onSpendChange: vi.fn(),
    onClear: vi.fn(),
    days: 7,
    onChangeTimeRange: vi.fn(),
    ...overrides,
  }
}

function StatefulHeader({ initial }: { initial: SessionFilters }) {
  const [filters, setFilters] = useState(initial)
  return (
    <SessionFiltersHeader
      filters={filters}
      counts={COUNTS}
      agents={["claude-code", "codex", "future-agent"]}
      onToggleAgent={(agent) =>
        setFilters((current) => ({
          ...current,
          agents: current.agents.includes(agent)
            ? current.agents.filter((value) => value !== agent)
            : [...current.agents, agent],
        }))
      }
      onResetAgents={() => setFilters((current) => ({ ...current, agents: [] }))}
      onResultChange={(result) => setFilters((current) => ({ ...current, result }))}
      onSpendChange={(spend) => setFilters((current) => ({ ...current, spend }))}
      onClear={() => setFilters({ agents: [], result: "all", spend: "all" })}
      highCostThresholdUsd={7.25}
      days={7}
      onChangeTimeRange={() => {}}
    />
  )
}

function openFilters() {
  const trigger = screen.getByRole("button", { name: "Filters" })
  fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false, pointerType: "mouse" })
  return trigger
}

describe("SessionFiltersHeader", () => {
  afterEach(() => {
    vi.useRealTimers()
    vi.restoreAllMocks()
  })

  it.each([true, false])("adds a drag region only on macOS (%s)", (macOS) => {
    vi.spyOn(platform, "isMacOS").mockReturnValue(macOS)
    const { container } = render(<SessionFiltersHeader {...props()} />)
    expect(
      container
        .querySelector("[data-collection-header]")
        ?.getAttribute("data-tauri-drag-region"),
    ).toBe(macOS ? "deep" : null)
  })

  it("updates live text when the matching count changes without changing the total", () => {
    const { container, rerender } = render(<SessionFiltersHeader {...props()} />)

    expect(screen.getByRole("heading", { name: "Sessions" })).not.toHaveClass("sr-only")
    expect(screen.getByText("12 total sessions, 3 matching")).toHaveAttribute(
      "aria-live",
      "polite",
    )
    expect(screen.getByText("12 total sessions, 3 matching")).toHaveAttribute(
      "aria-atomic",
      "true",
    )
    expect(screen.getByText("12 total sessions, 3 matching")).toHaveTextContent("12")
    expect(container.querySelector("[data-active-session-filters]")).toBeNull()
    expect(screen.queryByText(/^Showing /)).not.toBeInTheDocument()
    const announcement = screen.getByText("12 total sessions, 3 matching")
    rerender(<SessionFiltersHeader {...props({ counts: { ...COUNTS, matching: 0 } })} />)
    expect(announcement).toHaveTextContent("12 total sessions, 0 matching")
  })

  it.each(["keyboard", "mouse"])(
    "dismisses the trigger tooltip when opening by %s",
    async (input) => {
      render(<SessionFiltersHeader {...props()} />)
      const trigger = screen.getByRole("button", { name: "Filters" })
      if (input === "keyboard") act(() => trigger.focus())
      else fireEvent.pointerMove(trigger, { pointerType: "mouse" })
      expect(await screen.findByRole("tooltip")).toHaveTextContent("Filter sessions")
      if (input === "keyboard") fireEvent.keyDown(trigger, { key: "Enter" })
      else openFilters()
      expect(await screen.findByRole("menu")).toBeInTheDocument()
      expect(trigger).toHaveAttribute("aria-expanded", "true")
      expect(trigger).toHaveAttribute("data-state", "open")
      expect(document.querySelector(".ui-tooltip")).toBeNull()
      expect(screen.queryByRole("tooltip")).not.toBeInTheDocument()
      vi.useFakeTimers()
      fireEvent.pointerLeave(trigger, { pointerType: "mouse" })
      fireEvent.pointerMove(trigger, { pointerType: "mouse" })
      await act(() => vi.advanceTimersByTimeAsync(700))
      expect(screen.queryByRole("tooltip")).not.toBeInTheDocument()
      vi.useRealTimers()
      fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" })
      await waitFor(() => expect(trigger).toHaveFocus())
    },
  )

  it.each([0, 3])("shows %s matches separately from count-free chips", (matching) => {
    render(
      <SessionFiltersHeader
        {...props({
          filters: { agents: ["future-agent"], result: "failing", spend: "notable" },
          counts: { ...COUNTS, matching },
          highCostThresholdUsd: 7.25,
        })}
      />,
    )

    expect(screen.getByText(`Showing ${matching}`)).toBeVisible()
    expect(
      screen.getByRole("button", {
        name: "Remove Future Agent filter",
      }),
    ).not.toHaveTextContent("0")
    expect(screen.getByRole("button", { name: "Remove Failed filter" })).toHaveTextContent(
      /^Failed$/,
    )
    expect(
      screen.getByRole("button", {
        name: "Remove High cost filter",
      }),
    ).toHaveTextContent(/^High cost$/)
  })

  it("explains the automatic cost threshold on keyboard focus and preserves menu selection", async () => {
    const onSpendChange = vi.fn()
    render(<SessionFiltersHeader {...props({ highCostThresholdUsd: 28.93, onSpendChange })} />)
    openFilters()
    const highCost = screen.getByRole("menuitemradio", {
      name: "High cost, Over $28.93, 3 matching sessions",
    })
    act(() => highCost.focus())
    expect(await screen.findByRole("tooltip")).toHaveTextContent(
      "Based on all agents in this time range: 3× the median session cost, with a $2 minimum.",
    )
    expect(highCost).toHaveAccessibleDescription(
      "Based on all agents in this time range: 3× the median session cost, with a $2 minimum.",
    )
    fireEvent.click(highCost)
    expect(onSpendChange).toHaveBeenCalledWith("notable")
    expect(screen.getByRole("menu")).toBeInTheDocument()
  })

  it("keeps the cost chip compact and explains its threshold on focus", async () => {
    render(
      <SessionFiltersHeader
        {...props({
          filters: { agents: [], result: "all", spend: "notable" },
          highCostThresholdUsd: 28.93,
        })}
      />,
    )
    const chip = screen.getByRole("button", {
      name: "Remove High cost filter",
    })
    expect(chip).toHaveTextContent("High cost")
    expect(chip).not.toHaveTextContent("$28.93")
    act(() => chip.focus())
    expect(await screen.findByRole("tooltip")).toHaveTextContent(
      "Over $28.93. Based on all agents in this time range: 3× the median session cost, with a $2 minimum.",
    )
  })

  it("moves keyboard focus to the next chip, then to Filters when none remain", async () => {
    render(<StatefulHeader initial={{ agents: ["codex"], result: "failing", spend: "all" }} />)

    const codex = screen.getByRole("button", {
      name: "Remove Codex filter",
    })
    codex.focus()
    fireEvent.click(codex, { detail: 0 })
    await act(async () => {})
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Remove Failed filter" }),
    )

    fireEvent.click(document.activeElement as HTMLElement, { detail: 0 })
    await act(async () => {})
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Filters" }))
  })

  it("clears two agent chips with one action and restores Filters focus", async () => {
    render(
      <StatefulHeader
        initial={{ agents: ["claude-code", "codex"], result: "all", spend: "all" }}
      />,
    )

    openFilters()
    const clear = screen.getByRole("menuitem", { name: "Clear filters" })
    fireEvent.click(clear)
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull())
    expect(screen.queryByRole("button", { name: /Remove .* filter/ })).toBeNull()
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Filters" }))
  })

  it("offers a menu reset for one active facet and omits it when none are active", () => {
    const { unmount } = render(<SessionFiltersHeader {...props()} />)
    openFilters()
    expect(screen.queryByRole("menuitem", { name: "Clear filters" })).toBeNull()
    unmount()
    const onClear = vi.fn()
    render(
      <SessionFiltersHeader
        {...props({ filters: { agents: [], result: "passing", spend: "all" }, onClear })}
      />,
    )
    openFilters()
    fireEvent.click(screen.getByRole("menuitem", { name: "Clear filters" }))
    expect(onClear).toHaveBeenCalledOnce()
  })

  it.each([1, 7])(
    "keeps the compact range actionable with its complete accessible label (%s)",
    (days) => {
      const onChangeTimeRange = vi.fn()
      render(<SessionFiltersHeader {...props({ days, onChangeTimeRange })} />)
      const control = screen.getByRole("button", {
        name: `${days === 1 ? "Today" : `Last ${days} days`}, change time range in Settings`,
      })
      expect(control).toHaveTextContent(days === 1 ? "Today" : `${days} days`)
      fireEvent.click(control)
      expect(onChangeTimeRange).toHaveBeenCalledOnce()
    },
  )

  it("moves keyboard focus to the previous chip when the last chip is removed", async () => {
    render(
      <StatefulHeader
        initial={{ agents: ["claude-code", "codex"], result: "all", spend: "all" }}
      />,
    )

    const codex = screen.getByRole("button", {
      name: "Remove Codex filter",
    })
    codex.focus()
    fireEvent.click(codex, { detail: 0 })
    await act(async () => {})
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Remove Claude Code filter" }),
    )
  })

  it("keeps the menu open for available agent and radio changes", () => {
    const onToggleAgent = vi.fn()
    const onResetAgents = vi.fn()
    const onResultChange = vi.fn()
    render(
      <SessionFiltersHeader
        {...props({
          filters: { agents: ["codex"], result: "all", spend: "all" },
          onToggleAgent,
          onResetAgents,
          onResultChange,
          highCostThresholdUsd: 31,
        })}
      />,
    )
    openFilters()

    const future = screen.getByRole("menuitemcheckbox", {
      name: "Future Agent, 0 matching sessions",
    })
    expect(future).toHaveAttribute("data-state", "unchecked")
    expect(future).toHaveAttribute("aria-disabled", "true")
    expect(future).toHaveTextContent("Future Agent")
    expect(
      screen.getByRole("menuitemradio", {
        name: "High cost, Over $31, 3 matching sessions",
      }),
    ).toHaveTextContent("Over $31")
    fireEvent.click(
      screen.getByRole("menuitemcheckbox", { name: "Claude Code, 1 matching session" }),
    )
    expect(onToggleAgent).toHaveBeenCalledWith("claude-code")
    expect(screen.getByRole("menu")).toBeInTheDocument()

    fireEvent.click(screen.getByRole("menuitemradio", { name: "Failed, 3 matching sessions" }))
    expect(onResultChange).toHaveBeenCalledWith("failing")
    expect(screen.getByRole("menu")).toBeInTheDocument()

    fireEvent.click(
      screen.getByRole("menuitemcheckbox", { name: "All agents, 8 matching sessions" }),
    )
    expect(onResetAgents).toHaveBeenCalledOnce()
    expect(screen.getByRole("menu")).toBeInTheDocument()
  })

  it("disables unselected zero-count options and skips them during keyboard navigation", async () => {
    const headerProps = props({
      counts: {
        ...COUNTS,
        result: { all: 6, failing: 0, passing: 0 },
        spend: { all: 5, notable: 0, material: 0 },
      },
    })
    render(<SessionFiltersHeader {...headerProps} />)
    openFilters()

    for (const label of ["Future Agent", "Failed", "Passed", "High cost", "$1 or more"]) {
      const item = screen.getByRole(
        label === "Future Agent" ? "menuitemcheckbox" : "menuitemradio",
        { name: `${label}, 0 matching sessions` },
      )
      expect(item).toHaveAttribute("aria-disabled", "true")
      fireEvent.click(item)
    }
    expect(headerProps.onToggleAgent).not.toHaveBeenCalled()
    expect(headerProps.onResultChange).not.toHaveBeenCalled()
    expect(headerProps.onSpendChange).not.toHaveBeenCalled()

    const codex = screen.getByRole("menuitemcheckbox", { name: "Codex, 2 matching sessions" })
    codex.focus()
    fireEvent.keyDown(codex, { key: "ArrowDown" })
    const allResults = screen.getByRole("menuitemradio", {
      name: "All results, 6 matching sessions",
    })
    await waitFor(() => expect(allResults).toHaveFocus())
    fireEvent.keyDown(allResults, { key: "ArrowDown" })
    await waitFor(() =>
      expect(
        screen.getByRole("menuitemradio", { name: "All costs, 5 matching sessions" }),
      ).toHaveFocus(),
    )
  })

  it("keeps selected options and every All option enabled when their counts are zero", () => {
    const headerProps = props({
      filters: { agents: ["future-agent"], result: "failing", spend: "notable" },
      counts: {
        ...COUNTS,
        matching: 0,
        agentsAll: 0,
        result: { all: 0, failing: 0, passing: 0 },
        spend: { all: 0, notable: 0, material: 0 },
      },
    })
    render(<SessionFiltersHeader {...headerProps} />)
    openFilters()

    for (const label of ["Future Agent", "Failed", "High cost"]) {
      const item = screen.getByRole(
        label === "Future Agent" ? "menuitemcheckbox" : "menuitemradio",
        { name: `${label}, 0 matching sessions` },
      )
      expect(item).toHaveAttribute("aria-checked", "true")
      expect(item).not.toHaveAttribute("aria-disabled", "true")
    }
    fireEvent.click(
      screen.getByRole("menuitemcheckbox", { name: "Future Agent, 0 matching sessions" }),
    )
    expect(headerProps.onToggleAgent).toHaveBeenCalledWith("future-agent")

    for (const label of ["All agents", "All results", "All costs"]) {
      const item = screen.getByRole(
        label === "All agents" ? "menuitemcheckbox" : "menuitemradio",
        { name: `${label}, 0 matching sessions` },
      )
      expect(item).not.toHaveAttribute("aria-disabled", "true")
      fireEvent.click(item)
    }
    expect(headerProps.onResetAgents).toHaveBeenCalledOnce()
    expect(headerProps.onResultChange).toHaveBeenCalledWith("all")
    expect(headerProps.onSpendChange).toHaveBeenCalledWith("all")
    expect(screen.getByRole("menu")).toBeInTheDocument()
  })

  it("closes on mouse exit without reopening the tooltip, and can reopen", async () => {
    render(<SessionFiltersHeader {...props()} />)
    const trigger = openFilters()
    const menu = screen.getByRole("menu")
    vi.spyOn(menu, "getBoundingClientRect").mockReturnValue(new DOMRect(100, 100, 200, 300))
    vi.spyOn(trigger, "getBoundingClientRect").mockReturnValue(new DOMRect(240, 70, 60, 22))
    fireEvent.pointerMove(menu, { pointerType: "mouse", clientX: 200, clientY: 200 })
    fireEvent.pointerMove(document.body, { pointerType: "mouse", clientX: 450, clientY: 200 })
    expect(screen.getByRole("menu")).toBeInTheDocument()
    await waitFor(() => expect(screen.queryByRole("menu")).not.toBeInTheDocument())
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 20))
    })
    expect(trigger).not.toHaveFocus()
    expect(document.querySelector(".ui-tooltip")).toBeNull()
    openFilters()
    expect(screen.getByRole("menu")).toBeInTheDocument()
  })

  it("restores the Filters trigger after Escape", async () => {
    render(<SessionFiltersHeader {...props()} />)
    const trigger = openFilters()
    fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" })
    await waitFor(() => expect(document.activeElement).toBe(trigger))
  })
})
