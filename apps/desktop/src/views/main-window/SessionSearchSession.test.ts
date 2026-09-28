import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { SessionSearchEntry, SessionSearchResponse } from "../../lib/sessionSearchIpc"
import { SessionSearchSession, sessionSearchIdentity } from "./SessionSearchSession"
const entry = (id: string, environmentKey = "native"): SessionSearchEntry => ({
  environmentKey,
  agent: "codex",
  sessionId: id,
  wslDistro: environmentKey === "native" ? null : "Ubuntu",
  title: id,
  repository: "repo",
  cwdLabel: "repo",
  models: ["model"],
  timestamp: "2026-09-17T00:00:00Z",
})
const page = (
  results: SessionSearchEntry[],
  extra: Partial<SessionSearchResponse> = {},
): SessionSearchResponse => ({
  results,
  nextCursor: null,
  hasMore: false,
  indexing: false,
  ...extra,
})
function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => {
    resolve = done
  })
  return { promise, resolve }
}
beforeEach(() => vi.useFakeTimers())
afterEach(() => vi.useRealTimers())
describe("session metadata search", () => {
  it("debounces for 100ms, bounds queries and makes no request for empty input", async () => {
    const search = vi.fn().mockResolvedValue(page([]))
    const session = new SessionSearchSession(search)
    const stop = session.subscribe(() => {})
    session.setQuery("old")
    await vi.advanceTimersByTimeAsync(50)
    session.setQuery("new")
    await vi.advanceTimersByTimeAsync(99)
    expect(search).not.toHaveBeenCalled()
    await vi.advanceTimersByTimeAsync(1)
    expect(search).toHaveBeenCalledWith("new", null)
    session.setQuery("x".repeat(300))
    expect(session.getSnapshot().query).toHaveLength(200)
    session.setQuery(" ")
    await vi.runAllTimersAsync()
    expect(search).toHaveBeenCalledTimes(1)
    stop()
  })
  it("discards stale responses and work completing after unsubscribe", async () => {
    const first = deferred<SessionSearchResponse>(),
      second = deferred<SessionSearchResponse>()
    const search = vi
      .fn()
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise)
    const session = new SessionSearchSession(search),
      stop = session.subscribe(() => {})
    session.setQuery("first")
    await vi.advanceTimersByTimeAsync(100)
    session.setQuery("second")
    await vi.advanceTimersByTimeAsync(100)
    first.resolve(page([entry("stale")]))
    await Promise.resolve()
    expect(session.getSnapshot().results).toEqual([])
    stop()
    second.resolve(page([entry("late")]))
    await Promise.resolve()
    expect(session.getSnapshot().results).toEqual([])
  })
  it("expands the cached first page then loads pages and preserves native/WSL identities", async () => {
    const first = Array.from({ length: 20 }, (_, i) => entry(String(i)))
    const search = vi
      .fn()
      .mockResolvedValueOnce(page(first, { nextCursor: "opaque", hasMore: true }))
      .mockResolvedValueOnce(page([entry("0"), entry("0", "wsl:ubuntu")]))
    const session = new SessionSearchSession(search),
      stop = session.subscribe(() => {})
    session.setQuery("repo")
    await vi.advanceTimersByTimeAsync(100)
    expect(session.getSnapshot().expanded).toBe(false)
    session.more()
    expect(session.getSnapshot().expanded).toBe(true)
    expect(search).toHaveBeenCalledTimes(1)
    session.more()
    await Promise.resolve()
    expect(search).toHaveBeenLastCalledWith("repo", "opaque")
    expect(session.getSnapshot().results).toHaveLength(21)
    expect(new Set(session.getSnapshot().results.map(sessionSearchIdentity)).size).toBe(21)
    stop()
  })
  it("polls bounded indexing progress and cancels the poll when the query changes", async () => {
    const search = vi
      .fn()
      .mockResolvedValueOnce(page([entry("partial")], { indexing: true }))
      .mockResolvedValueOnce(page([entry("ready")]))
    const session = new SessionSearchSession(search),
      stop = session.subscribe(() => {})
    session.setQuery("repo")
    await vi.advanceTimersByTimeAsync(100)
    expect(session.getSnapshot().indexing).toBe(true)
    session.more()
    expect(session.getSnapshot().expanded).toBe(false)
    await vi.advanceTimersByTimeAsync(500)
    expect(session.getSnapshot().indexing).toBe(false)
    expect(session.getSnapshot().results[0]?.sessionId).toBe("ready")
    session.setQuery("different")
    session.setQuery("")
    await vi.runAllTimersAsync()
    expect(search).toHaveBeenCalledTimes(2)
    stop()
  })
  it("refreshes from the first page after a failed or expired pagination cursor", async () => {
    const search = vi
      .fn()
      .mockResolvedValueOnce(page([entry("one")], { hasMore: true, nextCursor: "expired" }))
      .mockRejectedValueOnce(new Error("expired"))
      .mockResolvedValueOnce(page([entry("fresh")]))
    const session = new SessionSearchSession(search),
      stop = session.subscribe(() => {})
    session.setQuery("repo")
    await vi.advanceTimersByTimeAsync(100)
    session.more()
    session.more()
    await Promise.resolve()
    await Promise.resolve()
    expect(session.getSnapshot().error).toBe(true)
    session.retry()
    await vi.advanceTimersByTimeAsync(0)
    expect(search).toHaveBeenLastCalledWith("repo", null)
    expect(session.getSnapshot().results[0]?.sessionId).toBe("fresh")
    stop()
  })
})

it("resets pagination and ignores metadata responses from a previous scope", async () => {
  const old = deferred<SessionSearchResponse>()
  const search = vi
    .fn()
    .mockReturnValueOnce(old.promise)
    .mockResolvedValue(page([entry("recent")]))
  const session = new SessionSearchSession(search)
  const stop = session.subscribe(() => {})
  session.setQuery("project")
  await vi.advanceTimersByTimeAsync(100)
  const scope = { days: 7, fromEpoch: 100, throughEpoch: 200, timeZone: "Australia/Brisbane" }
  session.setScope(scope)
  await vi.advanceTimersByTimeAsync(1)
  old.resolve(page([entry("old")], { hasMore: true, nextCursor: "obsolete" }))
  await Promise.resolve()
  expect(search).toHaveBeenLastCalledWith("project", null, scope)
  expect(session.getSnapshot().results.map((row) => row.sessionId)).toEqual(["recent"])
  expect(session.getSnapshot().nextCursor).toBeNull()
  stop()
})
