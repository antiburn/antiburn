import { act, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type * as IpcModule from "../../lib/ipc"
import type * as SubjectModule from "../../lib/sessionSubject"
import * as SnoozedBurnChecks from "../../lib/snoozedBurnChecks"
import type { SessionHygienePayload } from "../../lib/insightsIpc"
import { type ActivityEntryPayload, DEFAULT_SETTINGS } from "../../lib/ipc"
import { localSessionKey } from "../../lib/presentation/localIdentity"
import { serializeSessionFilters, type SessionFilters } from "../../lib/sessionFilters"
import type { SessionHygieneSnapshot } from "../../lib/useSessionHygiene"
import { MainActivitySession } from "./MainActivitySession"
import type { SessionListEntry } from "../../components/session/SessionList"
import { MainActivityView } from "./MainActivityView"

const mocks = vi.hoisted(() => ({
  getSettings: vi.fn(),
  setSettings: vi.fn(),
  listRecentSessions: vi.fn(),
  getMainWindowVisible: vi.fn(),
  getLiveUsage: vi.fn(),
  getSessionLimitAllocations: vi.fn(),
  loadSessionAnalysis: vi.fn(),
  noteInteraction: vi.fn(),
  openSettingsWindow: vi.fn(),
}))

/** Every push channel `MainActivitySession` listens on, as a no-op unlisten. */
async function noListener(): Promise<() => void> {
  return () => {}
}

vi.mock("../../lib/ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof IpcModule>()
  return {
    ...actual,
    ...mocks,
    onMainWindowVisibilityChanged: noListener,
    onSettingsChanged: noListener,
    onSessionIndexChanged: noListener,
    onSessionUpdated: noListener,
    onLiveUsageChanged: noListener,
  }
})
vi.mock("../../lib/sessionSubject", async (importOriginal) => ({
  ...(await importOriginal<typeof SubjectModule>()),
  loadSessionAnalysis: mocks.loadSessionAnalysis,
}))
vi.mock("../popover/SessionPane", () => ({
  SessionPane: ({ subject }: { subject: { sessionId: string } }) => (
    <p>Detail: {subject.sessionId}</p>
  ),
}))

/** Two days ago: inside the default 7-day window, but not "today", so the
 *  view never auto-selects a session and the detail pane stays empty. */
function daysAgo(days: number): string {
  return new Date(Date.now() - days * 24 * 60 * 60 * 1000).toISOString()
}

function entry(id: string, extra: Partial<ActivityEntryPayload> = {}): ActivityEntryPayload {
  return {
    agent: "claude-code",
    surface: "cli",
    title: id,
    sessionId: id,
    repo: "example",
    timestamp: daysAgo(2),
    isActive: false,
    cost: null,
    models: [],
    modelRuns: [],
    hasForkParent: false,
    forkChildCount: 0,
    wslDistro: null,
    ...extra,
  } as ActivityEntryPayload
}

function hygieneFor(
  pairs: Array<[ActivityEntryPayload, SessionHygienePayload]>,
): SessionHygieneSnapshot {
  return new Map(
    pairs.map(([item, payload]) => [
      localSessionKey(item.agent, item.sessionId ?? "", item.wslDistro ?? null),
      payload,
    ]),
  )
}

const cleanHygiene: SessionHygienePayload = {
  badges: [{ id: "obsoleteModel", status: "clean", notAssessedReason: null }],
  evidenceState: "ready",
  unusedResources: null,
}

let sessions: MainActivitySession[]

beforeEach(() => {
  vi.clearAllMocks()
  sessions = []
  mocks.getSettings.mockResolvedValue(DEFAULT_SETTINGS)
  mocks.setSettings.mockImplementation(async (settings) => settings)
  mocks.getMainWindowVisible.mockResolvedValue(true)
  mocks.listRecentSessions.mockResolvedValue([])
  mocks.loadSessionAnalysis.mockResolvedValue(null)
  mocks.getLiveUsage.mockResolvedValue(null)
  mocks.getSessionLimitAllocations.mockResolvedValue(null)
  mocks.openSettingsWindow.mockResolvedValue(undefined)
})

afterEach(() => sessions.forEach((session) => session.dispose()))

