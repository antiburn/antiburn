import { fireEvent, render, screen } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

import type { BurnCheckDetectorId, ChecksCategoryPayload } from "../../lib/insightsIpc"
import type { ChecksPresentation } from "../../lib/presentation/checks"
import { ChecksPeek, ChecksSummary } from "./ChecksView"

function category(
  id: BurnCheckDetectorId,
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
  it("opens the Burn checks companion on hover and focus", () => {
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

    expect(screen.getByText("16% token burn").closest(".text-system-red-text")).not.toBeNull()
    expect(screen.queryByText("Last 30 days")).not.toBeInTheDocument()
    const trigger = screen.getByRole("button", { name: /Burn checks/ })
    fireEvent.mouseEnter(trigger)
    fireEvent.focus(trigger)
    expect(onPreview).toHaveBeenCalledTimes(2)
    expect(onPreview).toHaveBeenLastCalledWith({ top: 0, height: 0 })
  })

  it("shows the aggregate token burn estimate in the main popover", () => {
    const { container } = render(
      <ChecksSummary
        active={false}
        presentation={presentation}
        reportUnavailable={false}
        onPreview={vi.fn()}
        onLeave={vi.fn()}
      />,
    )

    expect(screen.getByText("16% token burn")).toBeInTheDocument()
    expect(container.querySelectorAll(".text-roll")).toHaveLength(1)
  })

  it("shows a positive sub-percent fallback without rounding it to zero", () => {
    render(
      <ChecksSummary
        active={false}
        presentation={{ ...presentation, estimate: { tokenBurnBasisPoints: 1 } }}
        reportUnavailable={false}
        onPreview={vi.fn()}
        onLeave={vi.fn()}
      />,
    )

    expect(screen.getByText("<1% token burn").closest(".text-system-yellow")).not.toBeNull()
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
    const trigger = screen.getByRole("button", { name: /Burn checks/ })
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
    const summary = screen.getByRole("button", { name: /Burn checks/ })
    expect(summary).toBeDisabled()
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

  it("shows floored token burn estimates and every confirmed pass in preview mode", () => {
    const { container } = render(<ChecksPeek presentation={presentation} />)
    expect(screen.getByText("16% token burn")).toBeInTheDocument()
    expect(screen.getByText("12% token burn")).toBeInTheDocument()
    expect(container.querySelectorAll(".text-roll")).toHaveLength(2)
    expect(screen.getByText(/1 check failed/)).toBeInTheDocument()
    expect(screen.getByText("7/11 sessions failed")).toBeInTheDocument()
    expect(screen.getByText(/More evidence is needed/)).toBeInTheDocument()
    expect(screen.getByText("Passed checks")).toHaveClass("text-label-tertiary")
    expect(
      screen.queryByText("Estimated share of tokens spent on avoidable work."),
    ).not.toBeInTheDocument()
    expect(screen.getByText("Failed checks").nextElementSibling).toHaveClass("mt-2")
    expect(screen.getByText("Passed checks").nextElementSibling).toHaveClass("mt-2")
    expect(container.querySelector(".lucide-flame")).toBeInTheDocument()
    expect(container.querySelector(".lucide-circle-x")).not.toBeInTheDocument()
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
    expect(screen.queryByText("Passed")).not.toBeInTheDocument()
    expect(screen.getAllByText("12 passed")).toHaveLength(4)
    expect(screen.queryByRole("button")).not.toBeInTheDocument()
  })

  it("renders every shared per-check metric in the anchored companion", () => {
    const failures = [
      category("sessionsOverDepth", {
        finding: 1,
        clean: 0,
        estimatedTokenBurnBasisPoints: 800,
      }),
      category("modelOverthinking", {
        finding: 1,
        clean: 0,
        estimatedTokenBurnBasisPoints: 350,
      }),
      category("overpoweredSubagents", {
        finding: 1,
        clean: 0,
        estimatedTokenBurnBasisPoints: 880,
      }),
      category("unusedMcpServers", {
        finding: 1,
        clean: 0,
        estimatedTokenBurnBasisPoints: 100,
      }),
      category("unusedBuiltInTools", {
        finding: 1,
        clean: 0,
        estimatedTokenBurnBasisPoints: 1,
      }),
      category("unusedSkills", { finding: 1, clean: 0, estimatedTokenBurnBasisPoints: 100 }),
      category("oldModelUsage", { finding: 1, clean: 0, estimatedTokenBurnBasisPoints: 400 }),
      category("overuseOfFastMode", {
        finding: 1,
        clean: 0,
        estimatedTokenBurnBasisPoints: 333,
      }),
      category("cacheChurn", { finding: 1, clean: 0, estimatedTokenBurnBasisPoints: 700 }),
    ]

    render(
      <ChecksPeek
        presentation={{
          ...presentation,
          failures,
          wins: [],
          estimate: { tokenBurnBasisPoints: 880 },
        }}
      />,
    )

    for (const metric of [
      "8% token burn",
      "3% token burn",
      "8% token burn",
      "1% token burn",
      "<1% token burn",
      "1% token burn",
      "4% token burn",
      "3% token burn",
      "7% token burn",
    ]) {
      expect(screen.getAllByText(metric).length).toBeGreaterThan(0)
    }
  })

  it("animates changing summary and anchored token estimates", () => {
    const { container, rerender } = render(
      <ChecksSummary
        active={false}
        presentation={presentation}
        reportUnavailable={false}
        onPreview={vi.fn()}
        onLeave={vi.fn()}
      />,
    )
    expect(container.querySelector(".text-roll-sr")).toHaveTextContent("16% token burn")

    rerender(
      <ChecksSummary
        active={false}
        presentation={{ ...presentation, estimate: { tokenBurnBasisPoints: 1_500 } }}
        reportUnavailable={false}
        onPreview={vi.fn()}
        onLeave={vi.fn()}
      />,
    )
    expect(container.querySelector(".text-roll-sr")).toHaveTextContent("15% token burn")
    expect(container.querySelectorAll(".text-roll-in").length).toBeGreaterThan(0)

    rerender(
      <ChecksPeek
        presentation={{
          ...presentation,
          failures: [{ ...failure, finding: 1, clean: 0, estimatedTokenBurnBasisPoints: 500 }],
          estimate: { tokenBurnBasisPoints: 750 },
        }}
      />,
    )
    expect(screen.getByText("7% token burn")).toBeInTheDocument()
    expect(screen.getByText("5% token burn")).toBeInTheDocument()
    expect(screen.getByText("1/1 session failed")).toBeInTheDocument()
    expect(container.querySelectorAll(".text-roll")).toHaveLength(2)

    rerender(
      <ChecksPeek
        presentation={{
          ...presentation,
          failures: [{ ...failure, finding: 1, clean: 0, estimatedTokenBurnBasisPoints: 625 }],
          estimate: { tokenBurnBasisPoints: 875 },
        }}
      />,
    )
    expect(screen.getByText("8% token burn")).toBeInTheDocument()
    expect(screen.getByText("6% token burn")).toBeInTheDocument()
    for (const estimate of container.querySelectorAll(".text-roll")) {
      expect(estimate.querySelector(".text-roll-in")).not.toBeNull()
    }
  })

  it("uses passed wording for complete and partial results", () => {
    const { container, rerender } = render(
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
    expect(screen.getByText("All checks passed")).toHaveClass("text-label")
    const passingIcon = container.querySelector(".lucide-circle-check")
    expect(passingIcon).toBeInTheDocument()
    expect(passingIcon).toHaveAttribute("width", "24")
    expect(passingIcon?.parentElement).toHaveClass("text-system-green")

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
    expect(screen.getByText("More evidence is needed")).toBeInTheDocument()
    expect(screen.queryByText("Passed")).not.toBeInTheDocument()
    expect(screen.queryByText("Passed where assessed")).not.toBeInTheDocument()
    const row = screen.getByText("Session overdepth").closest(".grid")
    expect(row).toHaveTextContent("8 passed")
    expect(row).not.toHaveTextContent("need evidence")
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
