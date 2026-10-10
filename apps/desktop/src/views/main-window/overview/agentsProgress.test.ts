import { afterEach, expect, it, vi } from "vitest"
import { subscribeAgentsProgress } from "./agentsProgress"

const state = vi.hoisted(() => ({
  visible: false,
  visibility: (_visible: boolean) => {},
  update: () => {},
  progress: { agents: { done: false, rows: [] as { sessions: number }[] } },
  noteInteraction: vi.fn(),
  stop: vi.fn(),
}))
vi.mock("../../../lib/mainWindowIpc", () => ({
  getMainWindowVisible: async () => state.visible,
  onMainWindowVisibilityChanged: async (listener: typeof state.visibility) => {
    state.visibility = listener
    return state.stop
  },
}))
vi.mock("../../../lib/ipc", () => ({ noteInteraction: state.noteInteraction }))
vi.mock("./overviewProgressStore", () => ({
  overviewProgress: () => state.progress,
  subscribeOverviewProgress: (listener: () => void) => {
    state.update = listener
    return state.stop
  },
}))
afterEach(() => vi.clearAllMocks())

it("measures only visible Agents visits and cancels work on departure", async () => {
  const stop = subscribeAgentsProgress(vi.fn())
  await Promise.resolve()
  expect(state.noteInteraction).not.toHaveBeenCalled()
  state.visibility(true)
  expect(state.noteInteraction).toHaveBeenCalledWith({
    kind: "surfaceViewed",
    surface: "agents",
    origin: "user",
  })
  state.progress = { agents: { done: true, rows: [{ sessions: 2 }] } }
  state.update()
  expect(state.noteInteraction).toHaveBeenCalledWith({
    kind: "surfaceStateObserved",
    surface: "agents",
    origin: "user",
    state: "ready",
  })
  state.noteInteraction.mockClear()
  state.visibility(false)
  state.update()
  expect(state.noteInteraction).not.toHaveBeenCalled()
  stop()
  state.visibility(true)
  expect(state.noteInteraction).not.toHaveBeenCalled()
  expect(state.stop).toHaveBeenCalledTimes(2)
})
