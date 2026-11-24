import { act, fireEvent, render, screen } from "@testing-library/react"
import { useSyncExternalStore } from "react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type * as IpcModule from "../../lib/ipc"
import type * as SubjectModule from "../../lib/sessionSubject"
import * as SnoozedBurnChecks from "../../lib/snoozedBurnChecks"
import * as InsightsIpc from "../../lib/insightsIpc"
import type { SessionHygienePayload } from "../../lib/insightsIpc"
import { type ActivityEntryPayload, DEFAULT_SETTINGS } from "../../lib/ipc"
import { localSessionKey } from "../../lib/presentation/localIdentity"
import { serializeSessionFilters, type SessionFilters } from "../../lib/sessionFilters"
import {
  sessionHygieneIdentities,
  useSessionHygieneResource,
  type SessionHygieneResource,
  type SessionHygieneSnapshot,
} from "../../lib/useSessionHygiene"
import { MainActivitySession } from "./MainActivitySession"
import type { SessionListEntry } from "../../components/session/SessionList"
import { MainActivityView } from "./MainActivityView"

const mocks = vi.hoisted(() => ({
  getSettings: vi.fn(),
  setSettings: vi.fn(),
  listRecentSessions: vi.fn(),
  openSessionActivityStream: vi.fn(),
  getMainWindowVisible: vi.fn(),
  getLiveUsage: vi.fn(),
  getSessionLimitAllocations: vi.fn(),
  loadSessionAnalysis: vi.fn(),
  noteInteraction: vi.fn(),
}))

const onOpenRangeSettings = vi.fn()

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
  SessionPane: ({
    subject,
    onPrev,
    onNext,
  }: {
    subject: { sessionId: string }
    onPrev?: () => void
    onNext?: () => void
  }) => (
    <p
      onKeyDown={(event) => {
        if (event.key === "ArrowRight") onNext?.()
        if (event.key === "ArrowLeft") onPrev?.()
      }}
    >
      Detail: {subject.sessionId}
    </p>
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

function batchEndingAt(current: ActivityEntryPayload): ActivityEntryPayload[] {
  const yesterday = daysAgo(1)
  return [
    ...Array.from({ length: 49 }, (_, index) =>
      entry(`newer-${index}`, { timestamp: yesterday }),
    ),
    current,
  ]
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
  mocks.openSessionActivityStream.mockImplementation((days: number) => ({
    read: async () => ({ entries: await mocks.listRecentSessions(days), exhausted: true }),
    close: () => {},
  }))
  mocks.getSettings.mockResolvedValue(DEFAULT_SETTINGS)
  mocks.setSettings.mockImplementation(async (settings) => settings)
  mocks.getMainWindowVisible.mockResolvedValue(true)
  mocks.listRecentSessions.mockResolvedValue([])
  mocks.loadSessionAnalysis.mockResolvedValue(null)
  mocks.getLiveUsage.mockResolvedValue(null)
  mocks.getSessionLimitAllocations.mockResolvedValue(null)
  onOpenRangeSettings.mockClear()
})

afterEach(() => {
  sessions.forEach((session) => session.dispose())
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
})

function fixedHygiene(snapshot: SessionHygieneSnapshot) {
  return {
    snapshot,
    getSnapshot: () => snapshot,
    resolve: vi.fn().mockResolvedValue(false),
  }
}

function renderActivity(
  hygieneBySession: SessionHygieneSnapshot = new Map(),
  hygiene: SessionHygieneResource = fixedHygiene(hygieneBySession),
) {
  const session = new MainActivitySession()
  sessions.push(session)
  const utils = render(
    <MainActivityView
      active
      session={session}
      sessionHygiene={hygiene}
      onOpenRangeSettings={onOpenRangeSettings}
    />,
  )
  return { session, ...utils }
}

function renderActivityWithHygiene() {
  const session = new MainActivitySession()
  sessions.push(session)
  function Activity() {
    const snapshot = useSyncExternalStore(session.subscribeList, session.getSnapshot)
    const hygiene = useSessionHygieneResource(sessionHygieneIdentities(snapshot.entries ?? []))
    return <MainActivityView active session={session} sessionHygiene={hygiene} />
  }
  return { session, ...render(<Activity />) }
}

async function ready(session: MainActivitySession) {
  await vi.waitFor(() => expect(session.getSnapshot().entries).not.toBeNull())
}

function mockPendingCompactAppend(loadedMatch = false) {
  vi.stubGlobal("innerWidth", 550)
  const first = batchEndingAt(entry("current"))
  const matched = entry("matched", { timestamp: daysAgo(3) })
  if (loadedMatch) first[0] = matched
  type Batch = { entries: ActivityEntryPayload[]; exhausted: boolean }
  let finishBatch!: (value: Batch) => void
  let rejectBatch!: (reason: Error) => void
  const pendingBatch = new Promise<Batch>((resolve, reject) => {
    finishBatch = resolve
    rejectBatch = reject
  })
  const read = vi
    .fn()
    .mockResolvedValueOnce({ entries: first, exhausted: false })
    .mockReturnValueOnce(pendingBatch)
  mocks.openSessionActivityStream.mockReturnValueOnce({ read, close: vi.fn() })

  return { first, matched, read, finishBatch, rejectBatch }
}

async function startCompactAppend(
  session: MainActivitySession,
  read: ReturnType<typeof vi.fn>,
  dismiss = true,
) {
  act(() => {
    session.selectEntry(
      session.getSnapshot().entries!.find((row) => row.sessionId === "current")!,
    )
    session.revealDetail()
  })
  fireEvent.keyDown(await screen.findByText("Detail: current"), { key: "ArrowRight" })
  await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(2))
  if (dismiss) fireEvent.click(screen.getByRole("button", { name: "Back to sessions" }))
}

