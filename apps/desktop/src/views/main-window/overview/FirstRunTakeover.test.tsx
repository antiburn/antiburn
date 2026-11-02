import { fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { FirstRunTakeover } from "./FirstRunTakeover"
import type * as ProgressStore from "./overviewProgressStore"
import type { FlowStep, OverviewProgress } from "./overviewProgressStore"

let snapshot: OverviewProgress
const { showLiveLimits, skipLiveLimits, nextStep, previousStep } = vi.hoisted(() => ({
  showLiveLimits: vi.fn(async () => undefined),
  skipLiveLimits: vi.fn(),
  previousStep: vi.fn(),
  nextStep: vi.fn(async () => undefined),
}))

const platform = vi.hoisted(() => ({ macOS: true }))

vi.mock("../../../lib/platform", () => ({ isMacOS: () => platform.macOS }))

// Each step component's own tests cover its IPC-heavy settings; here a
// stand-in proves the first-run disclosure shows and hides them, without
// pulling Tauri-backed sessions into this file.
vi.mock("./stepSettings/StepSettings", () => ({
  StepSettings: ({ step }: { step: string }) => <div data-testid="step-settings">{step}</div>,
}))

vi.mock("./overviewProgressStore", async (importOriginal) => ({
  ...(await importOriginal<typeof ProgressStore>()),
  subscribeOverviewProgress: () => () => undefined,
  overviewProgress: () => snapshot,
  showLiveLimits,
  skipLiveLimits,
  previousStep,
  nextStep,
  LIVE_LIMITS_TRANSITION_NAME: "progress-live-limits",
  progressStepTransitionName: (step: string) => `progress-step-${step}`,
}))

afterEach(() => vi.clearAllMocks())

function progress(flow: FlowStep, overrides: Partial<OverviewProgress> = {}): OverviewProgress {
  return {
    mode: "firstRun",
    flow,
    stepShown: true,
    actionPending: false,
    actionError: null,
    agents: { done: false, rows: [] },
    sessions: {
      done: false,
      completed: 0,
      total: 0,
      displayCompleted: 0,
      displayTotal: 0,
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
    render(<FirstRunTakeover />)
    expect(screen.queryByText("Welcome to antiburn")).toBeNull()
    expect(screen.getByRole("heading", { name: "Plan limits" })).toBeInTheDocument()
  })

  it("mentions the Keychain prompt only on macOS", () => {
    snapshot = progress("limits")
    platform.macOS = true
    const { unmount } = render(<FirstRunTakeover />)
    expect(screen.getByText(/Keychain access/)).toBeInTheDocument()
    unmount()
    platform.macOS = false
    render(<FirstRunTakeover />)
    expect(screen.queryByText(/Keychain access/)).toBeNull()
    platform.macOS = true
  })

  it("calls showLiveLimits when Turn on limits is clicked", async () => {
    snapshot = progress("limits")
    render(<FirstRunTakeover />)
    fireEvent.click(screen.getByRole("button", { name: "Turn on limits" }))
    await vi.waitFor(() => expect(showLiveLimits).toHaveBeenCalledTimes(1))
    expect(skipLiveLimits).not.toHaveBeenCalled()
  })

  it("shows an error line when showLiveLimits fails, without calling skipLiveLimits", async () => {
    showLiveLimits.mockRejectedValueOnce(new Error("denied"))
    snapshot = progress("limits")
    render(<FirstRunTakeover />)
    fireEvent.click(screen.getByRole("button", { name: "Turn on limits" }))
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Could not start live limits. Try again.",
    )
    expect(skipLiveLimits).not.toHaveBeenCalled()
  })

  it("calls skipLiveLimits when Skip is clicked, without calling showLiveLimits", () => {
    snapshot = progress("limits")
    render(<FirstRunTakeover />)
    fireEvent.click(screen.getByRole("button", { name: "Skip until later" }))
    expect(skipLiveLimits).toHaveBeenCalledTimes(1)
    expect(showLiveLimits).not.toHaveBeenCalled()
  })

  it("keeps its settings hidden until Show settings is clicked", () => {
    snapshot = progress("limits")
    render(<FirstRunTakeover />)
    expect(screen.queryByTestId("step-settings")).toBeNull()
    fireEvent.click(screen.getByRole("button", { name: "Expand settings" }))
    expect(screen.getByTestId("step-settings")).toHaveTextContent("limits")
  })
})

