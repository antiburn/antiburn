import { render, screen } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

import { ProgressStepCard } from "./ProgressSteps"
import type { OverviewProgress } from "./overviewProgressStore"

vi.mock("../../../lib/useFolderPermissionFlow", () => ({
  useFolderPermissionFlow: () => ({
    phase: "idle",
    current: null,
    position: 0,
    total: 0,
    recordedDenials: [],
    start: vi.fn(),
  }),
}))

function progress(): OverviewProgress {
  return {
    mode: "firstRun",
    flow: "sessions",
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
  }
}

describe("ProgressStepCard empty progress", () => {
  it("shows a completed empty Sessions pass without a 0/0 progress value", () => {
    render(
      <ProgressStepCard
        step="sessions"
        surface="firstRun"
        progress={progress()}
        isSteady={false}
        transitionName={undefined}
      />,
    )

    expect(screen.getByText("No sessions found")).toBeVisible()
    expect(screen.getByRole("progressbar", { name: "Read session data" })).toHaveAttribute(
      "aria-valuenow",
      "100",
    )
    expect(screen.queryByText("0/0")).toBeNull()
  })

  it("shows a completed empty Checks pass with its own copy", () => {
    render(
      <ProgressStepCard
        step="checks"
        surface="firstRun"
        progress={progress()}
        isSteady={false}
        transitionName={undefined}
      />,
    )

    expect(screen.getByText("No sessions to check")).toBeVisible()
    expect(screen.getByRole("progressbar", { name: "Run session checks" })).toHaveAttribute(
      "aria-valuenow",
      "100",
    )
  })
})
