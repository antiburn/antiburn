import { cleanup, fireEvent, render, screen, within } from "@testing-library/react"
import { memo } from "react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { QuotaAccountPayload, QuotaUsagePayload } from "../../../lib/providerUsageIpc"
import type { SessionSubject } from "../../../lib/sessionSubject"
import type * as QuotaBurnupChartModule from "./QuotaBurnupChart"
import { QuotaSession, type QuotaAdapter } from "./QuotaSession"
import { QuotaView } from "./QuotaView"
import { quotaSessionKey } from "./quotaSeries"

// The real chart needs a measured layout that jsdom never supplies, and
// highlighting is now a CSS attribute on the wrapper div QuotaView renders
// around it, not a chart prop, so this stand-in only needs to exist, count
// its own renders, and capture the `onHighlight` callback so a test can call
// it the way a real chart hover would. Wrapped in `memo`, matching the real
// export, so a hover that leaves every one of its own props unchanged also
// leaves it unrendered here.
const chartRenders = { count: 0 }
const chart: {
  onHighlight: QuotaBurnupChartModule.QuotaBurnupChartProps["onHighlight"] | null
  props: QuotaBurnupChartModule.QuotaBurnupChartProps | null
} = { onHighlight: null, props: null }
vi.mock("./QuotaBurnupChart", async (importOriginal) => {
  const actual = await importOriginal<typeof QuotaBurnupChartModule>()
  return {
    ...actual,
    QuotaBurnupChart: memo((props: QuotaBurnupChartModule.QuotaBurnupChartProps) => {
      chartRenders.count += 1
      chart.onHighlight = props.onHighlight
      chart.props = props
      return <div data-testid="chart" />
    }),
  }
})

/** The highlight token QuotaView last set on the chart's wrapper div, or
 *  undefined when nothing is hovered. */
function wrapperHighlight(): string | null {
  return screen.getByTestId("chart").parentElement!.getAttribute("data-quota-highlight")
}

/** Opens the range dropdown: Radix's trigger listens for `pointerdown`, not
 *  `click`, to toggle open. */
function openRangeMenu(): void {
  const trigger = screen.getByRole("button", { name: "Range" })
  fireEvent.pointerDown(trigger, { button: 0, pointerId: 1 })
  fireEvent.click(trigger)
}

/** Opens the range dropdown and picks one item by its visible label. */
async function selectRangeOption(label: string): Promise<void> {
  openRangeMenu()
  const item = await screen.findByRole("menuitem", { name: label })
  fireEvent.click(item)
}

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
        unattributedBuckets: [{ bucketStartEpoch: NOW - WEEK, usd: 1, percent: 10 }],
        estimatedPercent: 30,
      },
    ],
    generatedAt: "g",
    ...over,
  }
}

/** Six sessions plus unattributed spend, so the top-sessions list holds a row
 *  outside the chart's own top five. */
function manySessionsUsage(): QuotaUsagePayload {
  const rowSessions = Array.from({ length: 6 }, (_, i) => ({
    agent: "claude",
    sessionId: `s${i + 1}`,
    wslDistro: null,
    title: `Session ${i + 1}`,
    usd: 6 - i,
    percent: 6 - i,
  }))
  return usage({
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
        sessions: rowSessions,
        unattributed: { usd: 1, percent: 10, sessionCount: 1 },
        unattributedBuckets: [{ bucketStartEpoch: NOW - WEEK, usd: 1, percent: 10 }],
        estimatedPercent: 30,
      },
    ],
  })
}

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => {
    resolve = done
  })
  return { promise, resolve }
}

function setup(overrides: Partial<QuotaAdapter> = {}) {
  const adapter: QuotaAdapter = {
    getAccounts: vi.fn().mockResolvedValue({ accounts: [account()], generatedAt: "g" }),
    getUsage: vi.fn().mockResolvedValue(usage()),
    getVisible: vi.fn().mockResolvedValue(true),
    onVisible: vi.fn(async () => () => undefined),
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
  chartRenders.count = 0
  chart.onHighlight = null
  cleanup()
})

