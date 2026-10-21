import { act, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import type { ClaudePluginStatusPayload } from "./claudePluginOffer"
import {
  createClaudePluginStore,
  offerForPlacement,
  resetClaudePluginReports,
  useClaudePlugin,
  type ClaudePluginPlacement,
  type ClaudePluginSnapshot,
  type ClaudePluginStore,
} from "./claudePluginStore"
import type { LiveUsageSummaryPayload } from "./providerUsageIpc"

const getClaudePluginStatus = vi.hoisted(() =>
  vi.fn<() => Promise<ClaudePluginStatusPayload | null>>(),
)
const getLiveUsage = vi.hoisted(() => vi.fn<() => Promise<LiveUsageSummaryPayload>>())
const noteInteraction = vi.hoisted(() => vi.fn())
vi.mock("./ipc", () => ({ getClaudePluginStatus, getLiveUsage, noteInteraction }))

const ABSENT: ClaudePluginStatusPayload = { state: "absent", version: null, enabled: true }
const INSTALLED: ClaudePluginStatusPayload = {
  state: "installed",
  version: "0.2.1",
  enabled: true,
}
const CLAUDE_HERE: LiveUsageSummaryPayload = {
  providers: [],
  errors: [],
  meters: [
    { provider: "anthropic", displayName: "Claude", shown: true, detection: "signedIn" },
  ],
  generatedAt: "",
}

function Viewer({
  store,
  placement,
}: {
  store: ClaudePluginStore
  placement: ClaudePluginPlacement
}) {
  const { offer } = useClaudePlugin(store, placement)
  return <output>{offer}</output>
}

beforeEach(() => {
  localStorage.clear()
  resetClaudePluginReports()
  getClaudePluginStatus.mockReset().mockResolvedValue(ABSENT)
  getLiveUsage.mockReset().mockResolvedValue(CLAUDE_HERE)
  noteInteraction.mockReset()
})

describe("createClaudePluginStore", () => {
  it("loads the status and meters and shows the offer, reporting it once per placement", async () => {
    const store = createClaudePluginStore({ reportInstalled: true })
    const { unmount } = render(<Viewer store={store} placement="sidebar" />)
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("offer"))
    expect(noteInteraction).toHaveBeenCalledTimes(1)
    expect(noteInteraction).toHaveBeenCalledWith({
      kind: "claudePluginOffer",
      action: "shown",
      placement: "sidebar",
    })

    unmount()
    render(<Viewer store={store} placement="sidebar" />)
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("offer"))
    expect(noteInteraction).toHaveBeenCalledTimes(1)

    render(<Viewer store={store} placement="agents" />)
    await waitFor(() => expect(noteInteraction).toHaveBeenCalledTimes(2))
    expect(noteInteraction).toHaveBeenLastCalledWith({
      kind: "claudePluginOffer",
      action: "shown",
      placement: "agents",
    })
  })

  it("reports installed_detected once per run, and only when asked to", async () => {
    getClaudePluginStatus.mockResolvedValue(INSTALLED)
    const quiet = createClaudePluginStore({ reportInstalled: false })
    render(<Viewer store={quiet} placement="usage_settings" />)
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("installed"))
    expect(noteInteraction).not.toHaveBeenCalled()

    const main = createClaudePluginStore({ reportInstalled: true })
    const { unmount } = render(<Viewer store={main} placement="sidebar" />)
    await waitFor(() =>
      expect(noteInteraction).toHaveBeenCalledWith({
        kind: "claudePluginOffer",
        action: "installed_detected",
        placement: "startup",
      }),
    )
    unmount()
    render(<Viewer store={main} placement="agents" />)
    await waitFor(() => expect(screen.getAllByRole("status")).toHaveLength(2))
    expect(noteInteraction).toHaveBeenCalledTimes(1)
  })

  it("dismisses for good: the sidebar hides, status placements stay, and the choice persists", async () => {
    const store = createClaudePluginStore({ reportInstalled: true })
    render(<Viewer store={store} placement="sidebar" />)
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("offer"))

    act(() => store.dismiss())
    expect(screen.getByRole("status")).toHaveTextContent("hidden")
    expect(noteInteraction).toHaveBeenCalledWith({
      kind: "claudePluginOffer",
      action: "dismissed",
      placement: "sidebar",
    })
    expect(offerForPlacement(store.getSnapshot(), "agents")).toBe("offer")
    expect(offerForPlacement(store.getSnapshot(), "usage_settings")).toBe("offer")

    const later = createClaudePluginStore({ reportInstalled: true })
    render(<Viewer store={later} placement="sidebar" />)
    await waitFor(() => expect(later.getSnapshot().status).toEqual(ABSENT))
    expect(offerForPlacement(later.getSnapshot(), "sidebar")).toBe("hidden")
  })

  it("re-reads the status when the window becomes visible or focused", async () => {
    const store = createClaudePluginStore({ reportInstalled: false })
    render(<Viewer store={store} placement="agents" />)
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("offer"))

    getClaudePluginStatus.mockResolvedValue(INSTALLED)
    act(() => {
      window.dispatchEvent(new Event("focus"))
    })
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("installed"))
    expect(getClaudePluginStatus).toHaveBeenCalledTimes(2)

    act(() => {
      document.dispatchEvent(new Event("visibilitychange"))
    })
    await waitFor(() => expect(getClaudePluginStatus).toHaveBeenCalledTimes(3))
  })

  it("takes the Claude Code session count from its source and follows its changes", async () => {
    getLiveUsage.mockResolvedValue({ ...CLAUDE_HERE, meters: [] })
    let sessions = 0
    const listeners = new Set<() => void>()
    const store = createClaudePluginStore({
      reportInstalled: false,
      sessions: {
        read: () => sessions,
        subscribe: (listener) => {
          listeners.add(listener)
          return () => listeners.delete(listener)
        },
      },
    })
    render(<Viewer store={store} placement="agents" />)
    await waitFor(() => expect(store.getSnapshot().status).toEqual(ABSENT))
    expect(screen.getByRole("status")).toHaveTextContent("hidden")

    sessions = 4
    act(() => listeners.forEach((listener) => listener()))
    expect(screen.getByRole("status")).toHaveTextContent("offer")
    const snapshot: ClaudePluginSnapshot = store.getSnapshot()
    expect(snapshot.claudeSessions).toBe(4)
  })

  it("stays hidden when the shell has no answer", async () => {
    getClaudePluginStatus.mockResolvedValue(null)
    const store = createClaudePluginStore({ reportInstalled: true })
    render(<Viewer store={store} placement="sidebar" />)
    await waitFor(() => expect(getClaudePluginStatus).toHaveBeenCalled())
    expect(screen.getByRole("status")).toHaveTextContent("hidden")
    expect(noteInteraction).not.toHaveBeenCalled()
  })
})
