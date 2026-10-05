import { fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { FirstRunTakeover } from "./FirstRunTakeover"
import type { FlowStep, OverviewProgress } from "./overviewProgressStore"

let snapshot: OverviewProgress
const { showLiveLimits, skipLiveLimits, nextStep, enhanceFixes } = vi.hoisted(() => ({
  showLiveLimits: vi.fn(async () => undefined),
  skipLiveLimits: vi.fn(),
  nextStep: vi.fn(async () => undefined),
  enhanceFixes: vi.fn(async () => undefined),
}))

const platform = vi.hoisted(() => ({ macOS: true }))
const openChecks = vi.fn()

vi.mock("../../../lib/platform", () => ({ isMacOS: () => platform.macOS }))

vi.mock("./overviewProgressStore", () => ({
  subscribeOverviewProgress: () => () => undefined,
  overviewProgress: () => snapshot,
  showLiveLimits,
  skipLiveLimits,
  nextStep,
  enhanceFixes,
  enableNonRepoFolders: vi.fn(),
  fixesFound: (progress: OverviewProgress) =>
    progress.checks.windowSessions > 0 && progress.failingCount > 0,
  LIVE_LIMITS_TRANSITION_NAME: "progress-live-limits",
  progressStepTransitionName: (step: string) => `progress-step-${step}`,
  firstFailingCheck: (progress: OverviewProgress) =>
    progress.categories.find((category) => category.status === "needsFix")?.id,
}))

afterEach(() => vi.clearAllMocks())

function progress(flow: FlowStep, overrides: Partial<OverviewProgress> = {}): OverviewProgress {
  return {
    mode: "firstRun",
    flow,
    openStep: null,
    stepShown: true,
    agents: { done: false, rows: [] },
    sessions: {
      done: false,
      completed: 0,
      total: 0,
      gate: null,
      includeNonRepoFolders: false,
      deferred: [],
    },
    checks: { done: false, windowSessions: 0, pendingEvidence: 0, deferredEvidence: 0 },
    categories: [],
    failingCount: 0,
    history: null,
    ...overrides,
  }
}

describe("FirstRunTakeover's live-limits step", () => {
  it("shows the live-limits card without the welcome line", () => {
    snapshot = progress("limits")
    render(<FirstRunTakeover onOpenChecks={openChecks} />)
    expect(screen.queryByText("Welcome")).toBeNull()
    expect(screen.getByRole("heading", { name: "Show your plan limits" })).toBeInTheDocument()
  })

  it("mentions the Keychain prompt only on macOS", () => {
    snapshot = progress("limits")
    platform.macOS = true
    const { unmount } = render(<FirstRunTakeover onOpenChecks={openChecks} />)
    expect(screen.getByText(/Keychain access/)).toBeInTheDocument()
    unmount()
    platform.macOS = false
    render(<FirstRunTakeover onOpenChecks={openChecks} />)
    expect(screen.queryByText(/Keychain access/)).toBeNull()
    platform.macOS = true
  })

  it("calls showLiveLimits when Turn on live limits is clicked", async () => {
    snapshot = progress("limits")
    render(<FirstRunTakeover onOpenChecks={openChecks} />)
    fireEvent.click(screen.getByRole("button", { name: "Turn on live limits" }))
    await vi.waitFor(() => expect(showLiveLimits).toHaveBeenCalledTimes(1))
    expect(skipLiveLimits).not.toHaveBeenCalled()
  })

  it("shows an error line when showLiveLimits fails, without calling skipLiveLimits", async () => {
    showLiveLimits.mockRejectedValueOnce(new Error("denied"))
    snapshot = progress("limits")
    render(<FirstRunTakeover onOpenChecks={openChecks} />)
    fireEvent.click(screen.getByRole("button", { name: "Turn on live limits" }))
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Could not start live limits. Try again.",
    )
    expect(skipLiveLimits).not.toHaveBeenCalled()
  })

  it("calls skipLiveLimits when Skip is clicked, without calling showLiveLimits", () => {
    snapshot = progress("limits")
    render(<FirstRunTakeover onOpenChecks={openChecks} />)
    fireEvent.click(screen.getByRole("button", { name: "Skip and turn on in settings later" }))
    expect(skipLiveLimits).toHaveBeenCalledTimes(1)
    expect(showLiveLimits).not.toHaveBeenCalled()
  })
})

