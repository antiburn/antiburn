import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { memoryCountStore } from "../../../lib/memoryCountStore"
import type { AgentMemoriesReport, MemoryEditOutcome } from "../../../lib/memoriesIpc"
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
    archive: vi.fn().mockResolvedValue({
      outcome: "archived",
      archiveId: "1-a.md",
      indexLineRemoved: true,
    }),
    restore: vi.fn().mockResolvedValue({ outcome: "restored", indexLineRestored: true }),
    removeIndexLine: vi.fn().mockResolvedValue({ outcome: "indexLineRemoved" }),
    noteInteraction: noteInteraction,
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
afterEach(() => {
  localStorage.clear()
  vi.restoreAllMocks()
})

describe("MemoriesSession", () => {
  it("loads on the first active subscribe", async () => {
    const accept = vi.spyOn(memoryCountStore, "acceptReport")
    const { adapter, session } = setup()
    const stop = session.subscribe(() => undefined)
    await flush()
    expect(adapter.listMemories).toHaveBeenCalledTimes(1)
    expect(session.getSnapshot().report).toEqual(report())
    expect(accept).toHaveBeenCalledWith(report(), expect.any(Number))
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

  it("focus un-collapses the project, expands the row and numbers each request", () => {
    const { session } = setup()
    session.toggleProject("-p")
    session.focus("-p", "/p/memory/a.md")
    let snapshot = session.getSnapshot()
    expect(snapshot.collapsedProjects.has("-p")).toBe(false)
    expect(setup().session.getSnapshot().collapsedProjects.has("-p")).toBe(false)
    expect(snapshot.expandedMemories.has("/p/memory/a.md")).toBe(true)
    const first = snapshot.focusRequest!
    expect(first.path).toBe("/p/memory/a.md")
    session.focus("-p", "/p/memory/a.md")
    snapshot = session.getSnapshot()
    expect(snapshot.focusRequest!.revision).toBeGreaterThan(first.revision)
  })

  it("focusHandled clears only the matching revision", () => {
    const { session } = setup()
    session.focus("-p", "/p/memory/a.md")
    const { revision } = session.getSnapshot().focusRequest!
    session.focusHandled(revision - 1)
    expect(session.getSnapshot().focusRequest).not.toBeNull()
    session.focusHandled(revision)
    expect(session.getSnapshot().focusRequest).toBeNull()
  })

  it("drops a pending focus request when the view is left", async () => {
    const { session } = setup()
    const stop = session.subscribe(() => undefined)
    await flush()
    session.focus("-p", "/p/memory/a.md")
    stop()
    expect(session.getSnapshot().focusRequest).toBeNull()
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

  it("sends a reveal to the adapter and measures it", async () => {
    const { adapter, session } = setup()
    await session.reveal("/p/a.md")
    expect(adapter.reveal).toHaveBeenCalledWith("/p/a.md")
    expect(noteInteraction).toHaveBeenCalledWith({
      kind: "memoryAction",
      action: "reveal",
      outcome: "succeeded",
    })
  })

  it("measures a failed reveal as failed", async () => {
    const { session } = setup({ reveal: vi.fn().mockRejectedValue(new Error("no")) })
    await session.reveal("/p/a.md")
    expect(memoryActions()).toEqual([{ action: "reveal", outcome: "failed" }])
  })
})

function memoryActions() {
  return noteInteraction.mock.calls
    .map(([interaction]) => interaction)
    .filter((interaction) => interaction.kind === "memoryAction")
    .map(({ action, outcome }) => ({ action, outcome }))
}

describe("MemoriesSession edits", () => {
  const target = project()
  const memory = target.memories[0]!

  async function active(overrides: Partial<MemoriesAdapter> = {}) {
    const made = setup(overrides)
    const stop = made.session.subscribe(() => undefined)
    await flush()
    return { ...made, stop }
  }

  it("archives with the listed size and time, then keeps the row", async () => {
    const invalidate = vi.spyOn(memoryCountStore, "invalidate")
    const { adapter, session, stop } = await active()
    await session.archive(target, { ...memory, sizeBytes: 12, modifiedMs: 34 })
    expect(adapter.archive).toHaveBeenCalledWith("-p", "a.md", 12, 34)
    expect(invalidate).toHaveBeenCalledOnce()
    expect(session.getSnapshot().archived.get(memory.path)).toEqual({
      slug: "-p",
      archiveId: "1-a.md",
      indexLineRemoved: true,
    })
    expect(session.getSnapshot().indexBackupWritten.has("-p")).toBe(true)
    expect(memoryActions()).toEqual([{ action: "archive", outcome: "succeeded" }])
    stop()
  })

  it("does not claim a backup when no index line was removed", async () => {
    const { session, stop } = await active({
      archive: vi.fn().mockResolvedValue({
        outcome: "archived",
        archiveId: "1-a.md",
        indexLineRemoved: false,
      }),
    })
    await session.archive(target, memory)
    expect(session.getSnapshot().indexBackupWritten.size).toBe(0)
    stop()
  })

  it("marks a changed file and does not strike the row", async () => {
    const { session, stop } = await active({
      archive: vi.fn().mockResolvedValue({ outcome: "changedOnDisk" }),
    })
    await session.archive(target, memory)
    expect(session.getSnapshot().archived.size).toBe(0)
    expect(session.getSnapshot().rowErrors.get(memory.path)).toBe("changedOnDisk")
    expect(memoryActions()).toEqual([{ action: "archive", outcome: "changed_on_disk" }])
    stop()
  })

  it("refreshes the sidebar count after restore", async () => {
    const { session, stop } = await active()
    await session.archive(target, memory)
    const invalidate = vi.spyOn(memoryCountStore, "invalidate")
    await session.undo(memory)
    expect(invalidate).toHaveBeenCalledOnce()
    stop()
  })

  it("treats a missing file as a change on disk", async () => {
    const { session, stop } = await active({
      archive: vi.fn().mockResolvedValue({ outcome: "missing" }),
    })
    await session.archive(target, memory)
    expect(session.getSnapshot().rowErrors.get(memory.path)).toBe("changedOnDisk")
    expect(memoryActions()).toEqual([{ action: "archive", outcome: "changed_on_disk" }])
    stop()
  })

  it("reports an unsupported platform and other unavailable reasons differently", async () => {
    const unsupported = await active({
      archive: vi
        .fn()
        .mockResolvedValue({ outcome: "unavailable", reason: "automaticapplyunsupported" }),
    })
    await unsupported.session.archive(target, memory)
    expect(unsupported.session.getSnapshot().rowErrors.get(memory.path)).toBe("failed")
    expect(memoryActions()).toEqual([{ action: "archive", outcome: "unsupported" }])
    unsupported.stop()

    noteInteraction.mockClear()
    const other = await active({
      archive: vi.fn().mockResolvedValue({ outcome: "unavailable", reason: "symlinktarget" }),
    })
    await other.session.archive(target, memory)
    expect(memoryActions()).toEqual([{ action: "archive", outcome: "failed" }])
    other.stop()
  })

  it("marks a rejected edit as failed and never as succeeded", async () => {
    const { session, stop } = await active({
      archive: vi.fn().mockRejectedValue(new Error("boom")),
    })
    await session.archive(target, memory)
    expect(session.getSnapshot().rowErrors.get(memory.path)).toBe("failed")
    expect(memoryActions()).toEqual([{ action: "archive", outcome: "failed" }])
    stop()
  })

  it("undoes a delete: restores and forgets the delete without a reload", async () => {
    const { adapter, session, stop } = await active()
    await session.archive(target, memory)
    noteInteraction.mockClear()
    await session.undo(memory)
    expect(adapter.restore).toHaveBeenCalledWith("-p", "1-a.md")
    expect(session.getSnapshot().archived.size).toBe(0)
    expect(adapter.listMemories).toHaveBeenCalledTimes(1)
    expect(memoryActions()).toEqual([{ action: "restore", outcome: "succeeded" }])
    stop()
  })

  it("treats an existing restore target as restored", async () => {
    const { adapter, session, stop } = await active({
      restore: vi.fn().mockResolvedValue({ outcome: "alreadyExists" }),
    })
    await session.archive(target, memory)
    await session.undo(memory)
    expect(session.getSnapshot().archived.size).toBe(0)
    expect(adapter.listMemories).toHaveBeenCalledTimes(1)
    stop()
  })

  it("keeps the deleted row when an undo fails", async () => {
    const { session, stop } = await active({
      restore: vi.fn().mockResolvedValue({ outcome: "unavailable", reason: "writefailed" }),
    })
    await session.archive(target, memory)
    noteInteraction.mockClear()
    await session.undo(memory)
    expect(session.getSnapshot().archived.size).toBe(1)
    expect(session.getSnapshot().rowErrors.get(memory.path)).toBe("failed")
    expect(memoryActions()).toEqual([{ action: "restore", outcome: "failed" }])
    stop()
  })

  it("removes an index line and notes the backup", async () => {
    const entryLine = { title: "Gone", target: "gone.md", lineNumber: 4 }
    const { adapter, session, stop } = await active()
    await session.removeIndexLine(target, entryLine)
    expect(adapter.removeIndexLine).toHaveBeenCalledWith("-p", 4, "gone.md")
    expect(session.getSnapshot().removedIndexLines.has("-p:4")).toBe(true)
    expect(session.getSnapshot().indexBackupWritten.has("-p")).toBe(true)
    expect(memoryActions()).toEqual([{ action: "remove_index_line", outcome: "succeeded" }])
    stop()
  })

  it("moves a later line up after an earlier line was removed", async () => {
    const { adapter, session, stop } = await active()
    await session.removeIndexLine(target, { title: "A", target: "a.md", lineNumber: 2 })
    await session.removeIndexLine(target, { title: "B", target: "b.md", lineNumber: 5 })
    expect(adapter.removeIndexLine).toHaveBeenLastCalledWith("-p", 4, "b.md")
    stop()
  })

  it("drops this visit's edits when the view is left", async () => {
    const { session, stop } = await active()
    await session.archive(target, memory)
    await session.removeIndexLine(target, { title: "A", target: "a.md", lineNumber: 2 })
    stop()
    expect(session.getSnapshot().archived.size).toBe(0)
    expect(session.getSnapshot().removedIndexLines.size).toBe(0)
  })

  it("still measures an edit that settles after the view was left", async () => {
    const pending = deferred<MemoryEditOutcome>()
    const { session, stop } = await active({ archive: vi.fn(() => pending.promise) })
    const edit = session.archive(target, memory)
    stop()
    pending.resolve({ outcome: "archived", archiveId: "1-a.md", indexLineRemoved: false })
    await edit
    expect(session.getSnapshot().archived.size).toBe(0)
    expect(memoryActions()).toEqual([{ action: "archive", outcome: "succeeded" }])
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
