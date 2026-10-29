import { beforeEach, expect, it, vi } from "vitest"
import type { CheckDirectory } from "../../lib/checkDirectories"
import { deferred } from "../main-window/burn-checks/tests/burnChecksTestSupport"
import { CheckDirectorySession } from "./CheckDirectorySession"

const read = vi.hoisted(() => vi.fn())
const open = vi.hoisted(() => vi.fn())
vi.mock("../../lib/checkDirectories", () => ({
  getCheckDirectories: read,
  openCheckDirectory: open,
}))
const rows: CheckDirectory[] = [
  {
    reference: "issued",
    scope: "global",
    label: "This device",
    path: "/fixture/.antiburn/checks",
    state: "ready",
    fileCount: 2,
    message: null,
  },
]
beforeEach(() => {
  read.mockReset().mockResolvedValue(rows)
  open.mockReset().mockResolvedValue(undefined)
})

it("loads once for the first subscriber and does not poll", async () => {
  const session = new CheckDirectorySession()
  expect(read).not.toHaveBeenCalled()
  const stop = session.subscribe(() => undefined)
  const stopOther = session.subscribe(() => undefined)
  await vi.waitFor(() => expect(session.getSnapshot().loading).toBe(false))
  expect(session.getSnapshot().directories).toEqual(rows)
  expect(read).toHaveBeenCalledOnce()
  stopOther()
  expect(read).toHaveBeenCalledOnce()
  stop()
})

it("clears known rows and errors before a revisit loads or fails", async () => {
  const session = new CheckDirectorySession()
  const stop = session.subscribe(() => undefined)
  await vi.waitFor(() => expect(session.getSnapshot().loading).toBe(false))
  open.mockRejectedValueOnce(new Error("private open error"))
  await session.open("issued")
  expect(session.getSnapshot().openErrors.size).toBe(1)
  stop()
  const fresh = deferred<CheckDirectory[]>()
  read.mockReturnValueOnce(fresh.promise)
  const cleanup = session.subscribe(() => undefined)
  expect(session.getSnapshot()).toEqual({
    directories: null,
    loading: true,
    error: null,
    opening: new Set(),
    openErrors: new Map(),
  })
  fresh.reject(new Error("private read error"))
  await vi.waitFor(() => expect(session.getSnapshot().loading).toBe(false))
  expect(session.getSnapshot().directories).toBeNull()
  expect(session.getSnapshot().error).toBe(
    "Could not load check folders. Reopen Settings → Checks to try again.",
  )
  cleanup()
  const stopRecovered = session.subscribe(() => undefined)
  expect(session.getSnapshot().error).toBeNull()
  await vi.waitFor(() => expect(session.getSnapshot().directories).toEqual(rows))
  expect(read).toHaveBeenCalledTimes(3)
  stopRecovered()
})

it.each(["success", "failure"])(
  "fences a late %s and queues only one read for the current visit",
  async (outcome) => {
    const old = deferred<CheckDirectory[]>()
    const fresh = deferred<CheckDirectory[]>()
    read.mockReturnValueOnce(old.promise).mockReturnValueOnce(fresh.promise)
    const session = new CheckDirectorySession()
    let stop = session.subscribe(() => undefined)
    for (let visit = 0; visit < 8; visit++) {
      stop()
      stop = session.subscribe(() => undefined)
    }
    expect(read).toHaveBeenCalledOnce()
    if (outcome === "success") old.resolve(rows)
    else old.reject(new Error("old private read failure"))
    await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(2))
    expect(session.getSnapshot().directories).toBeNull()
    expect(session.getSnapshot().error).toBeNull()
    expect(session.getSnapshot().loading).toBe(true)
    const freshRows = [{ ...rows[0]!, reference: "fresh-issued", fileCount: 3 }]
    fresh.resolve(freshRows)
    await vi.waitFor(() => expect(session.getSnapshot().loading).toBe(false))
    expect(session.getSnapshot().directories).toEqual(freshRows)
    expect(read).toHaveBeenCalledTimes(2)
    stop()
  },
)

it("does not start queued reads or publish results after the last subscriber leaves", async () => {
  const old = deferred<CheckDirectory[]>()
  read.mockReturnValueOnce(old.promise)
  const session = new CheckDirectorySession()
  const listener = vi.fn()
  const stop = session.subscribe(listener)
  stop()
  const cleanup = session.subscribe(listener)
  cleanup()
  listener.mockClear()
  old.resolve(rows)
  await old.promise
  expect(read).toHaveBeenCalledOnce()
  expect(listener).not.toHaveBeenCalled()
  expect(session.getSnapshot().directories).toBeNull()
})

it("suppresses concurrent opens for the same exact issued reference", async () => {
  const session = new CheckDirectorySession()
  const stop = session.subscribe(() => undefined)
  const pending = deferred<void>()
  open.mockReturnValueOnce(pending.promise)
  const first = session.open("issued")
  await session.open("issued")
  await session.open("other-issued")
  expect(open.mock.calls).toEqual([["issued"], ["other-issued"]])
  pending.resolve()
  await first
  expect(session.getSnapshot().opening.size).toBe(0)
  stop()
  await session.open("issued")
  expect(open).toHaveBeenCalledTimes(2)
})

it.each(["success", "failure"])(
  "ignores an old open %s without clearing the new visit's opening state",
  async (outcome) => {
    const old = deferred<void>()
    const fresh = deferred<void>()
    open.mockReturnValueOnce(old.promise).mockReturnValueOnce(fresh.promise)
    const session = new CheckDirectorySession()
    const stop = session.subscribe(() => undefined)
    const oldOpen = session.open("issued")
    stop()
    const cleanup = session.subscribe(() => undefined)
    const freshOpen = session.open("issued")
    if (outcome === "success") old.resolve()
    else old.reject(new Error("old private open failure"))
    await oldOpen
    expect(session.getSnapshot().opening.has("issued")).toBe(true)
    expect(session.getSnapshot().openErrors.size).toBe(0)
    fresh.reject(new Error("current private open failure"))
    await freshOpen
    expect(session.getSnapshot().opening.size).toBe(0)
    expect(session.getSnapshot().openErrors.get("issued")).toBe(
      "Could not open this check folder. Reopen Settings → Checks to try again.",
    )
    cleanup()
  },
)
