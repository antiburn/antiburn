import { act, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { MainOverviewSession, type MainOverviewSnapshot } from "./MainOverviewSession"
import { OverviewView } from "./OverviewView"
import type { OverviewMetric } from "./overview/overviewViewPrefs"
import { readOverviewViewPrefs, writeOverviewViewPrefs } from "./overview/overviewViewPrefs"
import type {
  AllowanceUsageAccountPayload,
  LiveProviderUsagePayload,
} from "../../lib/providerUsageIpc"

vi.mock("./overview/OverviewUsage", () => ({
  OverviewUsage: ({
    metric,
    onMetricChange,
  }: {
    metric: OverviewMetric
    onMetricChange: (metric: OverviewMetric) => void
  }) => (
    <div>
      <output aria-label="Usage metric">{metric}</output>
      <button onClick={() => onMetricChange("cost")}>Cost</button>
      <button onClick={() => onMetricChange("allowance")}>Subscription</button>
    </div>
  ),
}))
vi.mock("./overview/OverviewRecentSessions", () => ({ OverviewRecentSessions: () => null }))
vi.mock("./overview/OverviewProviderLimits", () => ({ OverviewProviderLimits: () => null }))

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
  const props = { active: true, session, onOpenSessions: vi.fn(), onSelectSession: vi.fn() }
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

beforeEach(() => localStorage.clear())
afterEach(() => vi.restoreAllMocks())

function expectMetric(metric: OverviewMetric) {
  expect(screen.getByLabelText("Usage metric")).toHaveTextContent(metric)
}

describe("OverviewView metric preference", () => {
  it("defaults to cost without a known plan and does not save the default", () => {
    const view = setup()
    expectMetric("cost")
    view.update(allowance([]))
    expectMetric("cost")
    view.update(allowance([{ ...account, plan: null }]))
    expectMetric("cost")
    expect(readOverviewViewPrefs().metric).toBeUndefined()
  })

  it("defaults to subscription when a plan is already available", () => {
    setup(allowance([account]))
    expectMetric("allowance")
    expect(readOverviewViewPrefs().metric).toBeUndefined()
  })

  it.each(["history", "live"] as const)("uses a plan that arrives later from %s", (source) => {
    const view = setup()
    expectMetric("cost")
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
    const view = setup()
    expectMetric("cost")
    view.update(allowance([account]))
    expectMetric("allowance")
  })
})
