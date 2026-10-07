import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { AgentMemoriesReport } from "../../../lib/memoriesIpc"
import { entry, project, report } from "./memoriesFixtures"
import { MemoriesSession, sortMemories, type MemoriesAdapter } from "./MemoriesSession"
import type * as IpcModule from "../../../lib/ipc"

const noteInteraction = vi.hoisted(() => vi.fn())
vi.mock("../../../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof IpcModule>()),
  noteInteraction,
}))

function setup(overrides: Partial<MemoriesAdapter> = {}) {
  const adapter: MemoriesAdapter = {
    listMemories: vi.fn().mockResolvedValue(report()),
    reveal: vi.fn().mockResolvedValue(undefined),
    now: vi.fn(() => 1_000_000),
    ...overrides,
  }
  const session = new MemoriesSession(adapter)
  return { adapter, session }
}

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((r) => (resolve = r))
  return { promise, resolve }
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0))

beforeEach(() => {
  noteInteraction.mockClear()
  localStorage.clear()
})
afterEach(() => localStorage.clear())

describe("MemoriesSession", () => {
  it("loads on the first active subscribe", async () => {
    const { adapter, session } = setup()
    const stop = session.subscribe(() => undefined)
    await flush()
    expect(adapter.listMemories).toHaveBeenCalledTimes(1)
    expect(session.getSnapshot().report).toEqual(report())
    stop()
    session.dispose()
  })

  it("does not load while only inactive listeners exist", async () => {
    const { adapter, session } = setup()
    const stop = session.subscribeInactive(() => undefined)
    await flush()
    expect(adapter.listMemories).not.toHaveBeenCalled()
    stop()
  })

  it("reloads on re-activation and keeps the stale report meanwhile", async () => {
    const pending = deferred<AgentMemoriesReport>()
    const { adapter, session } = setup()
    let stop = session.subscribe(() => undefined)
    await flush()
    stop()
    vi.mocked(adapter.listMemories).mockReturnValueOnce(pending.promise)
    stop = session.subscribe(() => undefined)
    expect(adapter.listMemories).toHaveBeenCalledTimes(2)
    expect(session.getSnapshot().loading).toBe(true)
    expect(session.getSnapshot().report).toEqual(report())
    const next = report([project({ slug: "-q" })])
    pending.resolve(next)
    await flush()
    expect(session.getSnapshot().loading).toBe(false)
    expect(session.getSnapshot().report).toEqual(next)
    stop()
  })

  it("discards a result from an older generation", async () => {
    const first = deferred<AgentMemoriesReport>()
    const second = deferred<AgentMemoriesReport>()
    const { adapter, session } = setup()
    vi.mocked(adapter.listMemories)
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise)
    const stop = session.subscribe(() => undefined)
    session.refresh()
    second.resolve(report([project({ slug: "-new" })]))
    await flush()
    first.resolve(report([project({ slug: "-old" })]))
    await flush()
    expect(session.getSnapshot().report?.projects[0]?.slug).toBe("-new")
    stop()
  })

  it("refreshes after an error", async () => {
    const { adapter, session } = setup()
    vi.mocked(adapter.listMemories).mockRejectedValueOnce(new Error("no"))
    const stop = session.subscribe(() => undefined)
    await flush()
    expect(session.getSnapshot().error).toBe(true)
    session.refresh()
    await flush()
    expect(session.getSnapshot().error).toBe(false)
    expect(session.getSnapshot().report).toEqual(report())
    stop()
  })

  it("persists collapsed projects and restores them in a new session", () => {
    const { session } = setup()
    session.toggleProject("-p")
    expect(session.getSnapshot().collapsedProjects.has("-p")).toBe(true)
    expect(setup().session.getSnapshot().collapsedProjects.has("-p")).toBe(true)
    session.toggleProject("-p")
    expect(setup().session.getSnapshot().collapsedProjects.has("-p")).toBe(false)
  })

  it("keeps expanded memories in memory only", () => {
    const { session } = setup()
    session.toggleMemory("/p/a.md")
    expect(session.getSnapshot().expandedMemories.has("/p/a.md")).toBe(true)
    expect(setup().session.getSnapshot().expandedMemories.size).toBe(0)
  })

  it("exposes the surface with its state and conceals it when inactive", async () => {
    const { adapter, session } = setup()
    const stop = session.subscribe(() => undefined)
    expect(noteInteraction).toHaveBeenCalledWith({
      kind: "surfaceViewed",
      surface: "memories",
      origin: "user",
    })
    expect(noteInteraction).not.toHaveBeenCalledWith(
      expect.objectContaining({ kind: "surfaceStateObserved" }),
    )
    await flush()
    expect(noteInteraction).toHaveBeenCalledWith({
      kind: "surfaceStateObserved",
      surface: "memories",
      state: "ready",
      origin: "user",
    })
    stop()
    noteInteraction.mockClear()
    vi.mocked(adapter.listMemories).mockResolvedValueOnce(report([]))
    const again = session.subscribe(() => undefined)
    await flush()
    expect(noteInteraction).toHaveBeenCalledWith(
      expect.objectContaining({ kind: "surfaceViewed", surface: "memories" }),
    )
    expect(noteInteraction).toHaveBeenCalledWith(
      expect.objectContaining({ kind: "surfaceStateObserved", state: "empty" }),
    )
    again()
    session.dispose()
  })

  it("observes an error state", async () => {
    const { session } = setup({ listMemories: vi.fn().mockRejectedValue(new Error("no")) })
    const stop = session.subscribe(() => undefined)
    await flush()
    expect(noteInteraction).toHaveBeenCalledWith(
      expect.objectContaining({ kind: "surfaceStateObserved", state: "error" }),
    )
    stop()
  })

  it("sends a reveal to the adapter", async () => {
    const { adapter, session } = setup()
    await session.reveal("/p/a.md")
    expect(adapter.reveal).toHaveBeenCalledWith("/p/a.md")
  })
})

describe("sortMemories", () => {
  const facts = (referenced: number | null, written: number | null) => ({
    ...entry().facts,
    lastReferencedMs: referenced,
    lastWrittenMs: written,
  })

  it("orders by last referenced ascending with nulls last", () => {
    const sorted = sortMemories([
      entry({ title: "none", facts: facts(null, 1) }),
      entry({ title: "new", facts: facts(20, 1) }),
      entry({ title: "old", facts: facts(10, 1) }),
    ])
    expect(sorted.map((e) => e.title)).toEqual(["old", "new", "none"])
  })

  it("breaks ties by last written, then by title", () => {
    const sorted = sortMemories([
      entry({ title: "b", facts: facts(10, 5) }),
      entry({ title: "z", facts: facts(10, null) }),
      entry({ title: "a", facts: facts(10, 5) }),
      entry({ title: "first", facts: facts(10, 2) }),
    ])
    expect(sorted.map((e) => e.title)).toEqual(["first", "a", "b", "z"])
  })

  it("does not change its input", () => {
    const input = [entry({ title: "b" }), entry({ title: "a" })]
    sortMemories(input)
    expect(input.map((e) => e.title)).toEqual(["b", "a"])
  })
})
