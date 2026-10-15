import { describe, expect, it, vi } from "vitest"
import { MemoryCountStore } from "./memoryCountStore"
import type { AgentMemoriesReport } from "./memoriesIpc"

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => {
    resolve = done
  })
  return { promise, resolve }
}

function setup(visible = true) {
  let now = 0
  let focus = () => {}
  let visibility = (_value: boolean) => {}
  const count = vi.fn(async () => 12)
  const stopFocus = vi.fn()
  const stopVisible = vi.fn()
  const store = new MemoryCountStore({
    count,
    now: () => now,
    getVisible: async () => visible,
    onVisible: async (callback) => {
      visibility = callback
      return stopVisible
    },
    onFocus: (callback) => {
      focus = callback
      return stopFocus
    },
  })
  return {
    store,
    count,
    stopFocus,
    stopVisible,
    focus: () => focus(),
    visible: (value: boolean) => visibility(value),
    advance: (ms: number) => {
      now += ms
    },
  }
}

const settle = async () => {
  for (let i = 0; i < 6; i += 1) await Promise.resolve()
}
const emptyReport: AgentMemoriesReport = {
  generatedAtMs: 0,
  writesSupported: true,
  projects: [],
}

describe("memory sidebar count", () => {
  it("waits for visibility, refreshes only on stale focus, and cleans up", async () => {
    const test = setup(false)
    const stop = test.store.subscribe(vi.fn())
    await settle()
    expect(test.count).not.toHaveBeenCalled()
    expect(test.store.getSnapshot()).toBeNull()
    test.visible(true)
    await settle()
    expect(test.store.getSnapshot()).toBe(12)
    test.focus()
    test.advance(59_999)
    test.focus()
    expect(test.count).toHaveBeenCalledTimes(1)
    test.advance(1)
    expect(test.count).toHaveBeenCalledTimes(1)
    test.focus()
    await settle()
    expect(test.count).toHaveBeenCalledTimes(2)
    test.visible(false)
    test.advance(60_000)
    test.focus()
    expect(test.count).toHaveBeenCalledTimes(2)
    stop()
    expect(test.stopFocus).toHaveBeenCalledOnce()
    expect(test.stopVisible).toHaveBeenCalledOnce()
  })

  it("coalesces reads and recounts after an edit invalidates an in-flight read", async () => {
    const test = setup()
    const pending = deferred<number>()
    test.count.mockReturnValueOnce(pending.promise).mockResolvedValue(9)
    const stop = test.store.subscribe(vi.fn())
    await settle()
    test.store.invalidate()
    test.focus()
    expect(test.count).toHaveBeenCalledTimes(1)
    pending.resolve(10)
    await settle()
    expect(test.count).toHaveBeenCalledTimes(2)
    expect(test.store.getSnapshot()).toBe(9)
    stop()
  })

  it("accepts full reports and rejects reads and reports made stale by edits", async () => {
    const test = setup()
    const pending = deferred<number>()
    test.count.mockReturnValueOnce(pending.promise)
    const stop = test.store.subscribe(vi.fn())
    await settle()
    test.store.acceptReport(emptyReport, test.store.getRevision())
    pending.resolve(12)
    await settle()
    expect(test.store.getSnapshot()).toBe(0)
    const oldRevision = test.store.getRevision()
    test.store.invalidate()
    test.store.acceptReport(emptyReport, oldRevision)
    await settle()
    expect(test.store.getSnapshot()).toBe(12)
    stop()
  })

  it("keeps unknown and last-good values on failure and retries on a later focus", async () => {
    const test = setup()
    test.count.mockRejectedValueOnce(new Error("unreadable"))
    const stop = test.store.subscribe(vi.fn())
    await settle()
    expect(test.store.getSnapshot()).toBeNull()
    test.focus()
    expect(test.count).toHaveBeenCalledTimes(1)
    test.advance(60_000)
    test.focus()
    await settle()
    expect(test.store.getSnapshot()).toBe(12)
    test.count.mockRejectedValueOnce(new Error("unreadable"))
    test.store.invalidate()
    await settle()
    expect(test.store.getSnapshot()).toBe(12)
    stop()
  })

  it("does not publish a read from a stopped subscription", async () => {
    const test = setup()
    const pending = deferred<number>()
    test.count.mockReturnValueOnce(pending.promise).mockResolvedValue(7)
    const stop = test.store.subscribe(vi.fn())
    await settle()
    stop()
    const nextStop = test.store.subscribe(vi.fn())
    await settle()
    pending.resolve(100)
    await settle()
    expect(test.store.getSnapshot()).toBe(7)
    nextStop()
  })
})
