import { beforeEach, describe, expect, it, vi } from "vitest"
import { invoke, isTauri } from "@tauri-apps/api/core"
import { searchSessions } from "./sessionSearchIpc"
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), isTauri: vi.fn() }))
beforeEach(() => vi.resetAllMocks())
describe("session-search IPC", () => {
  it("uses the dedicated metadata endpoint with the opaque cursor", async () => {
    vi.mocked(isTauri).mockReturnValue(true)
    vi.mocked(invoke).mockResolvedValue({
      results: [],
      hasMore: false,
      nextCursor: null,
      indexing: true,
    })
    expect(await searchSessions("local query", "opaque")).toMatchObject({ indexing: true })
    expect(invoke).toHaveBeenCalledExactlyOnceWith("search_sessions", {
      query: "local query",
      cursor: "opaque",
    })
  })
  it("keeps the browser preview usable without a native bridge", async () => {
    vi.mocked(isTauri).mockReturnValue(false)
    expect(await searchSessions("query")).toEqual({
      results: [],
      hasMore: false,
      nextCursor: null,
      indexing: false,
    })
    expect(invoke).not.toHaveBeenCalled()
  })
})