function renderActivity(hygieneBySession: SessionHygieneSnapshot = new Map()) {
  const session = new MainActivitySession()
  sessions.push(session)
  const utils = render(
    <MainActivityView active session={session} hygieneBySession={hygieneBySession} />,
  )
  return { session, ...utils }
}

async function ready(session: MainActivitySession) {
  await vi.waitFor(() => expect(session.getSnapshot().entries).not.toBeNull())
}

describe("MainActivityView", () => {
  it("renders only the sessions the current filter selects", async () => {
    mocks.getSettings.mockResolvedValue({ ...DEFAULT_SETTINGS, sessionFilter: "material" })
    mocks.listRecentSessions.mockResolvedValue([
      entry("priced", {
        cost: { totalUsd: 5, inputUsd: 5, outputUsd: 0, cacheReadUsd: 0, cacheWriteUsd: 0 },
      }),
      entry("free", { cost: null }),
    ])
    const { session } = renderActivity()
    await ready(session)

    expect(await screen.findByText("priced")).toBeInTheDocument()
    expect(screen.queryByText("free")).toBeNull()
  })

  it("shows a filter-specific message when the filter excludes every loaded session", async () => {
    mocks.getSettings.mockResolvedValue({
      ...DEFAULT_SETTINGS,
      sessionFilter: "failing",
      sessionBadgeMetric: "weeklyPercent",
    })
    const one = entry("one")
    const two = entry("two")
    mocks.listRecentSessions.mockResolvedValue([one, two])
    const { session } = renderActivity(
      hygieneFor([
        [one, cleanHygiene],
        [two, cleanHygiene],
      ]),
    )
    await ready(session)

    // The title renders twice: once visible, once in a screen-reader-only
    // live region that announces the empty state.
    await vi.waitFor(() => expect(screen.getAllByText("No matching sessions")).toHaveLength(2))
    expect(screen.queryByText("one")).toBeNull()
    expect(screen.queryByText(/No sessions in the last/)).toBeNull()
    expect(screen.queryByRole("radiogroup", { name: "Session metric" })).toBeNull()
    expect(screen.queryByRole("button", { name: "Change filters" })).toBeNull()
    expect(screen.queryByText(/Try changing or clearing your filters/)).toBeNull()
    fireEvent.click(screen.getByRole("button", { name: "Clear filters" }))
    expect(await screen.findByLabelText("2 total sessions, 2 matching")).toBeVisible()
    expect(screen.getByText("one")).toBeVisible()
    expect(screen.getByRole("button", { name: "Filters" })).toHaveFocus()
    expect(session.getSnapshot().settings.activityWindowDays).toBe(7)
    expect(screen.getByRole("radio", { name: "Week %" })).toHaveAttribute(
      "aria-checked",
      "true",
    )
  })

  it("keeps the day-window empty copy when the list itself is empty", async () => {
    mocks.listRecentSessions.mockResolvedValue([])
    const { session } = renderActivity()
    await ready(session)

    await vi.waitFor(() =>
      expect(screen.getAllByText(/No sessions in the last \d+ days/)).toHaveLength(2),
    )
    expect(screen.queryByRole("button", { name: "Clear filters" })).toBeNull()
    fireEvent.click(screen.getByRole("button", { name: "Change time range" }))
    expect(mocks.openSettingsWindow).toHaveBeenCalledWith("general", "recentDays")
  })

  it("does not show an empty filtered result before snoozes are ready", async () => {
    const hook = vi
      .spyOn(SnoozedBurnChecks, "useSnoozedBurnChecks")
      .mockReturnValue({ status: "loading", records: [] })
    mocks.getSettings.mockResolvedValue({ ...DEFAULT_SETTINGS, sessionFilter: "failing" })
    mocks.listRecentSessions.mockResolvedValue([entry("stored-failure")])
    const { session, unmount } = renderActivity()
    await ready(session)

    expect(screen.getByText("Loading sessions…")).toBeVisible()
    expect(screen.queryByText("No matching sessions")).toBeNull()
    expect(screen.queryByText("stored-failure")).toBeNull()
    unmount()
    hook.mockRestore()
  })

  it("combines multiple agents with the other facets and renders the matching count", async () => {
    mocks.getSettings.mockResolvedValue({
      ...DEFAULT_SETTINGS,
      sessionFilter: serializeSessionFilters({
        agents: ["codex", "claude-code"],
        result: "passing",
        spend: "material",
      }),
    })
    const priced = { totalUsd: 2, inputUsd: 2, outputUsd: 0, cacheReadUsd: 0, cacheWriteUsd: 0 }
    const codex = entry("codex-priced", { agent: "codex", cost: priced })
    const claude = entry("claude-priced", { cost: priced })
    const cursor = entry("cursor-priced", { agent: "cursor", cost: priced })
    const free = entry("claude-free")
    mocks.listRecentSessions.mockResolvedValue([codex, claude, cursor, free])
    const { session } = renderActivity(
      hygieneFor([codex, claude, cursor, free].map((item) => [item, cleanHygiene])),
    )
    await ready(session)
    expect(await screen.findByLabelText("4 total sessions, 2 matching")).toBeVisible()
    expect(screen.getByText("codex-priced")).toBeVisible()
    expect(screen.getByText("claude-priced")).toBeVisible()
    expect(screen.queryByText("cursor-priced")).toBeNull()
    expect(screen.queryByText("claude-free")).toBeNull()
    fireEvent.pointerDown(screen.getByRole("button", { name: "Filters" }), {
      button: 0,
      ctrlKey: false,
      pointerType: "mouse",
    })
    fireEvent.click(await screen.findByRole("menuitem", { name: "Clear filters" }))
    expect(await screen.findByLabelText("4 total sessions, 4 matching")).toBeVisible()
    expect(screen.getByText("cursor-priced")).toBeVisible()
    expect(mocks.noteInteraction).toHaveBeenCalledWith({
      kind: "sessionFiltersChanged",
      action: "cleared_all",
    })
  })

  it("preserves an open detail when filters exclude it and leaves filters accessible", async () => {
    mocks.listRecentSessions.mockResolvedValue([entry("open-session", { agent: "codex" })])
    const { session } = renderActivity()
    await ready(session)
    act(() => session.selectEntry(session.getSnapshot().entries![0]!))
    expect(await screen.findByText("Detail: open-session")).toBeVisible()
    act(() => session.toggleAgent("claude-code"))
    expect(await screen.findByLabelText("1 total sessions, 0 matching")).toBeVisible()
    expect(screen.getByText("Detail: open-session")).toBeVisible()
    expect(screen.getByText("This item is outside the current list.")).toBeVisible()
    expect(screen.getByRole("button", { name: "Filters" })).toBeVisible()
    act(() => session.clearFilters())
    expect(await screen.findByLabelText("1 total sessions, 1 matching")).toBeVisible()
    expect(screen.queryByText("This item is outside the current list.")).toBeNull()
    expect(screen.getByText("Detail: open-session")).toBeVisible()
  })

  it("opens the shared time range from the header and reports navigation failures", async () => {
    const { session } = renderActivity()
    await ready(session)
    mocks.openSettingsWindow.mockRejectedValueOnce(new Error("unavailable"))
    fireEvent.click(
      screen.getByRole("button", { name: "Last 7 days, change time range in Settings" }),
    )
    expect(
      await screen.findByText("Could not open time-range settings. Try again."),
    ).toBeVisible()
    fireEvent.click(screen.getByRole("button", { name: "Change time range" }))
    await vi.waitFor(() =>
      expect(screen.queryByText("Could not open time-range settings. Try again.")).toBeNull(),
    )
    expect(mocks.openSettingsWindow).toHaveBeenLastCalledWith("general", "recentDays")
  })
})

