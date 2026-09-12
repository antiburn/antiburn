import { render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type * as IpcModule from "../../lib/ipc"
import type * as SubjectModule from "../../lib/sessionSubject"
import type { SessionHygienePayload } from "../../lib/insightsIpc"
import { type ActivityEntryPayload, DEFAULT_SETTINGS } from "../../lib/ipc"
import { localSessionKey } from "../../lib/presentation/localIdentity"
import type { SessionHygieneSnapshot } from "../../lib/useSessionHygiene"
import { MainActivitySession } from "./MainActivitySession"
import { MainActivityView } from "./MainActivityView"

const mocks = vi.hoisted(() => ({
  getSettings: vi.fn(),
  setSettings: vi.fn(),
  listRecentSessions: vi.fn(),
  getMainWindowVisible: vi.fn(),
  peekMainWindowSessionTarget: vi.fn(),
  acknowledgeMainWindowSessionTarget: vi.fn(),
  getLiveUsage: vi.fn(),
  getSessionLimitAllocations: vi.fn(),
  loadSessionAnalysis: vi.fn(),
  noteInteraction: vi.fn(),
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
    onMainWindowSessionTarget: noListener,
    onSettingsChanged: noListener,
    onSessionsInvalidated: noListener,
    onScanEvent: noListener,
    onSessionEntryChanged: noListener,
    onLiveUsageChanged: noListener,
  }
})
vi.mock("../../lib/sessionSubject", async (importOriginal) => ({
  ...(await importOriginal<typeof SubjectModule>()),
  loadSessionAnalysis: mocks.loadSessionAnalysis,
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
}

let sessions: MainActivitySession[]

beforeEach(() => {
  vi.clearAllMocks()
  sessions = []
  mocks.getSettings.mockResolvedValue(DEFAULT_SETTINGS)
  mocks.setSettings.mockImplementation(async (settings) => settings)
  mocks.getMainWindowVisible.mockResolvedValue(true)
  mocks.peekMainWindowSessionTarget.mockResolvedValue(null)
  mocks.acknowledgeMainWindowSessionTarget.mockResolvedValue(undefined)
  mocks.listRecentSessions.mockResolvedValue([])
  mocks.loadSessionAnalysis.mockResolvedValue(null)
  mocks.getLiveUsage.mockResolvedValue(null)
  mocks.getSessionLimitAllocations.mockResolvedValue(null)
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
    mocks.getSettings.mockResolvedValue({ ...DEFAULT_SETTINGS, sessionFilter: "failing" })
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
    await vi.waitFor(() =>
      expect(screen.getAllByText("No sessions match this filter.")).toHaveLength(2),
    )
    expect(screen.queryByText("one")).toBeNull()
    expect(screen.queryByText(/No sessions in the last/)).toBeNull()
  })

  it("keeps the day-window empty copy when the list itself is empty", async () => {
    mocks.listRecentSessions.mockResolvedValue([])
    const { session } = renderActivity()
    await ready(session)

    await vi.waitFor(() =>
      expect(screen.getAllByText(/No sessions in the last \d+ days/)).toHaveLength(2),
    )
  })
})
