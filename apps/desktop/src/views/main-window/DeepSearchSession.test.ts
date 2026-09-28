import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { DeepSearchResponse } from "../../lib/deepSessionSearchIpc"
import type { SessionEvidenceHit } from "../../lib/sessionEvidenceIpc"
import {
  DeepSearchSession,
  deepSessionKey,
  stableDeepResults,
  type DeepSearchDependencies,
} from "./DeepSearchSession"

function deepHit(id = "one", score = 1): SessionEvidenceHit {
  return {
    session: {
      environmentKey: "native",
      agent: "codex",
      sessionId: id,
      title: `Session ${id}`,
      repository: "orchard",
      cwdLabel: "orchard",
      models: [],
      wslDistro: null,
      timestamp: "2026-09-25T00:00:00Z",
    },
    reference: {
      key: id,
      environmentKey: "native",
      agent: "codex",
      sessionId: id,
      sourceGeneration: 1,
      publishedFence: 2,
      sourceKey: "source",
      threadId: "main",
      scope: "main",
      turnRowId: 1,
      turnIndex: 0,
      partIndex: 0,
    },
    excerpt: "The orchard phrase appears only in retained content.",
    kind: "assistant",
    score,
    coverage: { state: "complete", inspectedBytes: 100, byteLimit: 1_048_576 },
    truncated: false,
  }
}

function deepResponse(
  scanId: number,
  queryRevision: number,
  extra: Partial<DeepSearchResponse> = {},
): DeepSearchResponse {
  return {
    scanId,
    queryRevision,
    available: true,
    status: "searching",
    continuationAvailable: true,
    results: [deepHit()],
    invalidatedSessions: [],
    totalMatchingSessions: 1,
    coverage: {
      eligibleSessions: 20,
      inspectedSessions: 4,
      inspectedParts: 32,
      inspectedBytes: 65_536,
      unavailableSessions: 0,
      changedSessions: 0,
      ingestionTruncatedParts: 0,
      scopeExhausted: false,
    },
    ...extra,
  }
}

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => {
    resolve = done
  })
  return { promise, resolve }
}

function setup(overrides: Partial<DeepSearchDependencies> = {}) {
  const deps: DeepSearchDependencies = {
    start: vi.fn(async (_query, id, revision) => deepResponse(id, revision)),
    next: vi.fn(async (id, revision) =>
      deepResponse(id, revision, { status: "finished", continuationAvailable: false }),
    ),
    cancel: vi.fn().mockResolvedValue(undefined),
    ...overrides,
  }
  const session = new DeepSearchSession(deps)
  const unsubscribe = session.subscribe(() => {})
  session.setQuery("orchard phrase")
  return { deps, session, unsubscribe }
}

async function settle() {
  for (let i = 0; i < 10; i++) await Promise.resolve()
}
beforeEach(() => vi.useFakeTimers())
afterEach(() => vi.useRealTimers())