describe("MainActivityView calendar scope", () => {
  function scopedView(entries: SessionListEntry[], filter = "all") {
    const session = new MainActivitySession()
    sessions.push(session)
    let snapshot = {
      ...session.getSnapshot(),
      entries,
      now: new Date(2026, 8, 23, 15).getTime(),
      filters: {
        agents: filter === "all" ? [] : [filter],
        result: "all",
        spend: "all",
      } as SessionFilters,
      settings: { ...DEFAULT_SETTINGS, sessionBadgeMetric: "cost" as const },
    }
    vi.spyOn(session, "subscribe").mockImplementation(() => () => {})
    vi.spyOn(session, "getSnapshot").mockImplementation(() => snapshot)
    const view = render(
      <MainActivityView active session={session} hygieneBySession={hygieneForRows(entries)} />,
    )
    return {
      session,
      update(next: Partial<typeof snapshot>) {
        snapshot = { ...snapshot, ...next }
        view.rerender(
          <MainActivityView
            active
            session={session}
            hygieneBySession={hygieneForRows(snapshot.entries)}
          />,
        )
      },
    }
  }

  function hygieneForRows(entries: SessionListEntry[]): SessionHygieneSnapshot {
    return new Map(
      entries.map((item) => [
        localSessionKey(item.agent, item.sessionId ?? "", item.wslDistro ?? null),
        cleanHygiene,
      ]),
    )
  }

  function row(
    title: string,
    timestamp: string,
    extra: Partial<SessionListEntry> = {},
  ): SessionListEntry {
    return {
      agent: "codex",
      sessionId: title,
      title,
      timestamp,
      repo: "fixture",
      isActive: false,
      ...extra,
    }
  }

  const current = new Date(2026, 8, 22, 12).toISOString()
  const rollingOnly = new Date(2026, 8, 16, 16).toISOString()

  function pricedRows(costs: number[], timestamp = current): SessionListEntry[] {
    return costs.map((totalUsd, index) =>
      row(`priced-${index}-${timestamp}`, timestamp, {
        cost: { totalUsd, figureLabel: "Estimated cost", isHighCost: false },
      }),
    )
  }

  function openFilters() {
    fireEvent.pointerDown(screen.getByRole("button", { name: "Filters" }), {
      button: 0,
      ctrlKey: false,
      pointerType: "mouse",
    })
  }

  it("uses the visible all-agent cohort for the threshold, badges, and facet counts", () => {
    const entries = pricedRows([0, 0, 0, 0, 0, 0, 2, 4])
    entries[0] = { ...entries[0]!, sessionId: undefined }
    entries[7] = { ...entries[7]!, agent: "claude-code" }
    const view = scopedView([
      ...entries,
      ...pricedRows(Array<number>(12).fill(100), rollingOnly),
    ])
    expect(screen.getByLabelText("Estimated cost $4.00, higher than usual")).toBeVisible()
    expect(screen.getByLabelText("Estimated cost $2.00")).toBeVisible()
    expect(screen.getByLabelText("8 total sessions, 8 matching")).toBeVisible()
    openFilters()
    expect(
      screen.getByRole("menuitemradio", {
        name: /High cost, Over \$2(?: · auto)?, 1 matching session/,
      }),
    ).toBeEnabled()
    fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" })
    view.update({ filters: { agents: ["claude-code"], result: "passing", spend: "notable" } })
    expect(screen.getByLabelText("8 total sessions, 1 matching")).toBeVisible()
    expect(screen.getByLabelText("Estimated cost $4.00, higher than usual")).toBeVisible()
    openFilters()
    expect(
      screen.getByRole("menuitemradio", {
        name: /High cost, Over \$2(?: · auto)?, 1 matching session/,
      }),
    ).toHaveAttribute("aria-checked", "true")
  })

  it("does not reach the minimum sample with spillover or unknown costs", () => {
    const entries = pricedRows([0, 0, 0, 0, 0, 0, 9])
    entries[6]!.cost!.isHighCost = true
    scopedView([...entries, row("unknown", current), ...pricedRows([0], rollingOnly)])
    expect(screen.getByLabelText("8 total sessions, 8 matching")).toBeVisible()
    expect(screen.getByLabelText("Estimated cost $9.00")).toBeVisible()
    expect(screen.queryByLabelText(/higher than usual/)).toBeNull()
    openFilters()
    expect(
      screen.getByRole("menuitemradio", { name: /High cost,.*0 matching sessions/ }),
    ).toHaveAttribute("aria-disabled", "true")
    expect(screen.queryByText(/Over \$/)).toBeNull()
  })

  it("refreshes the threshold as the time scope, date, and costs change", () => {
    const entries = pricedRows([1, 1, 1, 1, 1, 1, 1, 4])
    const view = scopedView([...entries, ...pricedRows(Array<number>(9).fill(10), rollingOnly)])
    expect(screen.getByLabelText("Estimated cost $4.00, higher than usual")).toBeVisible()
    view.update({
      settings: { ...DEFAULT_SETTINGS, activityWindowDays: 8, sessionBadgeMetric: "cost" },
    })
    expect(screen.getByLabelText("Estimated cost $4.00")).toBeVisible()
    openFilters()
    expect(
      screen.getByRole("menuitemradio", {
        name: /High cost, Over \$30(?: · auto)?, 0 matching sessions/,
      }),
    ).toBeInTheDocument()
    fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" })
    view.update({ now: new Date(2026, 8, 24, 15).getTime() })
    expect(screen.getByLabelText("Estimated cost $4.00, higher than usual")).toBeVisible()
    view.update({
      entries: entries.map((item) => ({ ...item, cost: { ...item.cost!, totalUsd: 4 } })),
    })
    expect(screen.queryByLabelText(/higher than usual/)).toBeNull()
    openFilters()
    expect(
      screen.getByRole("menuitemradio", {
        name: /High cost, Over \$12(?: · auto)?, 0 matching sessions/,
      }),
    ).toBeInTheDocument()
  })

  it("counts active out-of-range prices until those sessions become inactive", () => {
    const entries = pricedRows([0, 0, 0, 0, 0, 0, 9])
    const active = row("active-price", rollingOnly, {
      isActive: true,
      cost: { totalUsd: 0, figureLabel: "Estimated cost" },
    })
    const view = scopedView([...entries, active])
    expect(screen.getByLabelText("Estimated cost $9.00, higher than usual")).toBeVisible()
    view.update({ entries: [...entries, { ...active, isActive: false }] })
    expect(screen.getByLabelText("7 total sessions, 7 matching")).toBeVisible()
    expect(screen.getByLabelText("Estimated cost $9.00")).toBeVisible()
  })

  it("excludes rolling-window spillover before counts and empty-state decisions", () => {
    scopedView(
      [
        row("calendar-visible", current),
        row("rolling-only", rollingOnly, { agent: "claude-code" }),
      ],
      "claude-code",
    )
    expect(screen.getByLabelText("1 total sessions, 0 matching")).toBeVisible()
    expect(screen.getAllByText("No matching sessions")).toHaveLength(2)
    expect(
      screen.getByRole("button", { name: "Remove Claude Code filter, 0 matching sessions" }),
    ).toBeEnabled()
    expect(screen.queryByText("rolling-only")).toBeNull()
    expect(screen.queryByRole("radiogroup", { name: "Session metric" })).toBeNull()
  })

  it("uses the true range-empty state when only excluded records are loaded", () => {
    scopedView([row("rolling-only", rollingOnly)], "claude-code")
    expect(screen.getByLabelText("0 total sessions, 0 matching")).toBeVisible()
    expect(screen.getAllByText("No sessions in the last 7 days")).toHaveLength(2)
    expect(screen.queryByRole("button", { name: "Clear filters" })).toBeNull()
    expect(screen.getByRole("button", { name: "Change time range" })).toBeVisible()
  })

  it("counts visible rows without an id and rejects inactive invalid or future dates", () => {
    scopedView([
      row("no-id", current, { sessionId: undefined }),
      row("invalid", "invalid"),
      row("future", new Date(2026, 8, 24, 12).toISOString()),
    ])
    expect(screen.getByLabelText("1 total sessions, 1 matching")).toBeVisible()
    expect(screen.getByText("no-id")).toBeVisible()
    expect(screen.queryByText("invalid")).toBeNull()
    expect(screen.queryByText("future")).toBeNull()
  })

  it.each(["invalid", rollingOnly, new Date(2026, 8, 24, 12).toISOString()])(
    "keeps active rows and updates counts when they become inactive (%s)",
    (timestamp) => {
      const active = row("active-session", timestamp, { isActive: true })
      const view = scopedView([active])
      expect(screen.getByLabelText("1 total sessions, 1 matching")).toBeVisible()
      expect(screen.getByText("active-session")).toBeVisible()
      view.update({ entries: [{ ...active, isActive: false }] })
      expect(screen.getByLabelText("0 total sessions, 0 matching")).toBeVisible()
      expect(screen.queryByText("active-session")).toBeNull()
      expect(screen.queryByRole("radiogroup", { name: "Session metric" })).toBeNull()
    },
  )
})