async function reopenDuringAppend(session: MainActivitySession) {
  act(() => session.revealDetail())
  fireEvent.keyDown(await screen.findByText("Detail: current"), { key: "ArrowRight" })
  expect(
    await screen.findByText("Loading more sessions…", {
      selector: "[data-detail-pane] [role=status]",
    }),
  ).toBeVisible()
}

describe("MainActivityView", () => {
  it("reports compact detail only after a committed reveal and rotates the visit on return", async () => {
    vi.stubGlobal("innerWidth", 550)
    mocks.listRecentSessions.mockResolvedValue([entry("private-source-id")])
    const { session } = renderActivity()
    await ready(session)

    act(() => session.selectEntry(session.getSnapshot().entries![0]!))
    expect(mocks.noteInteraction).not.toHaveBeenCalledWith(
      expect.objectContaining({ kind: "sessionDetailViewed" }),
    )

    act(() => session.revealDetail())
    expect(await screen.findByText("Detail: private-source-id")).toBeVisible()
    fireEvent.click(screen.getByRole("button", { name: "Back to sessions" }))
    act(() => session.revealDetail())
    expect(await screen.findByText("Detail: private-source-id")).toBeVisible()

    const views = mocks.noteInteraction.mock.calls
      .map(([payload]) => payload)
      .filter(({ kind }) => kind === "sessionDetailViewed")
    expect(views).toHaveLength(2)
    expect(views[0].visit).not.toBe(views[1].visit)
    expect(views.map(({ context }) => context.entryRoute)).toEqual(["session_list", "return"])
    expect(JSON.stringify(views)).not.toContain("private-source-id")
  })

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
    expect(await screen.findByText("2 total sessions, 2 matching")).toBeVisible()
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
    expect(onOpenRangeSettings).toHaveBeenCalledOnce()
  })

  it("can clear filters from the header while no loaded rows match and older rows remain", async () => {
    const read = vi
      .fn()
      .mockResolvedValueOnce({ entries: [entry("one")], exhausted: false })
      .mockReturnValue(new Promise(() => {}))
    mocks.openSessionActivityStream.mockReturnValueOnce({ read, close: vi.fn() })
    mocks.getSettings.mockResolvedValue({ ...DEFAULT_SETTINGS, sessionFilter: "failing" })
    const { session } = renderActivity()
    await ready(session)
    await screen.findByText("1 loaded sessions, 0 matching")
    expect(screen.queryByText("No matching sessions")).toBeNull()
    expect(read).toHaveBeenCalledOnce()

    fireEvent.pointerDown(screen.getByRole("button", { name: "Filters" }), {
      button: 0,
      ctrlKey: false,
      pointerType: "mouse",
    })
    fireEvent.click(await screen.findByRole("menuitem", { name: "Clear filters" }))
    expect(await screen.findByText("one")).toBeVisible()
  })

  it.each(["processing", "stale"] as const)(
    "shows the filtered empty state after %s check evidence settles",
    async (evidenceState) => {
      const one = entry("one")
      const excluded = entry("excluded", { agent: "codex" })
      const incomplete: SessionHygienePayload = {
        badges: [],
        evidenceState,
        unusedResources: null,
      }
      mocks.getSettings.mockResolvedValue({
        ...DEFAULT_SETTINGS,
        sessionFilter: serializeSessionFilters({
          agents: [one.agent],
          result: "failing",
          spend: "all",
        }),
      })
      mocks.listRecentSessions.mockResolvedValue([one, excluded])
      const { session, rerender } = renderActivity(
        hygieneFor([
          [one, incomplete],
          [excluded, incomplete],
        ]),
      )
      await ready(session)

      const note = "Some sessions still need check results."
      expect(screen.getByText(note)).toBeVisible()
      expect(screen.getByRole("button", { name: "Clear filters" })).toBeVisible()
      expect(screen.queryByText("No matching sessions")).toBeNull()
      expect(screen.getByText("0 matching sessions among loaded sessions")).toBeInTheDocument()

      rerender(
        <MainActivityView
          active
          session={session}
          sessionHygiene={fixedHygiene(
            hygieneFor([
              [one, cleanHygiene],
              [excluded, incomplete],
            ]),
          )}
        />,
      )
      expect(screen.queryByText(note)).toBeNull()
      expect(screen.getAllByText("No matching sessions")).toHaveLength(2)
    },
  )

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
    expect(await screen.findByText("4 total sessions, 2 matching")).toBeVisible()
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
    expect(await screen.findByText("4 total sessions, 4 matching")).toBeVisible()
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
    expect(await screen.findByText("1 total sessions, 0 matching")).toBeVisible()
    expect(mocks.noteInteraction).toHaveBeenCalledWith({
      kind: "sessionFilterResultsObserved",
      result: "no_matches",
    })
    expect(screen.getByText("Detail: open-session")).toBeVisible()
    expect(screen.getByText("This item is outside the current list.")).toBeVisible()
    expect(screen.getByRole("button", { name: "Filters" })).toBeVisible()
    act(() => session.clearFilters())
    expect(await screen.findByText("1 total sessions, 1 matching")).toBeVisible()
    expect(screen.queryByText("This item is outside the current list.")).toBeNull()
    expect(screen.getByText("Detail: open-session")).toBeVisible()
  })

  it("loads another batch from keyboard detail navigation and selects the next passing session", async () => {
    const current = entry("current")
    const first = batchEndingAt(current)
    const rejected = entry("rejected", { timestamp: daysAgo(3) })
    const matched = entry("matched", { timestamp: daysAgo(4) })
    mocks.getSettings.mockResolvedValue({ ...DEFAULT_SETTINGS, sessionFilter: "passing" })
    const read = vi
      .fn()
      .mockResolvedValueOnce({ entries: first, exhausted: false })
      .mockResolvedValueOnce({ entries: [rejected, matched], exhausted: true })
    mocks.openSessionActivityStream.mockReturnValueOnce({ read, close: vi.fn() })
    const hygiene = vi
      .spyOn(InsightsIpc, "getSessionHygiene")
      .mockImplementation(async (rows) =>
        rows.map((row) =>
          row.sessionId === "rejected" ? { ...cleanHygiene, badges: [] } : cleanHygiene,
        ),
      )
    const { session } = renderActivityWithHygiene()
    await ready(session)
    await screen.findByText("50 loaded sessions, 50 matching")
    act(() => session.selectEntry(session.getSnapshot().entries!.at(-1)!))
    fireEvent.keyDown(await screen.findByText("Detail: current"), { key: "ArrowRight" })
    expect(await screen.findByText("Detail: matched")).toBeVisible()
    expect(read).toHaveBeenCalledTimes(2)
    expect(
      hygiene.mock.calls.some(([rows]) => rows.some((row) => row.sessionId === "matched")),
    ).toBe(true)
    hygiene.mockRestore()
  })

  it("checks a closer predecessor before choosing a previous match for a linked session", async () => {
    vi.stubGlobal("innerWidth", 550)
    const farther = entry("farther", { timestamp: daysAgo(1) })
    const nearer = entry("nearer", { timestamp: daysAgo(2) })
    const linked = entry("linked", { timestamp: daysAgo(3) })
    mocks.getSettings.mockResolvedValue({ ...DEFAULT_SETTINGS, sessionFilter: "passing" })
    const read = vi
      .fn()
      .mockResolvedValueOnce({ entries: batchEndingAt(farther), exhausted: false })
      .mockResolvedValueOnce({ entries: [nearer, linked], exhausted: true })
    mocks.openSessionActivityStream.mockReturnValueOnce({ read, close: vi.fn() })
    vi.spyOn(InsightsIpc, "getSessionHygiene").mockImplementation(async (rows) => {
      if (rows.some((row) => row.sessionId === "linked")) return new Promise(() => {})
      return rows.map(() => cleanHygiene)
    })
    const { session } = renderActivityWithHygiene()
    await ready(session)
    await screen.findByText("50 loaded sessions, 50 matching")
    act(() => {
      session.openRelated({ agent: linked.agent, sessionId: "linked" })
      session.revealDetail()
    })
    fireEvent.keyDown(await screen.findByText("Detail: linked"), { key: "ArrowLeft" })
    expect(await screen.findByText("Detail: nearer")).toBeVisible()
    expect(read).toHaveBeenCalledTimes(2)
  })

  it("checks nearer rows before jumping to a known loaded match, including after a badge is evicted", async () => {
    const current = entry("current", { timestamp: daysAgo(1) })
    const nearer = entry("nearer", { timestamp: daysAgo(2) })
    const middle = entry("middle", { timestamp: daysAgo(3) })
    const farther = entry("farther", { timestamp: daysAgo(4) })
    const noMatch: SessionHygienePayload = { ...cleanHygiene, badges: [] }
    mocks.getSettings.mockResolvedValue({ ...DEFAULT_SETTINGS, sessionFilter: "passing" })
    mocks.listRecentSessions.mockResolvedValue([current, nearer, middle, farther])
    const initial = hygieneFor([
      [current, cleanHygiene],
      [middle, noMatch],
      [farther, cleanHygiene],
    ])
    let badges = initial
    const resolve = vi
      .fn()
      .mockImplementationOnce(async () => {
        badges = hygieneFor([
          [nearer, { ...noMatch, evidenceState: "failed" }],
          [farther, cleanHygiene],
        ])
        return true
      })
      .mockImplementationOnce(async () => {
        badges = hygieneFor([
          [middle, noMatch],
          [farther, cleanHygiene],
        ])
        return true
      })
      .mockImplementationOnce(async () => {
        badges = hygieneFor([
          [nearer, cleanHygiene],
          [middle, noMatch],
          [farther, cleanHygiene],
        ])
        return true
      })
    const { session } = renderActivity(initial, {
      snapshot: initial,
      getSnapshot: () => badges,
      resolve,
    })
    await ready(session)
    act(() => session.selectEntry(session.getSnapshot().entries![0]!))

    fireEvent.keyDown(await screen.findByText("Detail: current"), { key: "ArrowRight" })
    expect(await screen.findByText("Detail: nearer")).toBeVisible()
    expect(
      resolve.mock.calls.map(([identities]) =>
        identities.map((row: { sessionId: string }) => row.sessionId),
      ),
    ).toEqual([["nearer"], ["middle"], ["nearer"]])
  })

  it.each([
    { evidenceState: "pending", cachedPassing: false },
    { evidenceState: "processing", cachedPassing: false },
    { evidenceState: "stale", cachedPassing: false },
    { evidenceState: "stale", cachedPassing: true },
  ] as const)(
    "keeps the current detail while the nearest result is unresolved (%j)",
    async ({ evidenceState, cachedPassing }) => {
      const current = entry("current", { timestamp: daysAgo(1) })
      const nearer = entry("nearer", { timestamp: daysAgo(2) })
      const farther = entry("farther", { timestamp: daysAgo(3) })
      mocks.getSettings.mockResolvedValue({ ...DEFAULT_SETTINGS, sessionFilter: "passing" })
      mocks.listRecentSessions.mockResolvedValue([current, nearer, farther])
      const initial = hygieneFor([
        [current, cleanHygiene],
        [
          nearer,
          { ...cleanHygiene, evidenceState, badges: cachedPassing ? cleanHygiene.badges : [] },
        ],
        [farther, cleanHygiene],
      ])
      let badges = initial
      const resolve = vi
        .fn<SessionHygieneResource["resolve"]>()
        .mockResolvedValueOnce(true)
        .mockImplementationOnce(async () => {
          badges = hygieneFor([
            [current, cleanHygiene],
            [nearer, cleanHygiene],
            [farther, cleanHygiene],
          ])
          return true
        })
      const { session } = renderActivity(initial, {
        snapshot: initial,
        getSnapshot: () => badges,
        resolve,
      })
      await ready(session)
      act(() => session.selectEntry(session.getSnapshot().entries![0]!))

      fireEvent.keyDown(await screen.findByText("Detail: current"), { key: "ArrowRight" })
      expect(
        await screen.findByText("Check results are still pending. Press Right Arrow to retry."),
      ).toBeVisible()
      expect(session.getSnapshot().subject?.sessionId).toBe("current")
      expect(resolve.mock.calls.map(([rows]) => rows.map((row) => row.sessionId))).toEqual([
        ["nearer"],
      ])

      fireEvent.keyDown(screen.getByText("Detail: current"), { key: "ArrowRight" })
      expect(await screen.findByText("Detail: nearer")).toBeVisible()
    },
  )

  it("releases an adjacent request when the selected session changes during its append", async () => {
    const { first, matched, read, finishBatch } = mockPendingCompactAppend()
    mocks.getSettings.mockResolvedValue({ ...DEFAULT_SETTINGS, sessionFilter: "passing" })
    const { session } = renderActivity(hygieneFor(first.map((row) => [row, cleanHygiene])))
    await ready(session)
    await startCompactAppend(session, read, false)

    act(() => session.selectEntry(session.getSnapshot().entries![1]!))
    const newSubject = session.getSnapshot().subject!.sessionId
    const previous = session.getSnapshot().entries![0]!.sessionId
    fireEvent.keyDown(await screen.findByText(`Detail: ${newSubject}`), { key: "ArrowLeft" })
    expect(await screen.findByText(`Detail: ${previous}`)).toBeVisible()
    await act(async () => finishBatch({ entries: [matched], exhausted: true }))
    expect(session.getSnapshot().subject?.sessionId).toBe(previous)
    expect(session.getSnapshot().entries?.some((row) => row.sessionId === "matched")).toBe(true)
  })

  it("opens the previous loaded match after compact detail reopens during a forward append", async () => {
    const { first, matched, read, finishBatch } = mockPendingCompactAppend()
    mocks.getSettings.mockResolvedValue({ ...DEFAULT_SETTINGS, sessionFilter: "passing" })
    const { session } = renderActivity(hygieneFor(first.map((row) => [row, cleanHygiene])))
    await ready(session)
    await startCompactAppend(session, read)

    act(() => session.revealDetail())
    fireEvent.keyDown(await screen.findByText("Detail: current"), { key: "ArrowLeft" })
    const previous = first.at(-2)!.sessionId
    expect(await screen.findByText(`Detail: ${previous}`)).toBeVisible()
    expect(session.getSnapshot().loadingMoreSessions).toBe(true)

    await act(async () => finishBatch({ entries: [matched], exhausted: true }))
    expect(session.getSnapshot().subject?.sessionId).toBe(previous)
    expect(read).toHaveBeenCalledTimes(2)
  })

  it("uses current snoozes to choose a match after an append", async () => {
    const { first, matched, read, finishBatch } = mockPendingCompactAppend()
    const farther = entry("farther", { timestamp: daysAgo(4) })
    const secondFailure: SessionHygienePayload = {
      ...cleanHygiene,
      badges: [{ id: "sessionOverdepth", status: "finding", notAssessedReason: null }],
    }
    const firstFailure: SessionHygienePayload = {
      ...cleanHygiene,
      badges: [{ id: "obsoleteModel", status: "finding", notAssessedReason: null }],
    }
    const snoozes = vi.spyOn(SnoozedBurnChecks, "getSnoozedBurnChecksSnapshot")
    mocks.getSettings.mockResolvedValue({ ...DEFAULT_SETTINGS, sessionFilter: "failing" })
    const { session } = renderActivity(
      hygieneFor([
        ...first.map((row): [ActivityEntryPayload, SessionHygienePayload] => [
          row,
          firstFailure,
        ]),
        [matched, firstFailure],
        [farther, secondFailure],
      ]),
    )
    await ready(session)
    await startCompactAppend(session, read, false)
    snoozes.mockReturnValue({
      status: "ready",
      records: [{ detector: "oldModelUsage", scope: "check", until: null }],
    })
    await act(async () => finishBatch({ entries: [matched, farther], exhausted: true }))
    expect(await screen.findByText("Detail: farther")).toBeVisible()
  })

  it("keeps the current detail if snoozes become unavailable during a check read", async () => {
    const current = entry("current", { timestamp: daysAgo(1) })
    const nearer = entry("nearer", { timestamp: daysAgo(2) })
    const farther = entry("farther", { timestamp: daysAgo(3) })
    mocks.getSettings.mockResolvedValue({ ...DEFAULT_SETTINGS, sessionFilter: "passing" })
    mocks.listRecentSessions.mockResolvedValue([current, nearer, farther])
    let snoozes: SnoozedBurnChecks.SnoozedBurnChecksState = { status: "ready", records: [] }
    vi.spyOn(SnoozedBurnChecks, "useSnoozedBurnChecks").mockImplementation(() => snoozes)
    vi.spyOn(SnoozedBurnChecks, "getSnoozedBurnChecksSnapshot").mockImplementation(
      () => snoozes,
    )
    let badges = hygieneFor([
      [current, cleanHygiene],
      [farther, cleanHygiene],
    ])
    let finish!: (resolved: boolean) => void
    const resolve = vi.fn().mockReturnValue(
      new Promise<boolean>((done) => {
        finish = done
      }),
    )
    const hygiene = { snapshot: badges, getSnapshot: () => badges, resolve }
    const { session, rerender, unmount } = renderActivity(badges, hygiene)
    await ready(session)
    act(() => session.selectEntry(session.getSnapshot().entries![0]!))
    fireEvent.keyDown(await screen.findByText("Detail: current"), { key: "ArrowRight" })
    expect(resolve).toHaveBeenCalledOnce()

    snoozes = { status: "error", records: [] }
    badges = hygieneFor([
      [current, cleanHygiene],
      [nearer, cleanHygiene],
      [farther, cleanHygiene],
    ])
    await act(async () => finish(true))
    expect(session.getSnapshot().subject?.sessionId).toBe("current")
    expect(
      screen.getByText("Sessions are unavailable.", {
        selector: "[data-detail-pane] [role=status]",
      }),
    ).toBeVisible()

    snoozes = { status: "ready", records: [] }
    rerender(
      <MainActivityView
        active
        session={session}
        sessionHygiene={{ ...hygiene, snapshot: badges }}
      />,
    )
    fireEvent.keyDown(screen.getByText("Detail: current"), { key: "ArrowRight" })
    expect(await screen.findByText("Detail: nearer")).toBeVisible()
    unmount()
  })

  it("does not search older batches from a subagent detail", async () => {
    const { read } = mockPendingCompactAppend()
    const { session } = renderActivity()
    await ready(session)
    act(() => {
      session.openRelated({
        agent: "claude-code",
        sessionId: "subagent",
        subagent: { parentSessionId: "current", subagentId: "subagent" },
      })
      session.revealDetail()
    })
    const detail = await screen.findByText("Detail: subagent")
    expect(session.getSnapshot().hasMoreSessions).toBe(true)
    expect(screen.queryByText(/Arrow to find this session/)).toBeNull()
    fireEvent.keyDown(detail, { key: "ArrowLeft" })
    fireEvent.keyDown(detail, { key: "ArrowRight" })
    expect(read).toHaveBeenCalledOnce()
  })

  it("keeps Right Arrow enabled after locating a filtered-out linked session", async () => {
    vi.stubGlobal("innerWidth", 550)
    const current = entry("current", { agent: "codex" })
    const linked = entry("linked", { timestamp: daysAgo(3) })
    const nonMatch = entry("non-match", { timestamp: daysAgo(4) })
    const match = entry("match", { agent: "codex", timestamp: daysAgo(5) })
    const read = vi
      .fn()
      .mockResolvedValueOnce({
        entries: batchEndingAt(current).map((row) => ({ ...row, agent: "codex" })),
        exhausted: false,
      })
      .mockResolvedValueOnce({ entries: [linked, nonMatch], exhausted: false })
      .mockResolvedValueOnce({ entries: [match], exhausted: true })
    mocks.openSessionActivityStream.mockReturnValueOnce({ read, close: vi.fn() })
    const { session } = renderActivity()
    await ready(session)
    act(() => {
      session.toggleAgent("codex")
      session.openRelated({ agent: linked.agent, sessionId: "linked" })
      session.revealDetail()
    })
    fireEvent.keyDown(await screen.findByText("Detail: linked"), { key: "ArrowRight" })
    expect(await screen.findByText(/No matches in this batch/)).toBeVisible()
    fireEvent.keyDown(screen.getByText("Detail: linked"), { key: "ArrowRight" })
    expect(await screen.findByText("Detail: match")).toBeVisible()
    expect(read).toHaveBeenCalledTimes(3)
  })

  it("reads an unchecked loaded successor before requesting another batch", async () => {
    const { first, matched, read } = mockPendingCompactAppend(true)
    mocks.getSettings.mockResolvedValue({ ...DEFAULT_SETTINGS, sessionFilter: "passing" })
    const initial = hygieneFor(first.slice(1).map((row) => [row, cleanHygiene]))
    let badges = initial
    const resolve = vi.fn<SessionHygieneResource["resolve"]>().mockImplementation(async () => {
      badges = hygieneFor(first.map((row) => [row, cleanHygiene]))
      return true
    })
    const { session } = renderActivity(initial, {
      snapshot: initial,
      getSnapshot: () => badges,
      resolve,
    })
    await ready(session)
    act(() => {
      session.selectEntry(
        session.getSnapshot().entries!.find((row) => row.sessionId === "current")!,
      )
      session.revealDetail()
    })

    fireEvent.keyDown(await screen.findByText("Detail: current"), { key: "ArrowRight" })
    expect(read).toHaveBeenCalledOnce()
    expect(resolve.mock.calls.map(([rows]) => rows.map((row) => row.sessionId))).toEqual([
      [matched.sessionId],
    ])
    expect(await screen.findByText("Detail: matched")).toBeVisible()
    expect(read).toHaveBeenCalledOnce()
  })

  it("opens a newly matching loaded successor while the original compact append is pending", async () => {
    const { first, matched, read, finishBatch } = mockPendingCompactAppend(true)
    const older = entry("older", { timestamp: daysAgo(4) })
    mocks.getSettings.mockResolvedValue({ ...DEFAULT_SETTINGS, sessionFilter: "passing" })
    const initial = hygieneFor(
      first.map((row) => [
        row,
        row === matched ? { ...cleanHygiene, badges: [] } : cleanHygiene,
      ]),
    )
    let badges = initial
    const { session } = renderActivity(initial, {
      ...fixedHygiene(initial),
      getSnapshot: () => badges,
    })
    await ready(session)
    await startCompactAppend(session, read)

    badges = hygieneFor(first.map((row) => [row, cleanHygiene]))
    act(() => session.revealDetail())
    fireEvent.keyDown(await screen.findByText("Detail: current"), { key: "ArrowRight" })
    expect(await screen.findByText("Detail: matched")).toBeVisible()
    expect(session.getSnapshot().loadingMoreSessions).toBe(true)
    expect(read).toHaveBeenCalledTimes(2)

    await act(async () => finishBatch({ entries: [older], exhausted: true }))
    expect(session.getSnapshot().subject?.sessionId).toBe("matched")
    expect(session.getSnapshot().entries?.some((row) => row.sessionId === "older")).toBe(true)
  })

  it("reopens and retries checks after the final batch, then updates the filtered list", async () => {
    vi.stubGlobal("innerWidth", 550)
    const current = entry("current")
    const first = batchEndingAt(current)
    const matched = entry("matched", { timestamp: daysAgo(3) })
    mocks.getSettings.mockResolvedValue({ ...DEFAULT_SETTINGS, sessionFilter: "passing" })
    const read = vi
      .fn()
      .mockResolvedValueOnce({ entries: first, exhausted: false })
      .mockResolvedValueOnce({ entries: [matched], exhausted: true })
    mocks.openSessionActivityStream.mockReturnValueOnce({ read, close: vi.fn() })
    let checksAvailable = false
    vi.spyOn(InsightsIpc, "getSessionHygiene").mockImplementation(async (rows) => {
      if (!checksAvailable && rows.some((row) => row.sessionId === "matched"))
        throw new Error("Check read failed")
      return rows.map(() => cleanHygiene)
    })
    const { session } = renderActivityWithHygiene()
    await ready(session)
    await screen.findByText("50 loaded sessions, 50 matching")
    act(() => {
      session.selectEntry(session.getSnapshot().entries!.at(-1)!)
      session.revealDetail()
    })
    fireEvent.keyDown(await screen.findByText("Detail: current"), { key: "ArrowRight" })
    expect(await screen.findByText(/Could not load checks for more sessions/)).toBeVisible()
    expect(session.getSnapshot().hasMoreSessions).toBe(false)
    fireEvent.click(screen.getByRole("button", { name: "Back to sessions" }))
    checksAvailable = true
    act(() => session.revealDetail())
    expect(await screen.findByText(/Press Right Arrow to resume/)).toBeVisible()
    fireEvent.keyDown(await screen.findByText("Detail: current"), { key: "ArrowRight" })
    expect(await screen.findByText("Detail: matched")).toBeVisible()
    expect(screen.queryByText("This item is outside the current list.")).toBeNull()
    fireEvent.click(screen.getByRole("button", { name: "Back to sessions" }))
    expect(await screen.findByText("51 total sessions, 51 matching")).toBeVisible()
    expect(read).toHaveBeenCalledTimes(2)
  })

  it.each([
    { exhausted: true, reopenBeforeAppend: false },
    { exhausted: false, reopenBeforeAppend: false },
    { exhausted: false, reopenBeforeAppend: true },
  ])(
    "resumes loaded checks after compact detail closes during an append (%j)",
    async ({ exhausted, reopenBeforeAppend }) => {
      const { matched, read, finishBatch } = mockPendingCompactAppend()
      read.mockRejectedValueOnce(new Error("Next batch read failed"))
      mocks.getSettings.mockResolvedValue({ ...DEFAULT_SETTINGS, sessionFilter: "passing" })
      let checksAvailable = false
      const failedCheck = vi.fn()
      vi.spyOn(InsightsIpc, "getSessionHygiene").mockImplementation(async (rows) => {
        if (!checksAvailable && rows.some((row) => row.sessionId === "matched")) {
          failedCheck()
          throw new Error("Check read failed")
        }
        return rows.map(() => cleanHygiene)
      })
      const { session } = renderActivityWithHygiene()
      await screen.findByText("50 loaded sessions, 50 matching")
      await startCompactAppend(session, read)
      if (reopenBeforeAppend) await reopenDuringAppend(session)
      await act(async () => finishBatch({ entries: [matched], exhausted }))
      await vi.waitFor(() => expect(failedCheck).toHaveBeenCalled())
      expect(session.getSnapshot().hasMoreSessions).toBe(!exhausted)
      expect(session.getSnapshot().subject?.sessionId).toBe("current")

      if (reopenBeforeAppend) {
        expect(await screen.findByText(/Could not load checks for more sessions/)).toBeVisible()
      } else {
        act(() => session.revealDetail())
        expect(await screen.findByText(/Press Right Arrow to resume/)).toBeVisible()
      }
      checksAvailable = true
      fireEvent.keyDown(await screen.findByText("Detail: current"), { key: "ArrowRight" })
      expect(await screen.findByText("Detail: matched")).toBeVisible()
      expect(screen.queryByText("This item is outside the current list.")).toBeNull()
      expect(read).toHaveBeenCalledTimes(2)
    },
  )

  it.each([
    { loadedMatch: false, reopen: false },
    { loadedMatch: true, reopen: false },
    { loadedMatch: false, reopen: true },
    { loadedMatch: true, reopen: true },
  ])(
    "checks loaded matches and handles append failure in compact detail (%j)",
    async ({ loadedMatch, reopen }) => {
      const { first, matched, read, rejectBatch } = mockPendingCompactAppend(loadedMatch)
      read.mockResolvedValueOnce({ entries: [matched], exhausted: true })
      mocks.getSettings.mockResolvedValue({ ...DEFAULT_SETTINGS, sessionFilter: "passing" })
      const initial = hygieneFor(
        first.map((row) => [
          row,
          row === matched ? { ...cleanHygiene, badges: [] } : cleanHygiene,
        ]),
      )
      let badges = initial
      const resolve = vi.fn().mockImplementation(async () => {
        badges = hygieneFor([...first, matched].map((row) => [row, cleanHygiene]))
        return true
      })
      const { session } = renderActivity(initial, {
        snapshot: initial,
        getSnapshot: () => badges,
        resolve,
      })
      await screen.findByText(`50 loaded sessions, ${loadedMatch ? 49 : 50} matching`)
      await startCompactAppend(session, read, reopen)
      if (reopen) await reopenDuringAppend(session)

      if (loadedMatch) badges = hygieneFor(first.map((row) => [row, cleanHygiene]))
      await act(async () => rejectBatch(new Error("Original append failed")))
      expect(session.getSnapshot().loadMoreSessionsError).toBe(true)
      expect(read).toHaveBeenCalledTimes(2)
      const loadError = "Could not load more sessions. Press Right Arrow to retry."
      if (loadedMatch) {
        expect(await screen.findByText("Detail: matched")).toBeVisible()
        expect(screen.queryByText(loadError)).toBeNull()
        expect(read).toHaveBeenCalledTimes(2)
      } else {
        expect(await screen.findByText(loadError)).toBeVisible()
        fireEvent.keyDown(screen.getByText("Detail: current"), { key: "ArrowRight" })
        expect(await screen.findByText("Detail: matched")).toBeVisible()
        expect(read).toHaveBeenCalledTimes(3)
      }
    },
  )

  it("still appends after a retry confirms an unresolved loaded successor does not match", async () => {
    const { first, matched, read, finishBatch } = mockPendingCompactAppend(true)
    const older = entry("older", { timestamp: daysAgo(4) })
    mocks.getSettings.mockResolvedValue({ ...DEFAULT_SETTINGS, sessionFilter: "passing" })
    const initial = hygieneFor(
      first.map((row) => [
        row,
        row === matched
          ? { ...cleanHygiene, evidenceState: "pending", badges: [] }
          : cleanHygiene,
      ]),
    )
    let badges = initial
    const resolve = vi
      .fn<SessionHygieneResource["resolve"]>()
      .mockResolvedValueOnce(true)
      .mockImplementationOnce(async () => {
        badges = hygieneFor(
          [...first, older].map((row) => [
            row,
            row === matched ? { ...cleanHygiene, badges: [] } : cleanHygiene,
          ]),
        )
        return true
      })
    const { session } = renderActivity(initial, {
      snapshot: initial,
      getSnapshot: () => badges,
      resolve,
    })
    await ready(session)
    act(() => {
      session.selectEntry(
        session.getSnapshot().entries!.find((row) => row.sessionId === "current")!,
      )
      session.revealDetail()
    })

    fireEvent.keyDown(await screen.findByText("Detail: current"), { key: "ArrowRight" })
    await screen.findByText("Check results are still pending. Press Right Arrow to retry.")
    expect(read).toHaveBeenCalledOnce()

    fireEvent.keyDown(screen.getByText("Detail: current"), { key: "ArrowRight" })
    await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(2))
    await act(async () => finishBatch({ entries: [older], exhausted: true }))
    expect(await screen.findByText("Detail: older")).toBeVisible()
  })

  it("retries a failed append on the first Right Arrow after compact detail reopens", async () => {
    const { first, matched, read, rejectBatch } = mockPendingCompactAppend()
    read.mockResolvedValueOnce({ entries: [matched], exhausted: true })
    mocks.getSettings.mockResolvedValue({ ...DEFAULT_SETTINGS, sessionFilter: "passing" })
    const { session } = renderActivity(
      hygieneFor([...first, matched].map((row) => [row, cleanHygiene])),
    )
    await ready(session)
    await startCompactAppend(session, read)
    await act(async () => rejectBatch(new Error("Original append failed")))
    expect(session.getSnapshot().loadMoreSessionsError).toBe(true)
    expect(read).toHaveBeenCalledTimes(2)

    act(() => session.revealDetail())
    fireEvent.keyDown(await screen.findByText("Detail: current"), { key: "ArrowRight" })
    expect(await screen.findByText("Detail: matched")).toBeVisible()
    expect(read).toHaveBeenCalledTimes(3)
  })

  it("keeps the selected session when compact detail closes during an append", async () => {
    const { first, matched, read, finishBatch } = mockPendingCompactAppend()
    const { session } = renderActivity()
    await ready(session)
    await startCompactAppend(session, read)
    await act(async () => finishBatch({ entries: [matched], exhausted: true }))
    expect(session.getSnapshot().entries?.map((row) => row.sessionId)).toEqual(
      [...first, matched].map((row) => row.sessionId),
    )
    expect(session.getSnapshot().subject?.sessionId).toBe("current")
    expect(screen.queryByText("Detail: matched")).toBeNull()
  })

  it("waits for the compact list to be visible before searching from zero matches", async () => {
    vi.stubGlobal("innerWidth", 550)
    const current = entry("current")
    mocks.getSettings.mockResolvedValue({ ...DEFAULT_SETTINGS, sessionFilter: "passing" })
    const read = vi
      .fn()
      .mockResolvedValueOnce({ entries: [current], exhausted: false })
      .mockReturnValue(new Promise(() => {}))
    mocks.openSessionActivityStream.mockReturnValueOnce({ read, close: vi.fn() })
    const { session, rerender } = renderActivity()
    await ready(session)
    act(() => {
      session.selectEntry(session.getSnapshot().entries![0]!)
      session.revealDetail()
    })
    expect(await screen.findByText("Detail: current")).toBeVisible()
    const hygiene = fixedHygiene(hygieneFor([[current, { ...cleanHygiene, badges: [] }]]))
    rerender(<MainActivityView active session={session} sessionHygiene={hygiene} />)
    await act(async () => {
      await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()))
    })
    expect(read).toHaveBeenCalledTimes(1)
    fireEvent.click(screen.getByRole("button", { name: "Back to sessions" }))
    await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(2))
  })

  it("uses badges from a matching batch that arrives during keyboard continuation", async () => {
    const current = entry("current")
    const first = batchEndingAt(current)
    const rejected = entry("rejected", { timestamp: daysAgo(3) })
    const matched = entry("matched", { timestamp: daysAgo(4) })
    const excludedBadge = { ...cleanHygiene, badges: [] }
    mocks.getSettings.mockResolvedValue({ ...DEFAULT_SETTINGS, sessionFilter: "passing" })
    const read = vi
      .fn()
      .mockResolvedValueOnce({ entries: first, exhausted: false })
      .mockResolvedValueOnce({ entries: [rejected], exhausted: false })
      .mockResolvedValueOnce({ entries: [matched], exhausted: true })
    mocks.openSessionActivityStream.mockReturnValueOnce({ read, close: vi.fn() })
    let finishChecks!: (value: SessionHygienePayload[]) => void
    const pendingChecks = new Promise<SessionHygienePayload[]>((resolve) => {
      finishChecks = resolve
    })
    const hygiene = vi
      .spyOn(InsightsIpc, "getSessionHygiene")
      .mockImplementation(async (rows) => {
        if (rows.length === 1 && rows[0]?.sessionId === "rejected") return pendingChecks
        return rows.map((row) => (row.sessionId === "rejected" ? excludedBadge : cleanHygiene))
      })
    const { session } = renderActivityWithHygiene()
    await screen.findByText("50 loaded sessions, 50 matching")
    act(() => session.selectEntry(session.getSnapshot().entries!.at(-1)!))
    fireEvent.keyDown(await screen.findByText("Detail: current"), { key: "ArrowRight" })
    await vi.waitFor(() =>
      expect(
        hygiene.mock.calls.filter(([rows]) => rows[0]?.sessionId === "rejected").length,
      ).toBeGreaterThanOrEqual(2),
    )
    await act(async () => {
      await session.loadMoreSessions()
    })
    await screen.findByText("52 total sessions, 51 matching")
    await act(async () => finishChecks([excludedBadge]))
    expect(await screen.findByText("Detail: matched")).toBeVisible()
    expect(screen.queryByText("No more matching sessions.")).toBeNull()
  })

  it("opens the shared time range from the header", async () => {
    const { session } = renderActivity()
    await ready(session)
    fireEvent.click(
      screen.getByRole("button", {
        name: "Last 7 days, change time range in Settings → Sessions",
      }),
    )
    expect(onOpenRangeSettings).toHaveBeenCalledOnce()
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
      <MainActivityView
        active
        session={session}
        sessionHygiene={fixedHygiene(hygieneForRows(entries))}
      />,
    )
    return {
      session,
      update(next: Partial<typeof snapshot>) {
        snapshot = { ...snapshot, ...next }
        view.rerender(
          <MainActivityView
            active
            session={session}
            sessionHygiene={fixedHygiene(hygieneForRows(snapshot.entries))}
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
    expect(screen.getByText("8 total sessions, 8 matching")).toBeVisible()
    openFilters()
    expect(
      screen.getByRole("menuitemradio", {
        name: /High cost, Over \$2(?: · auto)?, 1 matching session/,
      }),
    ).toBeEnabled()
    fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" })
    view.update({ filters: { agents: ["claude-code"], result: "passing", spend: "notable" } })
    expect(screen.getByText("8 total sessions, 1 matching")).toBeVisible()
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
    expect(screen.getByText("8 total sessions, 8 matching")).toBeVisible()
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
    expect(screen.getByText("7 total sessions, 7 matching")).toBeVisible()
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
    expect(screen.getByText("1 total sessions, 0 matching")).toBeVisible()
    expect(screen.getAllByText("No matching sessions")).toHaveLength(2)
    expect(screen.getByRole("button", { name: "Remove Claude Code filter" })).toBeEnabled()
    expect(screen.queryByText("rolling-only")).toBeNull()
    expect(screen.queryByRole("radiogroup", { name: "Session metric" })).toBeNull()
  })

  it("uses the true range-empty state when only excluded records are loaded", () => {
    scopedView([row("rolling-only", rollingOnly)], "claude-code")
    expect(screen.getByText("0 total sessions, 0 matching")).toBeVisible()
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
    expect(screen.getByText("1 total sessions, 1 matching")).toBeVisible()
    expect(screen.getByText("no-id")).toBeVisible()
    expect(screen.queryByText("invalid")).toBeNull()
    expect(screen.queryByText("future")).toBeNull()
  })

  it.each(["invalid", rollingOnly, new Date(2026, 8, 24, 12).toISOString()])(
    "keeps active rows and updates counts when they become inactive (%s)",
    (timestamp) => {
      const active = row("active-session", timestamp, { isActive: true })
      const view = scopedView([active])
      expect(screen.getByText("1 total sessions, 1 matching")).toBeVisible()
      expect(screen.getByText("active-session")).toBeVisible()
      view.update({ entries: [{ ...active, isActive: false }] })
      expect(screen.getByText("0 total sessions, 0 matching")).toBeVisible()
      expect(screen.queryByText("active-session")).toBeNull()
      expect(screen.queryByRole("radiogroup", { name: "Session metric" })).toBeNull()
    },
  )
})