describe("FirstRunTakeover's step cards", () => {
  it("shows the welcome step first, with an enabled Get Started that calls nextStep", () => {
    snapshot = progress("welcome")
    render(<FirstRunTakeover onOpenChecks={openChecks} />)
    expect(screen.getByRole("heading", { name: "Welcome" })).toBeInTheDocument()
    expect(screen.queryByRole("heading", { name: "Finding agents" })).toBeNull()
    fireEvent.click(screen.getByRole("button", { name: "Get Started" }))
    expect(nextStep).toHaveBeenCalledTimes(1)
  })

  it("renders only the Agents step's card, without the welcome, while flow is agents", () => {
    snapshot = progress("agents")
    render(<FirstRunTakeover onOpenChecks={openChecks} />)
    expect(screen.queryByText("Welcome")).toBeNull()
    expect(screen.getByRole("heading", { name: "Finding agents" })).toBeInTheDocument()
    expect(screen.queryByRole("heading", { name: "Reading sessions" })).toBeNull()
  })

  it("disables Next until the Agents step is done, then calls nextStep on click", () => {
    snapshot = progress("agents", { agents: { done: false, rows: [] } })
    const { rerender } = render(<FirstRunTakeover onOpenChecks={openChecks} />)
    expect(screen.getByRole("button", { name: "Next" })).toBeDisabled()

    snapshot = progress("agents", { agents: { done: true, rows: [] } })
    rerender(<FirstRunTakeover onOpenChecks={openChecks} />)
    const next = screen.getByRole("button", { name: "Next" })
    expect(next).not.toBeDisabled()
    fireEvent.click(next)
    expect(nextStep).toHaveBeenCalledTimes(1)
  })

  it("disables Next until the Sessions step is done", () => {
    snapshot = progress("sessions", {
      sessions: {
        done: false,
        completed: 1,
        total: 10,
        gate: null,
        includeNonRepoFolders: false,
        deferred: [],
      },
    })
    render(<FirstRunTakeover onOpenChecks={openChecks} />)
    expect(screen.getByRole("button", { name: "Next" })).toBeDisabled()
  })

  it("disables Next until the Checks step is done", () => {
    snapshot = progress("checks", {
      checks: { done: false, windowSessions: 10, pendingEvidence: 1, deferredEvidence: 0 },
    })
    render(<FirstRunTakeover onOpenChecks={openChecks} />)
    expect(screen.getByRole("button", { name: "Next" })).toBeDisabled()
  })

  it("labels the fixes step's button Done, and leaves it enabled", () => {
    snapshot = progress("fixes", {
      checks: { done: true, windowSessions: 0, pendingEvidence: 0, deferredEvidence: 0 },
    })
    render(<FirstRunTakeover onOpenChecks={openChecks} />)
    const done = screen.getByRole("button", { name: "Done" })
    expect(done).not.toBeDisabled()
    fireEvent.click(done)
    expect(nextStep).toHaveBeenCalledTimes(1)
  })

  it("finishes the first run on Enhance, then opens the first check that needs a fix", async () => {
    snapshot = progress("fixes", {
      checks: { done: true, windowSessions: 5, pendingEvidence: 0, deferredEvidence: 0 },
      failingCount: 2,
      categories: [
        { id: "cacheChurn", label: "Excess cache rehydration", status: "passing" },
        { id: "unusedSkills", label: "Unused skills", status: "needsFix" },
        { id: "unusedMcpServers", label: "Unused MCP servers", status: "needsFix" },
      ],
    })
    render(<FirstRunTakeover onOpenChecks={openChecks} />)
    expect(screen.queryByRole("button", { name: "Done" })).toBeNull()
    fireEvent.click(screen.getByRole("button", { name: "Enhance" }))
    expect(enhanceFixes).toHaveBeenCalledTimes(1)
    expect(nextStep).not.toHaveBeenCalled()
    await vi.waitFor(() => expect(openChecks).toHaveBeenCalledWith("unusedSkills"))
  })

  it("finishes the first run on Skip without opening Burn Checks", () => {
    snapshot = progress("fixes", {
      checks: { done: true, windowSessions: 5, pendingEvidence: 0, deferredEvidence: 0 },
      failingCount: 1,
      categories: [{ id: "unusedSkills", label: "Unused skills", status: "needsFix" }],
    })
    render(<FirstRunTakeover onOpenChecks={openChecks} />)
    fireEvent.click(screen.getByRole("button", { name: /skip/i }))
    expect(nextStep).toHaveBeenCalledTimes(1)
    expect(enhanceFixes).not.toHaveBeenCalled()
    expect(openChecks).not.toHaveBeenCalled()
  })
})
