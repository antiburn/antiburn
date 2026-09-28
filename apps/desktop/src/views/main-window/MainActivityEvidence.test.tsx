import { act, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"

import type * as IpcModule from "../../lib/ipc"
import type * as SubjectModule from "../../lib/sessionSubject"
import { DEFAULT_SETTINGS } from "../../lib/ipc"
import type { SessionHygieneSnapshot } from "../../lib/useSessionHygiene"
import { MainActivitySession } from "./MainActivitySession"
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

it("inactive evidence destination opens Evidence on activation", async () => {
  const { session, rerender } = renderActivity()
  await ready(session)
  const subject = { agent: "codex", sessionId: "review-probe", title: "Probe" }
  act(() => session.restoreNavigation({ agents: [], result: "all", spend: "all" }, subject))
  await screen.findByText("No session analysis available", {}, { timeout: 15000 })
  rerender(<MainActivityView active={false} session={session} hygieneBySession={new Map()} />)
  expect(session.getSnapshot().active).toBe(false)
  const reference = {
    key: "review-passage",
    environmentKey: "native",
    agent: "codex",
    sessionId: "review-probe",
    sourceGeneration: 1,
    publishedFence: 1,
    sourceKey: "source",
    threadId: "main",
    scope: "main" as const,
    turnRowId: 1,
    turnIndex: 1,
    partIndex: 0,
  }
  act(() =>
    session.restoreNavigation(
      { agents: [], result: "all", spend: "all" },
      subject,
      "user",
      false,
      reference,
    ),
  )
  rerender(<MainActivityView active session={session} hygieneBySession={new Map()} />)
  await screen.findByRole("tab", { name: "Evidence" })
  expect(screen.getByRole("tab", { name: "Evidence" })).toHaveAttribute("aria-selected", "true")
})