// jsdom has no scrollIntoView. Stub it so a hover that should scroll a row
// leaves a call this suite can assert on, and restore the original (absent)
// implementation after so other suites see jsdom's real, missing method.
const originalScrollIntoView = Element.prototype.scrollIntoView
let scrollIntoView: ReturnType<typeof vi.fn<Element["scrollIntoView"]>>
beforeEach(() => {
  scrollIntoView = vi.fn()
  Element.prototype.scrollIntoView = scrollIntoView
})
afterEach(() => {
  Element.prototype.scrollIntoView = originalScrollIntoView
  vi.useRealTimers()
})

// The list scrolls only after the pointer rests on a band for this long.
const CHART_HOVER_SCROLL_DELAY_MS = 1200

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
    await selectRangeOption("30 days")
    await vi.waitFor(() => expect(session.getSnapshot().range).toBe("last30Days"))
    const lastCall = vi.mocked(adapter.getUsage).mock.calls.at(-1)![0]
    expect(lastCall.rangeStartEpoch).toBe(NOW - 30 * 24 * 60 * 60)
    expect(lastCall.rangeEndEpoch).toBe(NOW)
  })

  it("the range dropdown lists the window presets and the date presets in two groups", async () => {
    const { session } = setup()
    sessions.push(session)
    await screen.findByText("This week")
    openRangeMenu()
    const menu = within(await screen.findByRole("menu"))
    expect(menu.getByText("Windows")).toBeInTheDocument()
    expect(menu.getByText("Dates")).toBeInTheDocument()
    for (const label of [
      "This window",
      "Last window",
      "Last 3 windows",
      "Last 5 windows",
      "Last 10 windows",
      "This week",
      "Last week",
      "30 days",
    ]) {
      expect(menu.getByRole("menuitem", { name: label })).toBeInTheDocument()
    }
  })

  it("selects Last 3 windows and calls selectRange with the window preset", async () => {
    const { session, adapter } = setup()
    sessions.push(session)
    await screen.findByText("This week")
    vi.mocked(adapter.getUsage).mockResolvedValueOnce(usage({ generatedAt: "g-3w" }))
    await selectRangeOption("Last 3 windows")
    await vi.waitFor(() => expect(session.getSnapshot().range).toBe("last3Windows"))
    await screen.findByText("Last 3 windows")
  })

  it("shows Custom only while a custom range from open() is active, and drops it on a preset", async () => {
    const { session } = setup()
    sessions.push(session)
    await screen.findByText("This week")
    expect(screen.queryByText("Custom")).not.toBeInTheDocument()

    session.open(
      { provider: "anthropic", accountKey: "acct-1", lane: "weekly" },
      { startEpoch: NOW - WEEK, endEpoch: NOW },
    )
    await screen.findByText("Custom")

    await selectRangeOption("This week")
    await vi.waitFor(() => expect(session.getSnapshot().range).toBe("thisWeek"))
    expect(screen.queryByText("Custom")).not.toBeInTheDocument()
  })

  it("defaults to window axis mode with the pace line on, and the controls change what the chart receives", async () => {
    const { session } = setup()
    sessions.push(session)
    await screen.findByTestId("chart")
    expect(chart.props?.axisMode).toBe("window")
    expect(chart.props?.showPace).toBe(true)

    fireEvent.click(screen.getByRole("radio", { name: "Dates" }))
    expect(chart.props?.axisMode).toBe("date")

    fireEvent.click(screen.getByRole("switch", { name: "Pace line" }))
    expect(chart.props?.showPace).toBe(false)
  })

  it("passes the display periods to the chart", async () => {
    const { session } = setup()
    sessions.push(session)
    await screen.findByTestId("chart")
    expect(chart.props?.periods).toHaveLength(1)
    expect(chart.props?.periods[0]!.periodId).toBe(1)
  })

  it("clicking a top-sessions row calls onSelectSession with the session's subject", async () => {
    const { session, onSelectSession } = setup()
    sessions.push(session)
    const list = await screen.findByRole("region", { name: "Most prominent sessions" })
    const row = within(list).getByRole("button", { name: /Fix bug/ })
    fireEvent.click(row)
    expect(onSelectSession).toHaveBeenCalledWith({
      agent: "claude",
      sessionId: "s1",
      wslDistro: null,
      title: "Fix bug",
    })
  })

  it("hovering a top-five row sets its chart-index token on the chart's wrapper", async () => {
    const { session } = setup({ getUsage: vi.fn().mockResolvedValue(manySessionsUsage()) })
    sessions.push(session)
    const list = await screen.findByRole("region", { name: "Most prominent sessions" })
    const row = within(list).getByRole("button", { name: /Session 1/ })
    fireEvent.mouseEnter(row)
    // Session 1 carries the most dollars, so it is index 0 in the chart's
    // own top-session order.
    expect(wrapperHighlight()).toBe("s0")
    // A row hover only sets the highlight; it never scrolls the list, since
    // the row is already the thing the reader's pointer is on.
    expect(scrollIntoView).not.toHaveBeenCalled()
    fireEvent.mouseLeave(row)
    expect(wrapperHighlight()).toBeNull()
  })

  it("folds the sixth session into an 'other sessions' row that sets 'other' on the chart's wrapper", async () => {
    const { session } = setup({ getUsage: vi.fn().mockResolvedValue(manySessionsUsage()) })
    sessions.push(session)
    const list = await screen.findByRole("region", { name: "Most prominent sessions" })
    expect(within(list).queryByRole("button", { name: /Session 6/ })).not.toBeInTheDocument()
    const row = within(list).getByText(/other session/).parentElement!
    expect(row).toHaveTextContent("1 other session")
    fireEvent.mouseEnter(row)
    expect(wrapperHighlight()).toBe("other")
    fireEvent.mouseLeave(row)
    expect(wrapperHighlight()).toBeNull()
  })

  it("hovering the Unattributed row sets 'unattributed' on the chart's wrapper", async () => {
    const { session } = setup({ getUsage: vi.fn().mockResolvedValue(manySessionsUsage()) })
    sessions.push(session)
    const list = await screen.findByRole("region", { name: "Most prominent sessions" })
    const row = within(list).getByText("Unattributed").parentElement!
    fireEvent.mouseEnter(row)
    expect(wrapperHighlight()).toBe("unattributed")
    fireEvent.mouseLeave(row)
    expect(wrapperHighlight()).toBeNull()
  })

  it("does not re-render the memoized chart when hover only changes the wrapper attribute", async () => {
    const { session } = setup({ getUsage: vi.fn().mockResolvedValue(manySessionsUsage()) })
    sessions.push(session)
    const list = await screen.findByRole("region", { name: "Most prominent sessions" })
    const row = within(list).getByRole("button", { name: /Session 1/ })
    await screen.findByTestId("chart")
    const rendersBeforeHover = chartRenders.count

    fireEvent.mouseEnter(row)
    expect(wrapperHighlight()).toBe("s0")
    expect(chartRenders.count).toBe(rendersBeforeHover)

    fireEvent.mouseLeave(row)
    expect(wrapperHighlight()).toBeNull()
    expect(chartRenders.count).toBe(rendersBeforeHover)
  })

  it("scrolls a top-session's row into view when the chart highlights it", async () => {
    const { session } = setup({ getUsage: vi.fn().mockResolvedValue(manySessionsUsage()) })
    sessions.push(session)
    const list = await screen.findByRole("region", { name: "Most prominent sessions" })
    const row = within(list).getByRole("button", { name: /Session 1/ })

    // The chart's own bands key a top session the same way the top-sessions
    // list does: by agent, session id and WSL distro, not by the CSS index
    // token the wrapper attribute uses.
    vi.useFakeTimers()
    chart.onHighlight!(quotaSessionKey("claude", "s1", null))

    // A pass over the band must not move the list: nothing scrolls until
    // the whole delay has elapsed.
    vi.advanceTimersByTime(CHART_HOVER_SCROLL_DELAY_MS - 1)
    expect(scrollIntoView).not.toHaveBeenCalled()
    vi.advanceTimersByTime(1)
    expect(scrollIntoView).toHaveBeenCalledOnce()
    expect(scrollIntoView.mock.contexts[0]).toBe(row)
    expect(scrollIntoView).toHaveBeenCalledWith(expect.objectContaining({ block: "nearest" }))
  })

  it("scrolls the other-sessions row into view when the chart highlights 'other'", async () => {
    const { session } = setup({ getUsage: vi.fn().mockResolvedValue(manySessionsUsage()) })
    sessions.push(session)
    const list = await screen.findByRole("region", { name: "Most prominent sessions" })
    const row = within(list).getByText(/other session/).parentElement!

    vi.useFakeTimers()
    chart.onHighlight!("other")
    vi.advanceTimersByTime(CHART_HOVER_SCROLL_DELAY_MS)

    expect(scrollIntoView).toHaveBeenCalledOnce()
    expect(scrollIntoView.mock.contexts[0]).toBe(row)
    expect(scrollIntoView).toHaveBeenCalledWith(expect.objectContaining({ block: "nearest" }))
  })

  it("does not scroll when the pointer leaves the band before the delay", async () => {
    const { session } = setup({ getUsage: vi.fn().mockResolvedValue(manySessionsUsage()) })
    sessions.push(session)
    await screen.findByRole("region", { name: "Most prominent sessions" })

    vi.useFakeTimers()
    chart.onHighlight!(quotaSessionKey("claude", "s1", null))
    vi.advanceTimersByTime(CHART_HOVER_SCROLL_DELAY_MS / 2)
    chart.onHighlight!(null)
    vi.advanceTimersByTime(CHART_HOVER_SCROLL_DELAY_MS)

    expect(scrollIntoView).not.toHaveBeenCalled()
  })

  it("does not scroll when the chart clears its highlight", async () => {
    const { session } = setup({ getUsage: vi.fn().mockResolvedValue(manySessionsUsage()) })
    sessions.push(session)
    await screen.findByRole("region", { name: "Most prominent sessions" })

    chart.onHighlight!(null)

    expect(scrollIntoView).not.toHaveBeenCalled()
  })

  it("shows the unattributed row when its dollars are non-zero", async () => {
    const { session } = setup()
    sessions.push(session)
    const list = await screen.findByRole("region", { name: "Most prominent sessions" })
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
    const list = await screen.findByRole("region", { name: "Most prominent sessions" })
    within(list).getByText(/Fix bug/)
    expect(within(list).queryByText("Unattributed")).not.toBeInTheDocument()
  })

  it("marks the chart region busy while a reload runs beside existing usage", async () => {
    const { session, adapter } = setup()
    sessions.push(session)
    const list = await screen.findByRole("region", { name: "Most prominent sessions" })
    expect(list.closest('[aria-busy="true"]')).toBeNull()

    const pending = deferred<QuotaUsagePayload>()
    vi.mocked(adapter.getUsage).mockReturnValueOnce(pending.promise)
    await selectRangeOption("30 days")
    expect(list.closest('[aria-busy="true"]')).not.toBeNull()

    pending.resolve(usage({ generatedAt: "g-30d" }))
    await vi.waitFor(() => expect(session.getSnapshot().loading).toBe(false))
    expect(list.closest('[aria-busy="true"]')).toBeNull()
  })
})
