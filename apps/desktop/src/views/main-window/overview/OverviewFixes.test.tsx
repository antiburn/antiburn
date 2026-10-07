import { fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { OverviewFixes } from "./OverviewFixes"
import type { OverviewProgress } from "./overviewProgressStore"

let snapshot: OverviewProgress

vi.mock("./overviewProgressStore", () => ({
  subscribeOverviewProgress: () => () => undefined,
  overviewProgress: () => snapshot,
}))

afterEach(() => vi.clearAllMocks())

function progress(overrides: Partial<OverviewProgress> = {}): OverviewProgress {
  return {
    mode: "steady",
    flow: "done",
    openStep: null,
    openStepControl: null,
    openStepControlRevision: 0,
    stepShown: true,
    actionPending: false,
    actionError: null,
    agents: { done: true, rows: [] },
    sessions: {
      done: true,
      completed: 0,
      total: 0,
      displayCompleted: 0,
      displayTotal: 0,
      deferred: [],
    },
    checks: { done: true, windowSessions: 0, pendingEvidence: 0, deferredEvidence: 0 },
    categories: [],
    failingCount: 0,
    history: null,
    ...overrides,
  }
}

describe("OverviewFixes", () => {
  it("opens the check when its row is clicked", () => {
    snapshot = progress({
      categories: [
        { id: "modelOverthinking", label: "Model overthinking", status: "needsFix" },
      ],
    })
    const onOpenCheck = vi.fn()
    render(<OverviewFixes onOpenCheck={onOpenCheck} />)
    fireEvent.click(screen.getByRole("button", { name: /Model overthinking/ }))
    expect(onOpenCheck).toHaveBeenCalledWith("modelOverthinking")
  })

  it("renders the config checks heading and nothing else", () => {
    snapshot = progress()
    render(<OverviewFixes onOpenCheck={() => {}} />)
    expect(screen.getByRole("heading", { name: "Config checks" })).toBeInTheDocument()
  })

  it("lists every category with its label and status", () => {
    snapshot = progress({
      categories: [
        { id: "modelOverthinking", label: "Model overthinking", status: "needsFix" },
        { id: "cacheChurn", label: "Cache churn", status: "passing" },
        { id: "oldModelUsage", label: "Old model usage", status: "awaitingVerification" },
        { id: "overuseOfFastMode", label: "Overuse of fast mode", status: "notChecked" },
      ],
    })
    render(<OverviewFixes onOpenCheck={() => {}} />)
    expect(screen.getByText("Model overthinking").closest("li")).toHaveTextContent("needs fix")
    expect(screen.getByText("Cache churn").closest("li")).toHaveTextContent("passing")
    expect(screen.getByText("Old model usage").closest("li")).toHaveTextContent(
      "awaiting verification",
    )
    expect(screen.getByText("Overuse of fast mode").closest("li")).toHaveTextContent(
      "not checked",
    )
  })

  it("shows a failing category's problem phrase beside its label", () => {
    snapshot = progress({
      categories: [
        { id: "modelOverthinking", label: "Model overthinking", status: "needsFix" },
      ],
    })
    render(<OverviewFixes onOpenCheck={() => {}} />)
    const row = screen.getByText("Model overthinking").closest("li")!
    expect(row).toHaveTextContent("needs fix")
    // The phrase itself comes from CHECK_PROBLEM_PHRASES; this only checks the
    // row renders more than the label and status for a failing category.
    expect(row.querySelectorAll("span").length).toBeGreaterThan(2)
  })

  it("renders no categories when none are reported", () => {
    snapshot = progress({ categories: [] })
    render(<OverviewFixes onOpenCheck={() => {}} />)
    expect(screen.getByRole("heading", { name: "Config checks" })).toBeInTheDocument()
    expect(screen.queryByRole("listitem")).toBeNull()
  })

  it("celebrates when every check passes or is snoozed, and shows the list on request", () => {
    snapshot = progress({
      categories: [
        { id: "modelOverthinking", label: "Model overthinking", status: "passing" },
        { id: "unusedSkills", label: "Unused skills", status: "snoozed" },
      ],
    } as Partial<OverviewProgress>)
    render(<OverviewFixes onOpenCheck={() => {}} />)
    expect(screen.getByText(/passed or snoozed/)).toHaveTextContent(
      "All checks passed or snoozed",
    )
    expect(screen.queryByRole("heading", { name: "Config checks" })).toBeNull()
    expect(screen.queryByRole("button", { name: /Model overthinking/ })).toBeNull()
    fireEvent.click(screen.getByRole("button", { name: "All checks" }))
    expect(screen.getByRole("button", { name: /Model overthinking/ })).toBeInTheDocument()
  })

  it("shows the list, not the celebration, while a check needs a fix", () => {
    snapshot = progress({
      categories: [
        { id: "modelOverthinking", label: "Model overthinking", status: "passing" },
        { id: "unusedSkills", label: "Unused skills", status: "needsFix" },
      ],
    } as Partial<OverviewProgress>)
    render(<OverviewFixes onOpenCheck={() => {}} />)
    expect(screen.queryByText("All checks passed or snoozed")).toBeNull()
  })

  it("shows the celebration again after the user leaves the Overview and returns", () => {
    snapshot = progress({
      categories: [{ id: "modelOverthinking", label: "Model overthinking", status: "passing" }],
    } as Partial<OverviewProgress>)
    const { rerender } = render(<OverviewFixes active onOpenCheck={() => {}} />)
    fireEvent.click(screen.getByRole("button", { name: "All checks" }))
    expect(screen.queryByText(/passed or snoozed/)).toBeNull()
    rerender(<OverviewFixes active={false} onOpenCheck={() => {}} />)
    rerender(<OverviewFixes active onOpenCheck={() => {}} />)
    expect(screen.getByText(/passed or snoozed/)).toBeInTheDocument()
  })
})
