import { afterEach, expect, it, vi } from "vitest"
import { openSessionActivityStream, SessionActivityTimeout } from "./sessionIpc"

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }))

vi.mock("@tauri-apps/api/core", () => ({ invoke, isTauri: () => true }))

afterEach(() => {
  vi.useRealTimers()
  invoke.mockReset()
})

const batch = { entries: [], exhausted: false }

it("closes a timed-out stream and leaves a replacement available", async () => {
  vi.useFakeTimers()
  try {
    invoke.mockReturnValueOnce(new Promise(() => {})).mockResolvedValue(null)
    const stream = openSessionActivityStream(7)
    const failed = expect(stream.read()).rejects.toBeInstanceOf(SessionActivityTimeout)
    const id = invoke.mock.calls[0]?.[1]?.request.id
    await vi.advanceTimersByTimeAsync(15_000)
    await failed
    expect(invoke).toHaveBeenLastCalledWith("session_activity_stream", {
      request: { kind: "close", id },
    })
    await expect(stream.read()).rejects.toThrow("unavailable")

    invoke.mockResolvedValueOnce({ entries: [], exhausted: true })
    const replacement = openSessionActivityStream(7)
    await expect(replacement.read()).resolves.toEqual({ entries: [], exhausted: true })
    expect(invoke.mock.calls.at(-1)?.[1]?.request.id).not.toBe(id)
    replacement.close()
  } finally {
    vi.useRealTimers()
  }
})

it("reuses the read sequence after a timeout so the shell can replay the same batch", async () => {
  vi.useFakeTimers()
  invoke
    .mockResolvedValueOnce(batch)
    .mockReturnValueOnce(new Promise(() => {}))
    .mockResolvedValue(batch)
  const stream = openSessionActivityStream(7)
  await stream.read()
  const read = stream.read()
  await vi.advanceTimersByTimeAsync(15_250)
  await expect(read).resolves.toEqual(batch)
  const firstRequest = invoke.mock.calls[1]?.[1]?.request
  expect(firstRequest).toMatchObject({ kind: "read", sequence: 0 })
  expect(invoke.mock.calls[2]?.[1]?.request).toEqual(firstRequest)
  await stream.read()
  expect(invoke.mock.calls[3]?.[1]?.request).toMatchObject({ kind: "read", sequence: 1 })
})

it("returns the final failure after one automatic retry and accepts later manual demand", async () => {
  vi.useFakeTimers()
  invoke
    .mockResolvedValueOnce(batch)
    .mockRejectedValueOnce(new Error("First read failed"))
    .mockRejectedValueOnce(new Error("Retry failed"))
    .mockResolvedValue(batch)
  const stream = openSessionActivityStream(7)
  await stream.read()
  const read = expect(stream.read()).rejects.toThrow("Retry failed")
  await vi.advanceTimersByTimeAsync(249)
  expect(invoke).toHaveBeenCalledTimes(2)
  await vi.advanceTimersByTimeAsync(1)
  await read
  expect(invoke).toHaveBeenCalledTimes(3)
  await expect(stream.read()).resolves.toEqual(batch)
  expect(invoke.mock.calls[3]?.[1]?.request).toMatchObject({ kind: "read", sequence: 0 })
})

it("keeps the read sequence available for manual retry after both attempts time out", async () => {
  vi.useFakeTimers()
  const pending = new Promise(() => {})
  invoke
    .mockResolvedValueOnce(batch)
    .mockReturnValueOnce(pending)
    .mockReturnValueOnce(pending)
    .mockResolvedValue(batch)
  const stream = openSessionActivityStream(7)
  await stream.read()
  const failed = expect(stream.read()).rejects.toBeInstanceOf(SessionActivityTimeout)
  await vi.advanceTimersByTimeAsync(30_250)
  await failed
  expect(invoke).toHaveBeenCalledTimes(3)
  await expect(stream.read()).resolves.toEqual(batch)
  expect(invoke.mock.calls[3]?.[1]?.request).toEqual(invoke.mock.calls[1]?.[1]?.request)
})

it("keeps the stream open and retries the same refresh sequence after a timeout", async () => {
  vi.useFakeTimers()
  const reset = { ...batch, reset: true, replayedRefresh: true, nextReadSequence: 1 }
  invoke
    .mockResolvedValueOnce(batch)
    .mockReturnValueOnce(new Promise(() => {}))
    .mockResolvedValueOnce(reset)
    .mockResolvedValueOnce(batch)
  const stream = openSessionActivityStream(7)
  await stream.read()
  const failed = expect(stream.refresh()).rejects.toBeInstanceOf(SessionActivityTimeout)
  await vi.advanceTimersByTimeAsync(15_000)
  await failed
  await expect(stream.refresh()).resolves.toEqual(reset)
  expect(invoke.mock.calls[2]?.[1]?.request).toEqual(invoke.mock.calls[1]?.[1]?.request)
  await stream.read()
  expect(invoke.mock.calls[3]?.[1]?.request).toMatchObject({ kind: "read", sequence: 1 })
})

it("cancels the automatic retry when the stream closes during the delay", async () => {
  vi.useFakeTimers()
  invoke
    .mockResolvedValueOnce(batch)
    .mockRejectedValueOnce(new Error("Read failed"))
    .mockResolvedValue(null)
  const stream = openSessionActivityStream(7)
  await stream.read()
  const read = expect(stream.read()).rejects.toThrow("stream is closed")
  await vi.advanceTimersByTimeAsync(100)
  stream.close()
  await vi.advanceTimersByTimeAsync(250)
  await read
  expect(invoke.mock.calls.map(([, args]) => args.request.kind)).toEqual([
    "open",
    "read",
    "close",
  ])
})
