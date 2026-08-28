import { act, cleanup, renderHook, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { SessionHygienePayload } from "./insightsIpc"
import type * as Ipc from "./ipc"
import type { LocalSessionIdentity } from "./types/session"
import { sessionHygieneFor, useSessionHygiene } from "./useSessionHygiene"

const ipcMocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  onScanEvent: vi.fn(),
  onSessionsInvalidated: vi.fn(),
  onSessionEntryChanged: vi.fn(),
}))

vi.mock("@tauri-apps/api/core", () => ({
  invoke: ipcMocks.invoke,
  isTauri: () => true,
}))

vi.mock("./ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof Ipc>()),
  onScanEvent: ipcMocks.onScanEvent,
  onSessionsInvalidated: ipcMocks.onSessionsInvalidated,
  onSessionEntryChanged: ipcMocks.onSessionEntryChanged,
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

function payload(status: "finding" | "clean" | "notAssessed"): SessionHygienePayload {
  return {
    evidenceState: status === "notAssessed" ? "processing" : "ready",
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
  ipcMocks.onScanEvent.mockReset()
  ipcMocks.onSessionsInvalidated.mockReset()
  ipcMocks.onSessionEntryChanged.mockReset()
  ipcMocks.invoke.mockResolvedValue(null)
  ipcMocks.onScanEvent.mockResolvedValue(vi.fn())
  ipcMocks.onSessionsInvalidated.mockResolvedValue(vi.fn())
  ipcMocks.onSessionEntryChanged.mockResolvedValue(vi.fn())
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
    expect(ipcMocks.onScanEvent).toHaveBeenCalledTimes(1)
    expect(ipcMocks.onSessionsInvalidated).toHaveBeenCalledTimes(1)
    expect(ipcMocks.onSessionEntryChanged).toHaveBeenCalledTimes(1)
  })

  it("refreshes only the session named by an entry change", async () => {
    ipcMocks.invoke
      .mockResolvedValueOnce([payload("clean"), payload("clean")])
      .mockResolvedValueOnce([payload("finding")])
    const { result } = renderHook(() => useSessionHygiene([FIRST, SECOND]))
    await waitFor(() => expect(ipcMocks.onSessionEntryChanged).toHaveBeenCalledTimes(1))

    const onEntryChange = ipcMocks.onSessionEntryChanged.mock.calls[0]?.[0]
    await act(async () => {
      onEntryChange({
        agent: FIRST.agent,
        sessionId: FIRST.sessionId,
        wslDistro: FIRST.wslDistro,
      })
    })

    await waitFor(() => expect(ipcMocks.invoke).toHaveBeenCalledTimes(2))
    expect(ipcMocks.invoke).toHaveBeenLastCalledWith("get_session_hygiene", {
      sessions: [FIRST],
    })
    expect(sessionHygieneFor(result.current, FIRST).badges[0]?.status).toBe("finding")
    expect(sessionHygieneFor(result.current, SECOND).badges[0]?.status).toBe("clean")
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
    await waitFor(() => expect(ipcMocks.onScanEvent).toHaveBeenCalledTimes(1))

    const onScan = ipcMocks.onScanEvent.mock.calls[0]?.[0]
    const onInvalidated = ipcMocks.onSessionsInvalidated.mock.calls[0]?.[0]
    act(() => {
      onScan({}, "finished")
      onInvalidated()
    })
    expect(ipcMocks.invoke).toHaveBeenCalledTimes(2)

    await act(async () => {
      resolveRefresh([payload("clean")])
      await pendingRefresh
    })
    await waitFor(() => expect(ipcMocks.invoke).toHaveBeenCalledTimes(3))
  })

  it("replaces subscriptions when the requested identity changes", async () => {
    const stopFirstScan = vi.fn()
    const stopSecondScan = vi.fn()
    ipcMocks.invoke
      .mockResolvedValueOnce([payload("clean")])
      .mockResolvedValueOnce([payload("finding")])
    ipcMocks.onScanEvent
      .mockResolvedValueOnce(stopFirstScan)
      .mockResolvedValueOnce(stopSecondScan)
    const { result, rerender, unmount } = renderHook(
      ({ sessions }: { sessions: LocalSessionIdentity[] }) => useSessionHygiene(sessions),
      { initialProps: { sessions: [FIRST] } },
    )
    await waitFor(() => expect(ipcMocks.onScanEvent).toHaveBeenCalledTimes(1))

    rerender({ sessions: [SECOND] })
    await waitFor(() => expect(ipcMocks.invoke).toHaveBeenCalledTimes(2))
    expect(stopFirstScan).toHaveBeenCalledTimes(1)
    expect(sessionHygieneFor(result.current, SECOND).badges[0]?.status).toBe("finding")

    unmount()
    expect(stopSecondScan).toHaveBeenCalledTimes(1)
  })

  it("tears down every listener", async () => {
    const stopScan = vi.fn()
    const stopInvalidation = vi.fn()
    const stopEntryChange = vi.fn()
    ipcMocks.onScanEvent.mockResolvedValueOnce(stopScan)
    ipcMocks.onSessionsInvalidated.mockResolvedValueOnce(stopInvalidation)
    ipcMocks.onSessionEntryChanged.mockResolvedValueOnce(stopEntryChange)
    const { unmount } = renderHook(() => useSessionHygiene([FIRST]))
    await waitFor(() => expect(ipcMocks.onSessionEntryChanged).toHaveBeenCalledTimes(1))

    unmount()

    expect(stopScan).toHaveBeenCalledTimes(1)
    expect(stopInvalidation).toHaveBeenCalledTimes(1)
    expect(stopEntryChange).toHaveBeenCalledTimes(1)
  })
})
