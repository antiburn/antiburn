import { cleanup, fireEvent, render, screen, within } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import type { QuotaAccountPayload, QuotaUsagePayload } from "../../../lib/providerUsageIpc"
import type { SessionSubject } from "../../../lib/sessionSubject"
import { QuotaSession, type QuotaAdapter } from "./QuotaSession"
import { QuotaView } from "./QuotaView"

const NOW = 1_000_000
const WEEK = 604800

function account(over: Partial<QuotaAccountPayload> = {}): QuotaAccountPayload {
  return {
    provider: "anthropic",
    displayName: "Claude",
    accountKey: "acct-1",
    lanes: [{ lane: "weekly", label: "Weekly", hasFactor: true, currentPeriod: null }],
    ...over,
  }
}

function usage(over: Partial<QuotaUsagePayload> = {}): QuotaUsagePayload {
  return {
    provider: "anthropic",
    accountKey: "acct-1",
    lane: "weekly",
    laneLabel: "Weekly",
    rangeStartEpoch: NOW - WEEK,
    rangeEndEpoch: NOW,
    factor: { usdPerPercent: 1, confidence: "learned" },
    periods: [
      {
        periodId: 1,
        startsAtEpoch: NOW - WEEK,
        resetsAtEpoch: NOW,
        startSource: "reported",
        resetSource: "reported",
        samples: [
          { observedAtEpoch: NOW - 100, usedPercent: 40, fresh: true, authoritative: true },
        ],
        peakPercent: 40,
        contributions: [],
        sessions: [
          {
            agent: "claude",
            sessionId: "s1",
            wslDistro: null,
            title: "Fix bug",
            usd: 3,
            percent: 30,
          },
        ],
        unattributed: { usd: 1, percent: 10, sessionCount: 1 },
        estimatedPercent: 30,
      },
    ],
    generatedAt: "g",
    ...over,
  }
}

function setup(overrides: Partial<QuotaAdapter> = {}) {
  const adapter: QuotaAdapter = {
    getAccounts: vi.fn().mockResolvedValue({ accounts: [account()], generatedAt: "g" }),
    getUsage: vi.fn().mockResolvedValue(usage()),
    getVisible: vi.fn().mockResolvedValue(true),
    onVisible: vi.fn(async () => () => undefined),
    onLiveUsageChanged: vi.fn(async () => () => undefined),
    onScanFinished: vi.fn(async () => () => undefined),
    now: vi.fn(() => NOW),
    ...overrides,
  }
  const session = new QuotaSession(adapter)
  const onSelectSession = vi.fn<(subject: SessionSubject) => void>()
  const view = render(<QuotaView active session={session} onSelectSession={onSelectSession} />)
  return { adapter, session, view, onSelectSession }
}

const sessions: QuotaSession[] = []
afterEach(() => {
  sessions.splice(0).forEach((s) => s.dispose())
  cleanup()
})

describe("QuotaView", () => {
  it("shows a loading state before the first accounts load resolves", () => {
    const { session } = setup()
    sessions.push(session)
    expect(screen.getByRole("status")).toHaveTextContent("Loading Quota.")
  })

  it("shows the empty copy when there are no accounts", async () => {
    const { session } = setup({
      getAccounts: vi.fn().mockResolvedValue({ accounts: [], generatedAt: "g" }),
    })
    sessions.push(session)
    await screen.findByText(/No quota readings yet/)
  })

  it("shows an error with a retry button when accounts fail to load", async () => {
    const { session, adapter } = setup({
      getAccounts: vi.fn().mockRejectedValue(new Error("no")),
    })
    sessions.push(session)
    await screen.findByRole("alert")
    expect(screen.getByRole("alert")).toHaveTextContent("Quota accounts are unavailable.")
    vi.mocked(adapter.getAccounts).mockResolvedValueOnce({
      accounts: [account()],
      generatedAt: "g2",
    })
    fireEvent.click(screen.getByRole("button", { name: "Retry" }))
    await screen.findByText("This week")
  })

  it("shows 'No windows in this range' when usage has no periods", async () => {
    const { session } = setup({ getUsage: vi.fn().mockResolvedValue(usage({ periods: [] })) })
    sessions.push(session)
    await screen.findByText("No windows in this range.")
  })

  it("changing the range control reloads usage for the new range", async () => {
    const { session, adapter } = setup()
    sessions.push(session)
    await screen.findByText("This week")
    vi.mocked(adapter.getUsage).mockResolvedValueOnce(usage({ generatedAt: "g-30d" }))
    fireEvent.click(screen.getByRole("radio", { name: "30 days" }))
    await vi.waitFor(() => expect(session.getSnapshot().range).toBe("last30Days"))
    const lastCall = vi.mocked(adapter.getUsage).mock.calls.at(-1)![0]
    expect(lastCall.rangeStartEpoch).toBe(NOW - 30 * 24 * 60 * 60)
    expect(lastCall.rangeEndEpoch).toBe(NOW)
  })

  it("clicking a top-sessions row calls onSelectSession with the session's subject", async () => {
    const { session, onSelectSession } = setup()
    sessions.push(session)
    const list = await screen.findByRole("region", { name: "Top sessions" })
    const row = within(list).getByRole("button", { name: /Fix bug/ })
    fireEvent.click(row)
    expect(onSelectSession).toHaveBeenCalledWith({
      agent: "claude",
      sessionId: "s1",
      wslDistro: null,
      title: "Fix bug",
    })
  })

  it("shows the unattributed row when its dollars are non-zero", async () => {
    const { session } = setup()
    sessions.push(session)
    const list = await screen.findByRole("region", { name: "Top sessions" })
    within(list).getByText("Unattributed")
  })

  it("omits the unattributed row when its dollars are zero", async () => {
    const zeroed = usage()
    const { session } = setup({
      getUsage: vi.fn().mockResolvedValue({
        ...zeroed,
        periods: [
          { ...zeroed.periods[0]!, unattributed: { usd: 0, percent: 0, sessionCount: 0 } },
        ],
      }),
    })
    sessions.push(session)
    const list = await screen.findByRole("region", { name: "Top sessions" })
    within(list).getByText(/Fix bug/)
    expect(within(list).queryByText("Unattributed")).not.toBeInTheDocument()
  })
})
