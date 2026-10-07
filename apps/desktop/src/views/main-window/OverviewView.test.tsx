import { act, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { MainOverviewSession, type MainOverviewSnapshot } from "./MainOverviewSession"
import { OverviewView } from "./OverviewView"
import type { OverviewMetric } from "./overview/overviewViewPrefs"
import { readOverviewViewPrefs, writeOverviewViewPrefs } from "./overview/overviewViewPrefs"
import type { AppSettings } from "../../lib/ipc"
import type { OverviewProgress } from "./overview/overviewProgressStore"
import type * as OverviewProgressStore from "./overview/overviewProgressStore"
import type {
  AllowanceUsageAccountPayload,
  LiveProviderUsagePayload,
} from "../../lib/providerUsageIpc"

const appSettings = vi.hoisted(() => ({
  current: { liveUsageEnabled: true, liveUsageStarted: false } as AppSettings,
}))
vi.mock("../settings/useAppSettings", () => ({
  useAppSettings: () => ({ settings: appSettings.current, loaded: true, update: vi.fn() }),
}))

// The real store's first settings read resolves to `onboardingCompleted:
// false` without a shell, which would otherwise flip this suite into the
// first-run takeover — hiding the usage card this file's tests click
// through — a tick after mount. Pinning the mode to "steady" keeps this
// suite about the metric preference; the takeover itself is covered in
// `FirstRunTakeover.test.tsx`.
const overviewProgressMock = vi.hoisted(() => ({
  current: {
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
  } as OverviewProgress,
}))
vi.mock("./overview/overviewProgressStore", async (importOriginal) => ({
  ...(await importOriginal<typeof OverviewProgressStore>()),
  subscribeOverviewProgress: () => () => undefined,
  overviewProgress: () => overviewProgressMock.current,
}))
vi.mock("./overview/FirstRunTakeover", () => ({
  FirstRunTakeover: () => <output aria-label="First-run takeover" />,
}))

vi.mock("./overview/OverviewUsage", () => ({
  OverviewUsage: ({
    metric,
    onMetricChange,
    loading,
    allowanceCollecting,
  }: {
    metric: OverviewMetric
    onMetricChange: (metric: OverviewMetric) => void
    loading?: boolean
    allowanceCollecting?: boolean
  }) => (
    <div>
      <output aria-label="Usage metric">{metric}</output>
      <output aria-label="Usage state">{loading ? "held" : "shown"}</output>
      <output aria-label="Allowance collection">
        {allowanceCollecting ? "collecting" : "idle"}
      </output>
      <button onClick={() => onMetricChange("cost")}>Cost</button>
      <button onClick={() => onMetricChange("allowance")}>Subscription</button>
    </div>
  ),
}))
vi.mock("./overview/OverviewRecentSessions", () => ({
  OverviewRecentSessions: ({ showChecks }: { showChecks?: boolean }) => (
    <output aria-label="Recent sessions">
      {showChecks === false ? "no checks" : "checks"}
    </output>
  ),
}))
vi.mock("./overview/OverviewProviderLimits", () => ({
  OverviewProviderLimits: () => <output aria-label="Provider limits pane" />,
}))

const account: AllowanceUsageAccountPayload = {
  provider: "anthropic",
  displayName: "Claude",
  accountKey: "account",
  plan: { name: "max", tier: null },
  utilization: null,
  chart: { shortWindows: [], weeklyWindows: [], rolling: [] },
}
const liveProvider: LiveProviderUsagePayload = {
  provider: "anthropic",
  accountKey: "account",
  displayName: "Claude",
  support: "live",
  freshness: "fresh",
  sourceLabel: "Test",
  observedAt: "2026-09-22T00:00:00Z",
  windows: [],
  extraUsage: null,
  resetCredits: null,
  plan: account.plan,
  accountUuid: null,
  accountEmail: null,
}

function allowance(accounts: AllowanceUsageAccountPayload[]): Partial<MainOverviewSnapshot> {
  return {
    allowance: {
      accounts,
      utilizationSpanDays: 28,
      rangeStartEpoch: 0,
      rangeEndEpoch: 0,
      generatedAt: "",
    },
  }
}

/** A local usage summary, so `loading` turns on the unit being unresolved
 *  rather than on the spend figures being unread. */
const usage: Partial<MainOverviewSnapshot> = {
  usage: {
    totals: { today: [], week: [], month: [] },
    days: [],
    generatedAt: "",
  } as unknown as MainOverviewSnapshot["usage"],
}

function setup(initial: Partial<MainOverviewSnapshot> = {}) {
  const session = new MainOverviewSession({
    getSnapshot: () => ({ entries: [] }),
    subscribeList: () => () => undefined,
  })
  let snapshot = { ...session.getSnapshot(), ...initial }
  const listeners = new Set<() => void>()
  vi.spyOn(session, "getSnapshot").mockImplementation(() => snapshot)
  vi.spyOn(session, "subscribe").mockImplementation((listener) => {
    listeners.add(listener)
    return () => {
      listeners.delete(listener)
    }
  })
  const props = {
    active: true,
    session,
    onOpenSessions: vi.fn(),
    onSelectSession: vi.fn(),
    onOpenChecks: vi.fn(),
  }
  const result = render(<OverviewView {...props} />)
  return {
    ...result,
    props,
    update(patch: Partial<MainOverviewSnapshot>) {
      act(() => {
        snapshot = { ...snapshot, ...patch }
        for (const listener of listeners) listener()
      })
    },
  }
}

beforeEach(() => {
  localStorage.clear()
  appSettings.current = { liveUsageEnabled: true, liveUsageStarted: false } as AppSettings
  overviewProgressMock.current = {
    ...overviewProgressMock.current,
    mode: "steady",
    flow: "done",
  }
})
afterEach(() => vi.restoreAllMocks())

function expectMetric(metric: OverviewMetric) {
  expect(screen.getByLabelText("Usage metric")).toHaveTextContent(metric)
}

function expectUsageState(state: "held" | "shown") {
  expect(screen.getByLabelText("Usage state")).toHaveTextContent(state)
}

describe("OverviewView metric preference", () => {
  it("shows allowance collection only during active first-run live usage", () => {
    overviewProgressMock.current = {
      ...overviewProgressMock.current,
      mode: "firstRun",
      flow: "checks",
    }
    appSettings.current = { liveUsageEnabled: true, liveUsageStarted: false } as AppSettings
    const view = setup(usage)
    expect(screen.getByLabelText("Allowance collection")).toHaveTextContent("idle")

    appSettings.current = { liveUsageEnabled: true, liveUsageStarted: true } as AppSettings
    view.rerender(<OverviewView {...view.props} />)
    expect(screen.getByLabelText("Allowance collection")).toHaveTextContent("collecting")

    overviewProgressMock.current = { ...overviewProgressMock.current, flow: "done" }
    view.rerender(<OverviewView {...view.props} />)
    expect(screen.getByLabelText("Allowance collection")).toHaveTextContent("idle")
  })

  it("settles on cost only once both the allowance and live-usage reads land without a plan", () => {
    const view = setup(usage)
    view.update(allowance([]))
    // The allowance read landed with no plan, but live usage has not
    // answered yet, so the unit stays undecided rather than picking cost.
    expectUsageState("held")
    view.update({
      liveUsage: { providers: [], errors: [], meters: [], generatedAt: "" },
      liveUsageSettled: true,
    })
    expectMetric("cost")
    view.update(allowance([{ ...account, plan: null }]))
    expectMetric("cost")
    expect(readOverviewViewPrefs().metric).toBeUndefined()
  })

  it("never shows cost when a plan arrives from a still-pending live-usage read", () => {
    const view = setup(usage)
    view.update(allowance([]))
    expectUsageState("held")
    view.update({
      liveUsage: { providers: [liveProvider], errors: [], meters: [], generatedAt: "" },
      liveUsageSettled: true,
    })
    expectMetric("allowance")
    expectUsageState("shown")
  })

  it("holds the figures until it knows which unit to show them in", () => {
    const view = setup(usage)
    // Nothing chosen, nothing remembered and nothing read: the unit on screen
    // is a guess, so the figures wait rather than land under the wrong tab.
    expectUsageState("held")
    view.update(allowance([account]))
    expectMetric("allowance")
    expectUsageState("shown")
  })

  it.each([
    ["a plan", true, "allowance"],
    ["no plan", false, "cost"],
  ] as const)("opens on the unit %s left behind last run", (_label, hadPlan, expected) => {
    // The session remembers the answer from a run's settled reads; this
    // covers the view reading that memory back before any read of its own
    // has answered.
    writeOverviewViewPrefs({ hadSubscriptionPlan: hadPlan })
    setup(usage)
    expectMetric(expected)
    expectUsageState("shown")
  })

  it("corrects a remembered answer that no longer holds", () => {
    writeOverviewViewPrefs({ hadSubscriptionPlan: true })
    const view = setup(usage)
    expectMetric("allowance")
    view.update(allowance([]))
    // The allowance read alone disagrees, but live usage has not answered
    // yet, so the remembered answer still stands.
    expectMetric("allowance")
    view.update({
      liveUsage: { providers: [], errors: [], meters: [], generatedAt: "" },
      liveUsageSettled: true,
    })
    expectMetric("cost")
  })

  it("defaults to subscription when a plan is already available", () => {
    setup(allowance([account]))
    expectMetric("allowance")
    expect(readOverviewViewPrefs().metric).toBeUndefined()
  })

  it.each(["history", "live"] as const)("uses a plan that arrives later from %s", (source) => {
    const view = setup(usage)
    expectUsageState("held")
    view.update(
      source === "history"
        ? allowance([account])
        : {
            liveUsage: { providers: [liveProvider], errors: [], meters: [], generatedAt: "" },
            liveUsageSettled: true,
          },
    )
    expectMetric("allowance")
  })

  it.each(["cost", "allowance"] as const)(
    "preserves a saved %s preference regardless of plans",
    (metric) => {
      writeOverviewViewPrefs({ metric })
      const view = setup()
      expectMetric(metric)
      view.update(allowance([account]))
      expectMetric(metric)
      view.update(allowance([]))
      expectMetric(metric)
    },
  )

  it("preserves a choice made before plans load and restores it after remount", () => {
    writeOverviewViewPrefs({ accountTabKey: "anthropic:account" })
    const view = setup()
    fireEvent.click(screen.getByRole("button", { name: "Cost" }))
    view.update(allowance([account]))
    expectMetric("cost")
    expect(readOverviewViewPrefs()).toEqual({
      metric: "cost",
      accountTabKey: "anthropic:account",
    })
    view.unmount()
    render(<OverviewView {...view.props} />)
    expectMetric("cost")
  })

  it("allows an explicit subscription choice even without a known plan", () => {
    const view = setup()
    fireEvent.click(screen.getByRole("button", { name: "Subscription" }))
    view.update(allowance([]))
    expectMetric("allowance")
    expect(readOverviewViewPrefs().metric).toBe("allowance")
  })

  it("treats an invalid saved metric as no preference", () => {
    localStorage.setItem("antiburn.overview.view.v1", JSON.stringify({ metric: "invalid" }))
    const view = setup(usage)
    expectUsageState("held")
    view.update(allowance([account]))
    expectMetric("allowance")
  })
})

describe("OverviewView's provider limits card", () => {
  it("hides the pane while live usage has not started, even though it defaults to enabled", () => {
    appSettings.current = { liveUsageEnabled: true, liveUsageStarted: false } as AppSettings
    setup()
    expect(screen.queryByLabelText("Provider limits pane")).toBeNull()
  })

  it("hides the pane when a reader turns limits off in Settings, even once started", () => {
    appSettings.current = { liveUsageEnabled: false, liveUsageStarted: true } as AppSettings
    setup()
    expect(screen.queryByLabelText("Provider limits pane")).toBeNull()
  })

  it("shows the pane once live usage is both enabled and started", () => {
    appSettings.current = { liveUsageEnabled: true, liveUsageStarted: true } as AppSettings
    setup()
    expect(screen.getByLabelText("Provider limits pane")).toBeInTheDocument()
  })
})

describe("OverviewView's first-run takeover", () => {
  function firstRunAt(flow: OverviewProgress["flow"]) {
    overviewProgressMock.current = { ...overviewProgressMock.current, mode: "firstRun", flow }
  }

  it("shows no usage card or Recent sessions until the Sessions step is done", () => {
    firstRunAt("sessions")
    setup()
    expect(screen.getByLabelText("First-run takeover")).toBeInTheDocument()
    expect(screen.queryByLabelText("Usage metric")).toBeNull()
    expect(screen.queryByLabelText("Recent sessions")).toBeNull()
  })

  it("shows Recent sessions without checks under the Checks step", () => {
    firstRunAt("checks")
    setup()
    expect(screen.getByLabelText("First-run takeover")).toBeInTheDocument()
    expect(screen.getByLabelText("Usage metric")).toBeInTheDocument()
    expect(screen.getByLabelText("Recent sessions")).toHaveTextContent("no checks")
  })

  it("adds the checks once the Checks step is done", () => {
    firstRunAt("fixes")
    setup()
    expect(screen.getByLabelText("Recent sessions")).toHaveTextContent(/^checks$/)
  })
})
