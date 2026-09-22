import { beforeEach, expect, it, vi } from "vitest"

const invoke = vi.hoisted(() => vi.fn())
const isTauri = vi.hoisted(() => vi.fn(() => true))
vi.mock("@tauri-apps/api/core", () => ({ invoke, isTauri }))

import { resizeOverlayWindow, setHudDetailSize } from "./hudSizingIpc"

function deferred() {
  let resolve!: () => void
  let reject!: (error: unknown) => void
  const promise = new Promise<void>((accept, fail) => {
    resolve = accept
    reject = fail
  })
  return { promise, resolve, reject }
}

beforeEach(() => {
  invoke.mockReset().mockResolvedValue(undefined)
  isTauri.mockReturnValue(true)
})

it.each([
  ["resize_overlay_window", (height: number) => resizeOverlayWindow(height, false, true)],
  ["set_hud_detail_size", setHudDetailSize],
] as const)("keeps only the latest pending %s measurement", async (command, resize) => {
  const firstReply = deferred()
  const lastReply = deferred()
  invoke.mockReturnValueOnce(firstReply.promise)
  invoke.mockReturnValueOnce(lastReply.promise)
  const first = resize(100)
  const superseded = resize(200)
  const latest = resize(300)
  const settled = vi.fn()
  void superseded.then(settled)
  await Promise.resolve()
  expect(settled).not.toHaveBeenCalled()
  expect(invoke).toHaveBeenCalledTimes(1)
  expect(invoke).toHaveBeenLastCalledWith(command, expect.objectContaining({ height: 100 }))
  firstReply.resolve()
  await first
  expect(settled).not.toHaveBeenCalled()
  expect(invoke).toHaveBeenCalledTimes(2)
  expect(invoke).toHaveBeenLastCalledWith(command, expect.objectContaining({ height: 300 }))
  lastReply.resolve()
  await Promise.all([superseded, latest])
  expect(settled).toHaveBeenCalledOnce()
})

it("continues with the newest size after a native failure", async () => {
  const reply = deferred()
  invoke.mockReturnValueOnce(reply.promise)
  const first = resizeOverlayWindow(100, false, true)
  const failure = expect(first).rejects.toThrow("native failure")
  const latest = resizeOverlayWindow(300, true, false)
  reply.reject(new Error("native failure"))
  await failure
  await latest
  expect(invoke).toHaveBeenLastCalledWith("resize_overlay_window", {
    height: 300,
    anchorBottom: true,
    animate: false,
  })
})

it("coalesces geometry revisions together with their measurements", async () => {
  const reply = deferred()
  invoke.mockReturnValueOnce(reply.promise)
  const first = resizeOverlayWindow(100, false, false, 1)
  const superseded = resizeOverlayWindow(200, false, true, 2)
  const latest = resizeOverlayWindow(150, false, false, 3)
  reply.resolve()
  await Promise.all([first, superseded, latest])
  expect(invoke).toHaveBeenCalledTimes(2)
  expect(invoke).toHaveBeenLastCalledWith("resize_overlay_window", {
    height: 150,
    anchorBottom: false,
    animate: false,
    geometryRevision: 3,
  })
})

it("keeps HUD and detail requests independent", async () => {
  const reply = deferred()
  invoke.mockReturnValueOnce(reply.promise)
  const hud = resizeOverlayWindow(100, false, true)
  await setHudDetailSize(200)
  expect(invoke).toHaveBeenCalledTimes(2)
  reply.resolve()
  await hud
})

it("rejects every pending caller when the latest native size fails", async () => {
  const firstReply = deferred()
  invoke.mockReturnValueOnce(firstReply.promise)
  invoke.mockRejectedValueOnce(new Error("latest failed"))
  const first = setHudDetailSize(100)
  const superseded = setHudDetailSize(200)
  const latest = setHudDetailSize(300)
  const failures = Promise.all([
    expect(superseded).rejects.toThrow("latest failed"),
    expect(latest).rejects.toThrow("latest failed"),
  ])
  firstReply.resolve()
  await first
  await failures
  await setHudDetailSize(400)
  expect(invoke).toHaveBeenLastCalledWith("set_hud_detail_size", { height: 400 })
})

it("does not invoke native sizing outside the shell", async () => {
  isTauri.mockReturnValue(false)
  await resizeOverlayWindow(100, false, true)
  await setHudDetailSize(200)
  expect(invoke).not.toHaveBeenCalled()
})