describe("FirstRunTakeover's step cards", () => {
  it("shows the welcome step first, with an enabled Get Started that calls nextStep", () => {
    snapshot = progress("welcome")
    render(<FirstRunTakeover />)
    expect(screen.getByRole("heading", { name: "Welcome to antiburn" })).toBeInTheDocument()
    expect(screen.queryByRole("heading", { name: "Finding agents…" })).toBeNull()
    fireEvent.click(screen.getByRole("button", { name: "Get Started" }))
    expect(nextStep).toHaveBeenCalledTimes(1)
  })

  it("renders only the Agents step's card, without the welcome, while flow is agents", () => {
    snapshot = progress("agents")
    render(<FirstRunTakeover />)
    expect(screen.queryByText("Welcome to antiburn")).toBeNull()
    expect(screen.getByRole("heading", { name: "Finding agents…" })).toBeInTheDocument()
    expect(screen.queryByRole("heading", { name: "Reading sessions" })).toBeNull()
  })

  it("disables Next until the Agents step is done, then calls nextStep on click", () => {
    snapshot = progress("agents", { agents: { done: false, rows: [] } })
    const { rerender } = render(<FirstRunTakeover />)
    expect(screen.getByRole("button", { name: "Next" })).toBeDisabled()

    snapshot = progress("agents", { agents: { done: true, rows: [] } })
    rerender(<FirstRunTakeover />)
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
        displayCompleted: 1,
        displayTotal: 10,
        deferred: [],
      },
    })
    render(<FirstRunTakeover />)
    expect(screen.getByRole("button", { name: "Next" })).toBeDisabled()
  })

  it("disables Next until the Checks step is done", () => {
    snapshot = progress("checks", {
      checks: { done: false, windowSessions: 10, pendingEvidence: 1, deferredEvidence: 0 },
    })
    render(<FirstRunTakeover />)
    expect(screen.getByRole("button", { name: "Next" })).toBeDisabled()
  })

  it("fills the Checks bar when only a deferred live session remains", () => {
    snapshot = progress("checks", {
      sessions: { ...progress("checks").sessions, done: true },
      checks: { done: true, windowSessions: 144, pendingEvidence: 1, deferredEvidence: 1 },
    })
    render(<FirstRunTakeover />)
    expect(screen.getByRole("progressbar")).toHaveAttribute("aria-valuenow", "100")
  })

  it.each([0, 2])("finishes setup with Get Started when %i issues exist", (failingCount) => {
    snapshot = progress("fixes", { failingCount })
    render(<FirstRunTakeover />)
    expect(screen.getByRole("heading", { name: "You're all set!" })).toBeVisible()
    expect(screen.getAllByRole("button")).toHaveLength(2)
    expect(screen.getByRole("button", { name: "Back" })).toBeEnabled()
    fireEvent.click(screen.getByRole("button", { name: "Get Started" }))
    expect(nextStep).toHaveBeenCalledOnce()
  })

  it("offers no settings disclosure on the Fixes step", () => {
    snapshot = progress("fixes", {
      checks: { done: true, windowSessions: 0, pendingEvidence: 0, deferredEvidence: 0 },
    })
    render(<FirstRunTakeover />)
    expect(screen.queryByRole("button", { name: "Expand settings" })).toBeNull()
  })

  it("shows the Agents step's settings when Show settings is clicked, and hides them again", () => {
    snapshot = progress("agents", { agents: { done: true, rows: [] } })
    render(<FirstRunTakeover />)
    expect(screen.queryByTestId("step-settings")).toBeNull()

    const toggle = screen.getByRole("button", { name: "Expand settings" })
    expect(toggle).toHaveAttribute("aria-expanded", "false")
    fireEvent.click(toggle)

    expect(screen.getByTestId("step-settings")).toHaveTextContent("agents")
    const hide = screen.getByRole("button", { name: "Hide settings" })
    expect(hide).toHaveAttribute("aria-expanded", "true")

    fireEvent.click(hide)
    expect(screen.queryByTestId("step-settings")).toBeNull()
  })
})

it.each(["welcome", "fixes"] as const)(
  "shows failed setup feedback and disables actions while %s is pending",
  (flow) => {
    snapshot = progress(flow, { actionPending: true })
    const { rerender } = render(<FirstRunTakeover />)
    expect(screen.getByRole("button", { name: "Get Started" })).toBeDisabled()
    snapshot = progress(flow, { actionError: "Could not continue setup. Try again." })
    rerender(<FirstRunTakeover />)
    expect(screen.getByRole("alert")).toHaveTextContent("Could not continue setup")
    expect(screen.getByRole("button", { name: "Get Started" })).toBeEnabled()
  },
)

it.each(["agents", "limits", "sessions", "checks", "fixes"] as const)(
  "offers Back from %s",
  (flow) => {
    snapshot = progress(flow)
    render(<FirstRunTakeover />)
    fireEvent.click(screen.getByRole("button", { name: "Back" }))
    expect(previousStep).toHaveBeenCalledOnce()
  },
)

describe("FirstRunTakeover empty progress", () => {
  it("shows a completed empty Sessions pass without a 0/0 progress value", () => {
    snapshot = progress("sessions", {
      sessions: { ...progress("sessions").sessions, done: true },
    })
    render(<FirstRunTakeover />)
    expect(screen.getByText("No sessions found")).toBeVisible()
    expect(screen.getByRole("progressbar", { name: "Read session data" })).toHaveAttribute(
      "aria-valuenow",
      "100",
    )
    expect(screen.queryByText("0/0")).toBeNull()
  })

  it("shows a completed empty Checks pass with its own copy", () => {
    snapshot = progress("checks", {
      sessions: { ...progress("checks").sessions, done: true },
      checks: { done: true, windowSessions: 0, pendingEvidence: 0, deferredEvidence: 0 },
    })
    render(<FirstRunTakeover />)
    expect(screen.getByText("No sessions to check")).toBeVisible()
    expect(screen.getByRole("progressbar", { name: "Run session checks" })).toHaveAttribute(
      "aria-valuenow",
      "100",
    )
  })
})