describe("explicit retained-content scan lifecycle", () => {
  it("waits for 600 ms of idle before starting", async () => {
    const { session, deps, unsubscribe } = setup()
    session.setQuery("orchard")
    session.setQuery("orchard phrase")
    await vi.advanceTimersByTimeAsync(599)
    expect(deps.start).not.toHaveBeenCalled()
    expect(deps.next).not.toHaveBeenCalled()
    expect(deps.cancel).not.toHaveBeenCalled()
    await vi.advanceTimersByTimeAsync(1)
    expect(deps.start).toHaveBeenCalledOnce()
    unsubscribe()
  })

  it("publishes a batch before requesting the next and stops after completion", async () => {
    const { session, deps, unsubscribe } = setup()
    session.start(false)
    session.start(false)
    await settle()
    expect(deps.start).toHaveBeenCalledOnce()
    expect(session.getSnapshot().results).toHaveLength(1)
    expect(deps.next).not.toHaveBeenCalled()
    await vi.advanceTimersByTimeAsync(1)
    expect(session.getSnapshot().phase).toBe("finished")
    await vi.advanceTimersByTimeAsync(100)
    expect(deps.next).toHaveBeenCalledOnce()
    unsubscribe()
  })

  it("Stop preserves results; Continue resumes the same identity after cancellation settles", async () => {
    const cancelled = deferred<void>()
    const { session, deps, unsubscribe } = setup({ cancel: vi.fn(() => cancelled.promise) })
    session.start()
    await settle()
    session.stop()
    session.continue()
    await vi.advanceTimersByTimeAsync(100)
    expect(deps.next).not.toHaveBeenCalled()
    expect(session.getSnapshot().phase).toBe("stopped")
    expect(session.getSnapshot().results).toHaveLength(1)
    expect(session.getSnapshot().settling).toBe(true)
    cancelled.resolve()
    await settle()
    session.continue()
    await settle()
    const [, id, revision] = vi.mocked(deps.start).mock.calls[0]!
    expect(deps.next).toHaveBeenCalledWith(id, revision)
    expect(session.getSnapshot().phase).toBe("finished")
    unsubscribe()
  })

  it("keeps an interrupted batch but never schedules more work after Stop", async () => {
    const pending = deferred<DeepSearchResponse>()
    const { session, deps, unsubscribe } = setup({ start: vi.fn(() => pending.promise) })
    session.start()
    session.stop()
    const [, id, revision] = vi.mocked(deps.start).mock.calls[0]!
    pending.resolve(deepResponse(id, revision))
    await settle()
    await vi.advanceTimersByTimeAsync(100)
    expect(session.getSnapshot().phase).toBe("stopped")
    expect(session.getSnapshot().results).toHaveLength(1)
    expect(deps.next).not.toHaveBeenCalled()
    unsubscribe()
  })

  it("discards stale results on query replacement and closure", async () => {
    const pending = deferred<DeepSearchResponse>()
    const { session, deps, unsubscribe } = setup({ start: vi.fn(() => pending.promise) })
    session.start()
    const [, id, revision] = vi.mocked(deps.start).mock.calls[0]!
    session.setQuery("different query")
    expect(deps.cancel).toHaveBeenCalledWith(id, revision, true)
    pending.resolve(deepResponse(id, revision))
    await settle()
    await vi.advanceTimersByTimeAsync(100)
    expect(session.getSnapshot().phase).toBe("idle")
    expect(session.getSnapshot().results).toHaveLength(0)
    expect(deps.next).not.toHaveBeenCalled()
    unsubscribe()
  })

  it("rejects a response for another revision", async () => {
    const { session, deps, unsubscribe } = setup({
      start: vi.fn(async (_query, id, revision) => deepResponse(id, revision + 1)),
    })
    session.start()
    await settle()
    expect(session.getSnapshot().phase).toBe("failed")
    expect(session.getSnapshot().results).toHaveLength(0)
    await vi.advanceTimersByTimeAsync(10)
    expect(deps.next).not.toHaveBeenCalled()
    unsubscribe()
  })

  it("does not continue resource-limited partial scans automatically", async () => {
    const { session, deps, unsubscribe } = setup({
      start: vi.fn(async (_query, id, revision) =>
        deepResponse(id, revision, { status: "partial", continuationAvailable: false }),
      ),
    })
    session.start()
    await settle()
    await vi.advanceTimersByTimeAsync(10)
    expect(session.getSnapshot().phase).toBe("partial")
    expect(deps.next).not.toHaveBeenCalled()
    unsubscribe()
  })

  it("cancels pending scans when the last subscriber leaves", async () => {
    const { session, deps, unsubscribe } = setup({
      start: vi.fn(async (_query, id, revision) => deepResponse(id, revision)),
    })
    session.start()
    await settle()
    unsubscribe()
    await vi.advanceTimersByTimeAsync(100)
    expect(deps.cancel).toHaveBeenCalledWith(expect.any(Number), expect.any(Number), true)
    expect(deps.next).not.toHaveBeenCalled()
    expect(session.getSnapshot().phase).toBe("idle")
  })
})

describe("progressive session ordering", () => {
  it("deduplicates, updates evidence and retains existing order", () => {
    const first = deepHit("one"),
      second = deepHit("two"),
      changed = deepHit("one", 5)
    expect(
      stableDeepResults(
        [first, second],
        deepResponse(1, 1, { results: [deepHit("three"), second, changed, changed] }),
        null,
      ),
    ).toEqual([changed, second, deepHit("three")])
  })

  it("pins an evicted selection within 100 rows but removes invalidated sessions", () => {
    const selected = deepHit("selected")
    const incoming = Array.from({ length: 100 }, (_, i) => deepHit(String(i)))
    const response = deepResponse(1, 1, { results: incoming })
    const results = stableDeepResults([selected], response, deepSessionKey(selected.session))
    expect(results).toHaveLength(100)
    expect(results[0]).toBe(selected)
    expect(
      stableDeepResults(
        [selected],
        { ...response, invalidatedSessions: [selected.session] },
        deepSessionKey(selected.session),
      ),
    ).not.toContain(selected)
  })
})

it("keeps the selected row index when earlier top results are evicted", () => {
  const selected = deepHit("selected")
  const response = deepResponse(1, 1, { results: [deepHit("new"), selected] })
  expect(
    stableDeepResults(
      [deepHit("evicted"), selected],
      response,
      deepSessionKey(selected.session),
    ).map((hit) => hit.session.sessionId),
  ).toEqual(["new", "selected"])
})

