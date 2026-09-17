import { act, cleanup, renderHook, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { SessionHygienePayload } from "./insightsIpc"
import type * as Ipc from "./ipc"
import type { LocalSessionIdentity } from "./types/session"
import { sessionHygieneFor, useSessionHygiene } from "./useSessionHygiene"

const ipcMocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  onSessionIndexChanged: vi.fn(),
  onSessionUpdated: vi.fn(),
}))

vi.mock("@tauri-apps/api/core", () => ({
  invoke: ipcMocks.invoke,
  isTauri: () => true,
}))

vi.mock("./ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof Ipc>()),
  onSessionIndexChanged: ipcMocks.onSessionIndexChanged,
  onSessionUpdated: ipcMocks.onSessionUpdated,
}))

const FIRST: LocalSessionIdentity = {
  agent: "claude-code",
  sessionId: "synthetic-first",
  wslDistro: null,
}
const SECOND: LocalSessionIdentity = {
  agent: "claude-code",
  sessionId: "synthetic-second",
  wslDistro: "Synthetic-Linux",
}

function facets(overrides: Partial<Ipc.UpdateFacetsPayload> = {}): Ipc.UpdateFacetsPayload {
  return {
    metadata: false,
    title: false,
    analysis: false,
    usage: false,
    checks: false,
    limits: false,
    ...overrides,
  }
}

function updateFor(
  identity: LocalSessionIdentity,
  facetOverrides: Partial<Ipc.UpdateFacetsPayload>,
): {
  seq: number
  session: Ipc.SessionRefPayload
  facets: Ipc.UpdateFacetsPayload
  entry: { agent: string; sessionId: string; wslDistro: string | null }
} {
  return {
    seq: 1,
    session: {
      environmentKey: identity.wslDistro ? `wsl:${identity.wslDistro}` : "native",
      agent: identity.agent,
      sessionId: identity.sessionId,
    },
    facets: facets(facetOverrides),
    entry: {
      agent: identity.agent,
      sessionId: identity.sessionId,
      wslDistro: identity.wslDistro ?? null,
    },
  }
}

function payload(status: "finding" | "clean" | "notAssessed"): SessionHygienePayload {
  return {
    evidenceState: status === "notAssessed" ? "processing" : "ready",
    unusedResources: null,
    badges: [
      {
        id: "sessionOverdepth",
        status,
        notAssessedReason: status === "notAssessed" ? "incompleteEvidence" : null,
      },
      {
        id: "modelOverthinking",
        status: "clean",
        notAssessedReason: null,
      },
      {
        id: "overpoweredSubagents",
        status: "clean",
        notAssessedReason: null,
      },
      {
        id: "obsoleteModel",
        status: "clean",
        notAssessedReason: null,
      },
      {
        id: "fastModeOveruse",
        status: "clean",
        notAssessedReason: null,
      },
      {
        id: "excessCacheRehydration",
        status: "clean",
        notAssessedReason: null,
      },
    ],
  }
}

beforeEach(() => {
  ipcMocks.invoke.mockReset()
  ipcMocks.onSessionIndexChanged.mockReset()
  ipcMocks.onSessionUpdated.mockReset()
  ipcMocks.invoke.mockResolvedValue(null)
  ipcMocks.onSessionIndexChanged.mockResolvedValue(vi.fn())
  ipcMocks.onSessionUpdated.mockResolvedValue(vi.fn())
})

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})

