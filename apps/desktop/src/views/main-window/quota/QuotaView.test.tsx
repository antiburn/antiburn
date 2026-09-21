import { cleanup, fireEvent, render, screen, within } from "@testing-library/react"
import { memo } from "react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { QuotaAccountPayload, QuotaUsagePayload } from "../../../lib/providerUsageIpc"
import type { SessionSubject } from "../../../lib/sessionSubject"
import type * as QuotaBurnupChartModule from "./QuotaBurnupChart"
import { QuotaSession, type QuotaAdapter } from "./QuotaSession"
import { QuotaView } from "./QuotaView"
import { quotaSessionKey } from "./quotaSeries"
import { writeQuotaViewPrefs } from "./quotaViewPrefs"

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

/** Opens one jump bar menu: Radix's trigger listens for `pointerdown`, not
 *  `click`, to toggle open. The trigger's name is "<level>: <value>". */
function openJumpMenu(level: "Account" | "Lane" | "Range"): void {
  const trigger = screen.getByRole("button", { name: new RegExp(`^${level}: `) })
  fireEvent.pointerDown(trigger, { button: 0, pointerId: 1 })
  fireEvent.click(trigger)
}

/** Opens the range menu and picks one item by its visible label. */
async function selectRangeOption(label: string): Promise<void> {
  openJumpMenu("Range")
  const item = await screen.findByRole("menuitemradio", { name: label })
  fireEvent.click(item)
}

/** Resolves once the loaded screen shows: the range level is always a menu. */
async function loaded(): Promise<HTMLElement> {
  return screen.findByRole("button", { name: /^Range: / })
}

/** The jump bar's range trigger, by its accessible name. */
function rangeTrigger(): HTMLElement {
  return screen.getByRole("button", { name: /^Range: / })
}

const NOW = 1_000_000
const WEEK = 604800

function account(over: Partial<QuotaAccountPayload> = {}): QuotaAccountPayload {
  return {
    provider: "anthropic",
    displayName: "Claude",
    accountKey: "acct-1",
    lanes: [{ lane: "weekly", label: "Weekly", currentPeriod: null }],
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
        unexplainedBuckets: [],
        unexplainedPercent: null,
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
        contributions: [],
        sessions: rowSessions,
        unattributed: { usd: 1, percent: 10, sessionCount: 1 },
        unattributedBuckets: [{ bucketStartEpoch: NOW - WEEK, usd: 1, percent: 10 }],
        estimatedPercent: 30,
        unexplainedBuckets: [],
        unexplainedPercent: null,
      },
    ],
  })
}

/** Two closed windows and one open window, with the same session in each,
 *  so a row can say how many windows it spans. */