it("pins the automatic first selection when the top 100 changes", async () => {
  const { session, unsubscribe } = setup({
    next: vi.fn(async (id, revision) =>
      deepResponse(id, revision, {
        status: "finished",
        continuationAvailable: false,
        results: Array.from({ length: 100 }, (_, i) => deepHit(String(i))),
      }),
    ),
  })
  session.start()
  await settle()
  await vi.advanceTimersByTimeAsync(1)
  expect(session.getSnapshot().results).toHaveLength(100)
  expect(session.getSnapshot().results[0]?.session.sessionId).toBe("one")
  unsubscribe()
})

it("scope replacement cancels the old scan and rejects its late response", async () => {
  const pending = deferred<DeepSearchResponse>()
  const { session, deps, unsubscribe } = setup({ start: vi.fn(() => pending.promise) })
  const scope = { days: 7, fromEpoch: 100, throughEpoch: 200, timeZone: "Australia/Brisbane" }
  session.setScope(scope)
  session.start(false)
  const [, id, revision] = vi.mocked(deps.start).mock.calls[0]!
  expect(vi.mocked(deps.start).mock.calls[0]?.[3]).toEqual(scope)
  session.setScope(null)
  expect(deps.cancel).toHaveBeenCalledWith(id, revision, true)
  pending.resolve(deepResponse(id, revision))
  await settle()
  expect(session.getSnapshot().phase).toBe("idle")
  expect(session.getSnapshot().results).toEqual([])
  expect(deps.next).not.toHaveBeenCalled()
  unsubscribe()
})

describe("idle-triggered content search", () => {
  it("resets idle delay and cancels active work on replacement", async () => {
    const { session, deps, unsubscribe } = setup({
      start: vi.fn(() => new Promise<DeepSearchResponse>(() => {})),
    })
    await vi.advanceTimersByTimeAsync(500)
    session.setQuery("new query")
    await vi.advanceTimersByTimeAsync(599)
    expect(deps.start).not.toHaveBeenCalled()
    await vi.advanceTimersByTimeAsync(1)
    expect(deps.start).toHaveBeenCalledWith("new query", expect.any(Number), expect.any(Number))
    expect(session.getSnapshot().showProgress).toBe(false)
    await vi.advanceTimersByTimeAsync(499)
    expect(session.getSnapshot().showProgress).toBe(false)
    await vi.advanceTimersByTimeAsync(1)
    expect(session.getSnapshot().showProgress).toBe(true)
    session.setQuery("replacement")
    expect(deps.cancel).toHaveBeenCalledWith(expect.any(Number), expect.any(Number), true)
    unsubscribe()
    await vi.advanceTimersByTimeAsync(2000)
    expect(deps.start).toHaveBeenCalledOnce()
  })

  it("does not auto-start short queries or composing text; supports immediate activation", async () => {
    const { session, deps, unsubscribe } = setup()
    session.setQuery("ab")
    await vi.advanceTimersByTimeAsync(1000)
    expect(deps.start).not.toHaveBeenCalled()
    session.start()
    await settle()
    expect(deps.start).toHaveBeenCalledOnce()
    expect(session.getSnapshot().showProgress).toBe(true)
    session.setQuery("日本語", false)
    await vi.advanceTimersByTimeAsync(1000)
    expect(deps.start).toHaveBeenCalledOnce()
    session.setQuery("日本語")
    await vi.advanceTimersByTimeAsync(600)
    expect(deps.start).toHaveBeenCalledTimes(2)
    unsubscribe()
  })

  it("keeps fast automatic scans quiet and never restarts after Stop", async () => {
    const { session, deps, unsubscribe } = setup()
    await vi.advanceTimersByTimeAsync(1200)
    expect(session.getSnapshot().phase).toBe("finished")
    expect(session.getSnapshot().showProgress).toBe(false)
    session.setQuery("another query")
    await vi.advanceTimersByTimeAsync(600)
    session.stop()
    await vi.advanceTimersByTimeAsync(2000)
    expect(deps.start).toHaveBeenCalledTimes(2)
    unsubscribe()
  })
})

it("shows Stop when explicitly continuing after an early automatic failure", async () => {
  const next = vi
    .fn<DeepSearchDependencies["next"]>()
    .mockRejectedValueOnce(new Error("read failed"))
    .mockImplementation(() => new Promise<DeepSearchResponse>(() => {}))
  const { session, unsubscribe } = setup({ next })
  await vi.advanceTimersByTimeAsync(1200)
  expect(session.getSnapshot().phase).toBe("failed")
  expect(session.getSnapshot().showProgress).toBe(false)
  session.continue()
  expect(session.getSnapshot().phase).toBe("searching")
  expect(session.getSnapshot().showProgress).toBe(true)
  unsubscribe()
})
