import { act, cleanup, renderHook, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { DEFAULT_SETTINGS, SETTINGS_CHANGED_EVENT, type AppSettings } from "../../../lib/ipc"
import { skillsMcpExpandedStore, useSkillsMcpExpanded } from "./useSkillsMcpExpanded"

/**
 * A stand-in for the shell's settings store: it keeps what was written and
 * hands it back, so a test can close the chart and reopen it the way a reader
 * does across a launch.
 */
let saved: AppSettings
/** Event handlers the fake `listen` was given, one per live subscriber. */
const listeners = new Map<string, (event: { payload: unknown }) => void>()

const invoke = vi.hoisted(() => vi.fn())

vi.mock("@tauri-apps/api/core", () => ({ invoke, isTauri: () => true }))
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (name: string, handler: (event: { payload: unknown }) => void) => {
    listeners.set(name, handler)
    return () => listeners.delete(name)
  }),
}))

/** Push a settings broadcast at whatever subscribed to it. */
function broadcast(settings: AppSettings): void {
  act(() => listeners.get(SETTINGS_CHANGED_EVENT)?.({ payload: settings }))
}

beforeEach(() => {
  saved = { ...DEFAULT_SETTINGS }
  listeners.clear()
  invoke.mockImplementation((command: string, args?: { settings?: AppSettings }) => {
    if (command === "get_settings") return Promise.resolve({ ...saved })
    if (command === "set_settings") {
      saved = { ...(args?.settings as AppSettings) }
      return Promise.resolve({ ...saved })
    }
    return Promise.reject(new Error(`unexpected command ${command}`))
  })
})

afterEach(() => {
  cleanup()
  skillsMcpExpandedStore.set(DEFAULT_SETTINGS.skillsMcpExpanded)
  vi.clearAllMocks()
})

describe("useSkillsMcpExpanded", () => {
  it("starts collapsed for a reader who has never chosen", async () => {
    const { result } = renderHook(() => useSkillsMcpExpanded())
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("get_settings"))

    expect(result.current[0]).toBe(false)
  })

  it("opens the table for a reader whose stored answer is expanded", async () => {
    saved = { ...DEFAULT_SETTINGS, skillsMcpExpanded: true }

    const { result } = renderHook(() => useSkillsMcpExpanded())

    await waitFor(() => expect(result.current[0]).toBe(true))
  })

  it("writes the reader's choice through to the store", async () => {
    const { result } = renderHook(() => useSkillsMcpExpanded())
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("get_settings"))

    act(() => {
      result.current[1](true)
    })

    // Optimistic: the button does not wait for the write to land.
    expect(result.current[0]).toBe(true)
    await waitFor(() => expect(saved.skillsMcpExpanded).toBe(true))
  })

  it("puts the previous answer back when the write fails", async () => {
    const { result } = renderHook(() => useSkillsMcpExpanded())
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("get_settings"))
    invoke.mockImplementation(() => Promise.reject(new Error("store is unavailable")))

    act(() => {
      result.current[1](true)
    })

    expect(result.current[0]).toBe(true)
    await waitFor(() => expect(result.current[0]).toBe(false))
  })

  it("leaves every other preference alone when it writes", async () => {
    saved = { ...DEFAULT_SETTINGS, theme: "dark", activityWindowDays: 14 }
    const { result } = renderHook(() => useSkillsMcpExpanded())
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("get_settings"))

    act(() => {
      result.current[1](true)
    })
    await waitFor(() => expect(saved.skillsMcpExpanded).toBe(true))

    expect(saved.theme).toBe("dark")
    expect(saved.activityWindowDays).toBe(14)
  })

  it("comes back expanded after every reader of it has gone away", async () => {
    const first = renderHook(() => useSkillsMcpExpanded())
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("get_settings"))
    act(() => {
      first.result.current[1](true)
    })
    await waitFor(() => expect(saved.skillsMcpExpanded).toBe(true))
    first.unmount()

    // The store tore itself down with the last listener, so this remount takes
    // the same cold path a fresh app launch does.
    skillsMcpExpandedStore.set(DEFAULT_SETTINGS.skillsMcpExpanded)
    const second = renderHook(() => useSkillsMcpExpanded())

    await waitFor(() => expect(second.result.current[0]).toBe(true))
  })

  it("follows a change made in another window", async () => {
    const { result } = renderHook(() => useSkillsMcpExpanded())
    await waitFor(() => expect(listeners.has(SETTINGS_CHANGED_EVENT)).toBe(true))

    broadcast({ ...DEFAULT_SETTINGS, skillsMcpExpanded: true })

    expect(result.current[0]).toBe(true)
  })

  it("shares the flag between components mounted at the same time", async () => {
    const a = renderHook(() => useSkillsMcpExpanded())
    const b = renderHook(() => useSkillsMcpExpanded())
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("get_settings"))

    act(() => {
      a.result.current[1](true)
    })

    expect(b.result.current[0]).toBe(true)
  })
})