function multiWindowUsage(): QuotaUsagePayload {
  const base = usage().periods[0]!
  return usage({
    rangeStartEpoch: NOW - 2 * WEEK,
    rangeEndEpoch: NOW + WEEK,
    periods: [
      {
        ...base,
        periodId: 1,
        startsAtEpoch: NOW - 2 * WEEK,
        resetsAtEpoch: NOW - WEEK,
        estimatedPercent: 20,
      },
      {
        ...base,
        periodId: 2,
        startsAtEpoch: NOW - WEEK,
        resetsAtEpoch: NOW,
        estimatedPercent: 40,
      },
      {
        ...base,
        periodId: 3,
        startsAtEpoch: NOW,
        resetsAtEpoch: NOW + WEEK,
        estimatedPercent: 15,
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
  localStorage.clear()
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
    expect(screen.getByRole("status")).toHaveTextContent("Loading Limits.")
  })

  it("shows the empty copy when there are no accounts", async () => {
    const { session } = setup({
      getAccounts: vi.fn().mockResolvedValue({ accounts: [], generatedAt: "g" }),
    })
    sessions.push(session)
    await screen.findByText(/No limit readings yet/)
  })

  it("shows an error with a retry button when accounts fail to load", async () => {
    const { session, adapter } = setup({
      getAccounts: vi.fn().mockRejectedValue(new Error("no")),
    })
    sessions.push(session)
    await screen.findByRole("alert")
    expect(screen.getByRole("alert")).toHaveTextContent("Limit accounts are unavailable.")
    vi.mocked(adapter.getAccounts).mockResolvedValueOnce({
      accounts: [account()],
      generatedAt: "g2",
    })
    fireEvent.click(screen.getByRole("button", { name: "Retry" }))
    await loaded()
  })

  it("shows 'No windows in this range' when usage has no periods", async () => {
    const { session } = setup({ getUsage: vi.fn().mockResolvedValue(usage({ periods: [] })) })
    sessions.push(session)
    await screen.findByText("No windows in this range.")
  })

  it("shows This week with its reset, then Highest, P90 and Median over the windows in range", async () => {
    const { session } = setup({ getUsage: vi.fn().mockResolvedValue(multiWindowUsage()) })
    sessions.push(session)
    await loaded()
    await selectRangeOption("Last 3 weeks")
    const figures = await screen.findByRole("region", { name: "Limit usage" })
    // All three windows count: 20%, 40% and the open window at 15% now.
    // Sorted they are 15, 20, 40: Highest is 40%, the median is 20%, and
    // the interpolated P90 sits at rank 1.8, so 20 + 0.8 * 20 = 36%. The
    // third window is still open (its reset is after `now`), so This week
    // reads its own value, 15%, over its reset countdown. The figures round
    // to whole percents.
    const cell = (label: string) => within(figures).getByText(label).closest("div")!
    expect(cell("This week")).toHaveTextContent("15%")
    expect(cell("This week")).toHaveTextContent("resets in 7d")
    expect(cell("Highest week")).toHaveTextContent("40%")
    expect(cell("Highest week")).toHaveTextContent("of 3 weeks")
    expect(cell("P90 week")).toHaveTextContent("36%")
    expect(cell("Median week")).toHaveTextContent("20%")
    expect(figures.textContent).not.toContain("40.0%")
    expect(screen.getByText(/Last reading/)).toBeInTheDocument()
  })

  it("shows only This week when the open window is the only one in range", async () => {
    const base = usage().periods[0]!
    const onlyOpen = usage({
      rangeEndEpoch: NOW + WEEK,
      periods: [
        {
          ...base,
          periodId: 1,
          startsAtEpoch: NOW,
          resetsAtEpoch: NOW + WEEK,
          estimatedPercent: 25,
        },
      ],
    })
    const { session } = setup({ getUsage: vi.fn().mockResolvedValue(onlyOpen) })
    sessions.push(session)
    await loaded()
    const figures = screen.getByRole("region", { name: "Limit usage" })
    // One window compares with nothing, so the three comparison cells stay
    // out and the open window's own cell is the whole row.
    expect(within(figures).getByText("This week").closest("div")).toHaveTextContent("25%")
    expect(within(figures).queryByText("Highest week")).not.toBeInTheDocument()
    expect(within(figures).queryByText("P90 week")).not.toBeInTheDocument()
    expect(within(figures).queryByText("Median week")).not.toBeInTheDocument()
    expect(figures.textContent).not.toContain("—")
  })

  it("names a lone closed window by its lane word with its reset time", async () => {
    const base = usage().periods[0]!
    const closed = usage({
      periods: [{ ...base, startsAtEpoch: NOW - 2 * WEEK, resetsAtEpoch: NOW - WEEK }],
    })
    const { session } = setup({ getUsage: vi.fn().mockResolvedValue(closed) })
    sessions.push(session)
    await loaded()
    const figures = screen.getByRole("region", { name: "Limit usage" })
    const cell = within(figures).getByText("Week").closest("div")!
    expect(cell).toHaveTextContent("30%")
    expect(cell).toHaveTextContent(/reset .* ago/)
    expect(within(figures).queryByText("This week")).not.toBeInTheDocument()
  })

  it("clamps a window's value at 100 percent, since the provider's own meter can never pass it", async () => {
    const base = usage().periods[0]!
    const overshootWindows = usage({
      rangeStartEpoch: NOW - 2 * WEEK,
      rangeEndEpoch: NOW + WEEK,
      periods: [
        {
          ...base,
          periodId: 1,
          startsAtEpoch: NOW - 2 * WEEK,
          resetsAtEpoch: NOW - WEEK,
          estimatedPercent: 130,
        },
        {
          ...base,
          periodId: 2,
          startsAtEpoch: NOW - WEEK,
          resetsAtEpoch: NOW,
          estimatedPercent: 50,
        },
        {
          ...base,
          periodId: 3,
          startsAtEpoch: NOW,
          resetsAtEpoch: NOW + WEEK,
          estimatedPercent: 120,
        },
      ],
    })
    const { session } = setup({ getUsage: vi.fn().mockResolvedValue(overshootWindows) })
    sessions.push(session)
    await loaded()
    await selectRangeOption("Last 3 weeks")
    const figures = await screen.findByRole("region", { name: "Limit usage" })
    // An estimated tail with no meter readings can price a window past 100,
    // but the provider's own meter never passes it: 130 and 120 both clamp
    // to 100 before the figures read them. Sorted the windows are 50, 100,
    // 100, so This week, Highest, P90 and Median all read 100% (four times,
    // total) and nothing reads past it.
    expect((figures.textContent!.match(/100%/g) ?? []).length).toBe(4)
    expect(figures.textContent).not.toContain("130%")
    expect(figures.textContent).not.toContain("120%")
  })

  it("shows the Unexplained row in the sessions list once the latest period carries unexplained spend", async () => {
    const basePeriod = usage().periods[0]!
    const withUnexplained = usage({
      periods: [
        {
          ...basePeriod,
          estimatedPercent: 33.1,
          unexplainedPercent: 8.9,
          unexplainedBuckets: [{ bucketStartEpoch: NOW - WEEK, usd: 0, percent: 8.9 }],
        },
      ],
    })
    const { session } = setup({ getUsage: vi.fn().mockResolvedValue(withUnexplained) })
    sessions.push(session)
    await loaded()
    expect(screen.getByText("Unexplained")).toBeInTheDocument()
  })

  it("changing the range reloads usage for the new preset", async () => {
    const { session, adapter } = setup()
    sessions.push(session)
    await loaded()
    const callsBefore = vi.mocked(adapter.getUsage).mock.calls.length
    vi.mocked(adapter.getUsage).mockResolvedValueOnce(usage({ generatedAt: "g-5w" }))
    await selectRangeOption("Last 5 weeks")
    await vi.waitFor(() => expect(session.getSnapshot().range).toBe("last5Windows"))
    expect(vi.mocked(adapter.getUsage).mock.calls.length).toBeGreaterThan(callsBefore)
  })

  it("the range menu lists the five window presets in the lane's own words, with the current one checked", async () => {
    const { session } = setup()
    sessions.push(session)
    await loaded()
    openJumpMenu("Range")
    const menu = within(await screen.findByRole("menu"))
    expect(menu.queryByText("Windows")).not.toBeInTheDocument()
    expect(menu.queryByText("Dates")).not.toBeInTheDocument()
    expect(menu.getAllByRole("menuitemradio").map((item) => item.textContent)).toEqual([
      "This week",
      "Last week",
      "Last 3 weeks",
      "Last 5 weeks",
      "Last 10 weeks",
    ])
    expect(menu.getByRole("menuitemradio", { name: "This week" })).toHaveAttribute(
      "aria-checked",
      "true",
    )
    expect(menu.getByRole("menuitemradio", { name: "Last week" })).toHaveAttribute(
      "aria-checked",
      "false",
    )
  })

  it("selects Last 3 weeks and calls selectRange with the window preset", async () => {
    const { session, adapter } = setup()
    sessions.push(session)
    await loaded()
    vi.mocked(adapter.getUsage).mockResolvedValueOnce(usage({ generatedAt: "g-3w" }))
    await selectRangeOption("Last 3 weeks")
    await vi.waitFor(() => expect(session.getSnapshot().range).toBe("last3Windows"))
    expect(rangeTrigger()).toHaveTextContent("Last 3 weeks")
  })

  it("words the range and the figures by window on a 5-hour lane", async () => {
    const twoLanes = account({
      lanes: [
        { lane: "weekly", label: "Weekly", currentPeriod: null },
        { lane: "fiveHour", label: "5-hour", currentPeriod: null },
      ],
    })
    // The window is still open, so the figures lead with "This window".
    const base = usage().periods[0]!
    const open = usage({
      rangeEndEpoch: NOW + 3600,
      periods: [{ ...base, resetsAtEpoch: NOW + 3600 }],
    })
    const { session } = setup({
      getAccounts: vi.fn().mockResolvedValue({ accounts: [twoLanes], generatedAt: "g" }),
      getUsage: vi.fn().mockResolvedValue(open),
    })
    sessions.push(session)
    await loaded()
    expect(rangeTrigger()).toHaveTextContent("This week")

    openJumpMenu("Lane")
    fireEvent.click(await screen.findByRole("menuitemradio", { name: "5-hour" }))
    await vi.waitFor(() => expect(session.getSnapshot().selection?.lane).toBe("fiveHour"))
    expect(rangeTrigger()).toHaveTextContent("This window")
    const figures = screen.getByRole("region", { name: "Limit usage" })
    expect(within(figures).getByText("This window")).toBeInTheDocument()
    openJumpMenu("Range")
    const menu = within(await screen.findByRole("menu"))
    expect(menu.getByRole("menuitemradio", { name: "Last 3 windows" })).toBeInTheDocument()
  })

  it("shows one account and one lane as plain text, not menus", async () => {
    const { session } = setup()
    sessions.push(session)
    const scope = within(await screen.findByRole("group", { name: "Limits scope" }))
    expect(scope.queryByRole("button", { name: /^Account: / })).not.toBeInTheDocument()
    expect(scope.queryByRole("button", { name: /^Lane: / })).not.toBeInTheDocument()
    expect(scope.getByText("Claude")).toBeInTheDocument()
    expect(scope.getByText("Weekly")).toBeInTheDocument()
    expect(scope.getByRole("button", { name: "Range: This week" })).toBeInTheDocument()
  })

  it("offers an account menu once a provider has two accounts", async () => {
    const { session } = setup({
      getAccounts: vi.fn().mockResolvedValue({
        accounts: [account(), account({ accountKey: "acct-2" })],
        generatedAt: "g",
      }),
    })
    sessions.push(session)
    await loaded()
    openJumpMenu("Account")
    const menu = within(await screen.findByRole("menu"))
    fireEvent.click(menu.getByRole("menuitemradio", { name: "Claude account 2" }))
    await vi.waitFor(() => expect(session.getSnapshot().selection?.accountKey).toBe("acct-2"))
  })

  it("reads the dates of a custom range from open() until a preset is picked", async () => {
    const { session } = setup()
    sessions.push(session)
    await loaded()
    expect(rangeTrigger()).not.toHaveTextContent("–")

    session.open(
      { provider: "anthropic", accountKey: "acct-1", lane: "weekly" },
      { startEpoch: NOW - WEEK, endEpoch: NOW },
    )
    await vi.waitFor(() => expect(rangeTrigger()).toHaveTextContent("–"))
    openJumpMenu("Range")
    const menu = within(await screen.findByRole("menu"))
    for (const item of menu.getAllByRole("menuitemradio")) {
      expect(item).toHaveAttribute("aria-checked", "false")
    }
    fireEvent.click(menu.getByRole("menuitemradio", { name: "This week" }))
    await vi.waitFor(() => expect(session.getSnapshot().range).toBe("thisWindow"))
    expect(rangeTrigger()).toHaveTextContent("This week")
    expect(rangeTrigger()).not.toHaveTextContent("–")
  })

  it("draws the window axis with the pace line on, and the pace switch changes what the chart receives", async () => {
    const { session } = setup()
    sessions.push(session)
    await screen.findByTestId("chart")
    expect(chart.props?.axisMode).toBe("window")
    expect(chart.props?.showPace).toBe(true)

    fireEvent.click(screen.getByRole("switch", { name: "Pace line" }))
    expect(chart.props?.showPace).toBe(false)
  })

  it("restores the pace switch from saved prefs", async () => {
    writeQuotaViewPrefs({ showPace: false })
    const { session } = setup()
    sessions.push(session)
    await screen.findByTestId("chart")
    expect(chart.props?.axisMode).toBe("window")
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
    // The list heading names the window every share is a share of, by
    // provider and lane, once, instead of every row repeating it.
    expect(
      screen.getByRole("heading", { name: "Sessions · share of a Claude weekly window" }),
    ).toBeInTheDocument()
    expect(within(list).getByRole("button", { name: /Session 1/ })).not.toHaveTextContent(
      "of a Claude",
    )
    fireEvent.mouseEnter(row)
    expect(wrapperHighlight()).toBe("other")
    fireEvent.mouseLeave(row)
    expect(wrapperHighlight()).toBeNull()
  })

  it("shows a session's agent icon, its share and dollars, and how many windows it spans", async () => {
    const { session } = setup({ getUsage: vi.fn().mockResolvedValue(multiWindowUsage()) })
    sessions.push(session)
    await loaded()
    await selectRangeOption("Last 3 weeks")
    const list = await screen.findByRole("region", { name: "Most prominent sessions" })
    const row = within(list).getByRole("button", { name: /Fix bug/ })
    expect(within(row).getByRole("img", { name: "Claude" })).toBeInTheDocument()
    expect(row).toHaveTextContent("across 3 weeks")
    expect(row).toHaveTextContent("90.0%")
    expect(row).toHaveTextContent("$9.00")
  })

  it("omits the windows caption on a row from a single window", async () => {
    const { session } = setup()
    sessions.push(session)
    const list = await screen.findByRole("region", { name: "Most prominent sessions" })
    const row = within(list).getByRole("button", { name: /Fix bug/ })
    expect(row).not.toHaveTextContent("across")
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
    await selectRangeOption("Last 5 weeks")
    expect(list.closest('[aria-busy="true"]')).not.toBeNull()

    pending.resolve(usage({ generatedAt: "g-5w" }))
    await vi.waitFor(() => expect(session.getSnapshot().loading).toBe(false))
    expect(list.closest('[aria-busy="true"]')).toBeNull()
  })
})
