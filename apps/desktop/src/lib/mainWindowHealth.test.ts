import type * as IpcModule from "./ipc"
import { beforeEach, describe, expect, it, vi } from "vitest"

import { installMainWindowHealthResponder } from "./mainWindowHealth"
import {
  markFallbackCommitted,
  markHealthyCommitted,
  resetRendererHealthForTest,
} from "./rendererHealth"

const mocks = vi.hoisted(() => ({
  ack: vi.fn(),
  pending: vi.fn(),
  listen: vi.fn(),
  handler: null as ((request: { requestId: number; generation: number }) => void) | null,
  unlisten: vi.fn(),
}))

vi.mock("./ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof IpcModule>()),
  mainWindowHealthAck: mocks.ack,
  mainWindowPendingHealthCheck: mocks.pending,
  onMainWindowHealthCheck: mocks.listen,
}))

beforeEach(() => {
  vi.clearAllMocks()
  resetRendererHealthForTest()
  Object.defineProperty(window, "__ANTIBURN_WINDOW_GENERATION__", {
    value: 9,
    configurable: true,
  })
  mocks.handler = null
  mocks.pending.mockResolvedValue(null)
  mocks.ack.mockResolvedValue(undefined)
  mocks.listen.mockImplementation(async (handler) => {
    mocks.handler = handler
    return mocks.unlisten
  })
})

describe("installMainWindowHealthResponder", () => {
  it("answers one matching event after the application tree settles", async () => {
    const dispose = await installMainWindowHealthResponder()
    mocks.handler?.({ requestId: 3, generation: 9 })
    mocks.handler?.({ requestId: 3, generation: 9 })
    expect(mocks.ack).not.toHaveBeenCalled()
    markHealthyCommitted()
    expect(mocks.ack).toHaveBeenCalledOnce()
    expect(mocks.ack).toHaveBeenCalledWith(3, 9, true)
    dispose()
    expect(mocks.unlisten).toHaveBeenCalledOnce()
  })

  it("ignores generation mismatches", async () => {
    await installMainWindowHealthResponder()
    mocks.handler?.({ requestId: 3, generation: 8 })
    markHealthyCommitted()
    expect(mocks.ack).not.toHaveBeenCalled()
  })

  it("pulls a request emitted before listener installation", async () => {
    mocks.pending.mockResolvedValue({ requestId: 4, generation: 9 })
    markFallbackCommitted()
    await installMainWindowHealthResponder()
    expect(mocks.pending).toHaveBeenCalledWith(9)
    expect(mocks.ack).toHaveBeenCalledWith(4, 9, false)
  })
})
