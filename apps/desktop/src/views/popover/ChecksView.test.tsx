import { fireEvent, render, screen } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

import type { ChecksCategoryPayload } from "../../lib/insightsIpc"
import type { ChecksPresentation } from "../../lib/presentation/checks"
import { ChecksPeek, ChecksSummary } from "./ChecksView"

function category(
  id: string,
  overrides: Partial<ChecksCategoryPayload> = {},
): ChecksCategoryPayload {
  return {
    id,
    finding: 0,
    clean: 12,
    unavailable: 0,
    estimatedTokenBurnBasisPoints: 0,
    ...overrides,
  }
}

const failure = category("cacheChurn", {
  finding: 7,
  clean: 4,
  unavailable: 3,
  estimatedTokenBurnBasisPoints: 1_250,
})

const wins = [
  category("sessionsOverDepth"),
  category("modelOverthinking"),
  category("unusedMcpServers"),
  category("oldModelUsage"),
]

const presentation: ChecksPresentation = {
  failures: [failure],
  wins,
  unavailable: [],
  refreshUnavailable: false,
  estimate: { tokenBurnBasisPoints: 1_625 },
}

describe("Checks", () => {
  it("opens the All checks companion on hover and focus", () => {
    const onPreview = vi.fn()
    render(
      <ChecksSummary
        active={false}
        presentation={presentation}
        reportUnavailable={false}
        onPreview={onPreview}
        onLeave={vi.fn()}
      />,
    )

    expect(screen.getByText("~17% token burn")).toHaveClass("text-system-red-text")
    expect(screen.queryByText("Last 30 days")).not.toBeInTheDocument()
    const trigger = screen.getByText("All checks").closest("[tabindex]")!
    fireEvent.mouseEnter(trigger)
    fireEvent.focus(trigger)
    expect(onPreview).toHaveBeenCalledTimes(2)
    expect(onPreview).toHaveBeenLastCalledWith({ top: 0, height: 0 })
  })

  it("conceals only after both hover and focus leave", () => {
    const onLeave = vi.fn()
    const { container } = render(
      <ChecksSummary
        active
        presentation={presentation}
        reportUnavailable={false}
        onPreview={vi.fn()}
        onLeave={onLeave}
      />,
    )
    const summary = container.firstElementChild!
    const trigger = screen.getByText("All checks").closest("[tabindex]")!
    fireEvent.mouseEnter(summary)
    fireEvent.focus(trigger)
    fireEvent.mouseLeave(summary)
    expect(onLeave).not.toHaveBeenCalled()

    fireEvent.blur(trigger)
    expect(onLeave).toHaveBeenCalledOnce()
  })

  it("keeps the token burn definition out of the main popover", () => {
    render(
      <ChecksSummary
        active={false}
        presentation={presentation}
        reportUnavailable={false}
        onPreview={vi.fn()}
        onLeave={vi.fn()}
      />,
    )
    expect(screen.queryByText(/Token burn estimates/)).not.toBeInTheDocument()
  })

  it("does not focus the summary while the report loads", () => {
    render(
      <ChecksSummary
        active={false}
        presentation={null}
        reportUnavailable={false}
        onPreview={vi.fn()}
        onLeave={vi.fn()}
      />,
    )
    const summary = screen.getByText("All checks").parentElement?.parentElement
    expect(summary).toHaveAttribute("aria-disabled", "true")
    expect(summary).not.toHaveAttribute("tabindex")
  })

  it("ends the loading state when the report is unavailable", () => {
    render(
      <ChecksSummary
        active={false}
        presentation={null}
        reportUnavailable
        onPreview={vi.fn()}
        onLeave={vi.fn()}
      />,
    )
    expect(screen.getByText("Checks unavailable")).toBeInTheDocument()
    expect(screen.queryByText("Checking local sessions…")).not.toBeInTheDocument()
  })

  it("shows whole token burn estimates and every confirmed pass in preview mode", () => {
    render(<ChecksPeek presentation={presentation} />)
    expect(screen.getByText("~17% token burn")).toBeInTheDocument()
    expect(screen.getByText("~13% token burn")).toBeInTheDocument()
    expect(screen.queryByText("1 check failed")).not.toBeInTheDocument()
    expect(
      screen.getByText("7 failed sessions · 4 passed · 3 need evidence"),
    ).toBeInTheDocument()
    expect(screen.getByText("Passed checks")).toHaveClass("text-label-tertiary")
    const explainer = screen.getByText("Estimated share of tokens spent on avoidable work.")
    expect(explainer).toBeInTheDocument()
    expect(explainer.closest("section")).toHaveClass("bg-surface-card")
    expect(screen.getByText("Failed checks").nextElementSibling).toHaveClass("mt-2")
    expect(screen.getByText("Passed checks").nextElementSibling).toHaveClass("mt-2")
    for (const label of [
      "Session overdepth",
      "Model overthinking",
      "Unused MCP servers",
      "Old model usage",
    ]) {
      const row = screen.getByText(label).closest(".grid")!
      expect(row).toBeInTheDocument()
      expect(row.firstElementChild).toHaveClass("bg-system-green/10", "text-system-green")
    }
    expect(screen.getAllByText("Passed")).toHaveLength(4)
    expect(screen.queryByRole("button")).not.toBeInTheDocument()
  })

  it("uses passed wording for complete and partial results", () => {
    const { rerender } = render(
      <ChecksPeek
        presentation={{
          failures: [],
          wins,
          unavailable: [],
          refreshUnavailable: false,
          estimate: { tokenBurnBasisPoints: 0 },
        }}
      />,
    )
    expect(screen.getByText("All checks passed")).toBeInTheDocument()

    rerender(
      <ChecksPeek
        presentation={{
          failures: [],
          wins: [category("sessionsOverDepth", { clean: 8, unavailable: 4 })],
          unavailable: [],
          refreshUnavailable: false,
          estimate: { tokenBurnBasisPoints: 0 },
        }}
      />,
    )
    expect(screen.getByText("No issues found where assessed")).toBeInTheDocument()
    expect(screen.getByText("Passed")).toBeInTheDocument()
    expect(screen.queryByText("Passed where assessed")).not.toBeInTheDocument()
  })

  it("does not show unavailable checks", () => {
    const unavailable = category("unusedBuiltInTools", {
      clean: 0,
      unavailable: 12,
      estimatedTokenBurnBasisPoints: null,
    })
    render(<ChecksPeek presentation={{ ...presentation, unavailable: [unavailable] }} />)
    expect(screen.queryByText("Unavailable checks")).not.toBeInTheDocument()
    expect(screen.queryByText("Unused built-in tools")).not.toBeInTheDocument()
    expect(screen.queryByRole("button")).not.toBeInTheDocument()
    expect(
      screen.queryByText("Session sources do not record this evidence"),
    ).not.toBeInTheDocument()
  })
})