describe("useSessionHygiene", () => {
  it("loads multiple sessions through one IPC request", async () => {
    ipcMocks.invoke.mockResolvedValueOnce([payload("finding"), payload("clean")])

    const { result } = renderHook(() => useSessionHygiene([FIRST, SECOND]))

    await waitFor(() => {
      expect(sessionHygieneFor(result.current, FIRST).badges[0]?.status).toBe("finding")
    })
    expect(sessionHygieneFor(result.current, SECOND).badges[0]?.status).toBe("clean")
    expect(ipcMocks.invoke).toHaveBeenCalledTimes(1)
    expect(ipcMocks.invoke).toHaveBeenCalledWith("get_session_hygiene", {
      sessions: [FIRST, SECOND],
    })
    expect(ipcMocks.onSessionIndexChanged).toHaveBeenCalledTimes(1)
    expect(ipcMocks.onSessionUpdated).toHaveBeenCalledTimes(1)
  })

  it("refreshes only the session named by an analysis-facet update", async () => {
    ipcMocks.invoke
      .mockResolvedValueOnce([payload("clean"), payload("clean")])
      .mockResolvedValueOnce([payload("finding")])
    const { result } = renderHook(() => useSessionHygiene([FIRST, SECOND]))
    await waitFor(() => expect(ipcMocks.onSessionUpdated).toHaveBeenCalledTimes(1))

    const onUpdate = ipcMocks.onSessionUpdated.mock.calls[0]?.[0]
    await act(async () => {
      onUpdate(updateFor(FIRST, { analysis: true }))
    })

    await waitFor(() => expect(ipcMocks.invoke).toHaveBeenCalledTimes(2))
    expect(ipcMocks.invoke).toHaveBeenLastCalledWith("get_session_hygiene", {
      sessions: [FIRST],
    })
    expect(sessionHygieneFor(result.current, FIRST).badges[0]?.status).toBe("finding")
    expect(sessionHygieneFor(result.current, SECOND).badges[0]?.status).toBe("clean")
  })

  it("ignores an update whose facets cannot move hygiene", async () => {
    ipcMocks.invoke.mockResolvedValueOnce([payload("clean")])
    renderHook(() => useSessionHygiene([FIRST]))
    await waitFor(() => expect(ipcMocks.onSessionUpdated).toHaveBeenCalledTimes(1))
    await waitFor(() => expect(ipcMocks.invoke).toHaveBeenCalledTimes(1))

    const onUpdate = ipcMocks.onSessionUpdated.mock.calls[0]?.[0]
    await act(async () => {
      onUpdate(updateFor(FIRST, { title: true, metadata: true, usage: true }))
    })

    expect(ipcMocks.invoke).toHaveBeenCalledTimes(1)
  })

  it("queues one follow-up refresh while a batch is in flight", async () => {
    let resolveRefresh!: (value: SessionHygienePayload[]) => void
    const pendingRefresh = new Promise<SessionHygienePayload[]>((resolve) => {
      resolveRefresh = resolve
    })
    ipcMocks.invoke
      .mockResolvedValueOnce([payload("clean")])
      .mockReturnValueOnce(pendingRefresh)
      .mockResolvedValueOnce([payload("finding")])
    renderHook(() => useSessionHygiene([FIRST]))
    await waitFor(() => expect(ipcMocks.onSessionIndexChanged).toHaveBeenCalledTimes(1))

    const onIndexChanged = ipcMocks.onSessionIndexChanged.mock.calls[0]?.[0]
    act(() => {
      onIndexChanged({ seq: 2, cause: "scan_pass" })
      onIndexChanged({ seq: 3, cause: "invalidated" })
    })
    expect(ipcMocks.invoke).toHaveBeenCalledTimes(2)

    await act(async () => {
      resolveRefresh([payload("clean")])
      await pendingRefresh
    })
    await waitFor(() => expect(ipcMocks.invoke).toHaveBeenCalledTimes(3))
  })

  it("replaces subscriptions when the requested identity changes", async () => {
    const stopFirst = vi.fn()
    const stopSecond = vi.fn()
    ipcMocks.invoke
      .mockResolvedValueOnce([payload("clean")])
      .mockResolvedValueOnce([payload("finding")])
    ipcMocks.onSessionIndexChanged
      .mockResolvedValueOnce(stopFirst)
      .mockResolvedValueOnce(stopSecond)
    const { result, rerender, unmount } = renderHook(
      ({ sessions }: { sessions: LocalSessionIdentity[] }) => useSessionHygiene(sessions),
      { initialProps: { sessions: [FIRST] } },
    )
    await waitFor(() => expect(ipcMocks.onSessionIndexChanged).toHaveBeenCalledTimes(1))

    rerender({ sessions: [SECOND] })
    await waitFor(() => expect(ipcMocks.invoke).toHaveBeenCalledTimes(2))
    expect(stopFirst).toHaveBeenCalledTimes(1)
    expect(sessionHygieneFor(result.current, SECOND).badges[0]?.status).toBe("finding")

    unmount()
    expect(stopSecond).toHaveBeenCalledTimes(1)
  })

  it("tears down every listener", async () => {
    const stopIndexChange = vi.fn()
    const stopUpdate = vi.fn()
    ipcMocks.onSessionIndexChanged.mockResolvedValueOnce(stopIndexChange)
    ipcMocks.onSessionUpdated.mockResolvedValueOnce(stopUpdate)
    const { unmount } = renderHook(() => useSessionHygiene([FIRST]))
    await waitFor(() => expect(ipcMocks.onSessionUpdated).toHaveBeenCalledTimes(1))

    unmount()

    expect(stopIndexChange).toHaveBeenCalledTimes(1)
    expect(stopUpdate).toHaveBeenCalledTimes(1)
  })
})
