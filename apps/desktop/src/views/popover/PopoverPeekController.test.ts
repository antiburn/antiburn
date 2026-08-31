import { describe, expect, it, vi } from "vitest"

import type { PopoverPeekData } from "../../lib/popoverPeekIpc"
import { PopoverPeekController } from "./PopoverPeekController"

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason?: unknown) => void
  const promise = new Promise<T>((settle, fail) => {
    resolve = settle
    reject = fail
  })
  return { promise, reject, resolve }
}

function providerTarget(provider: string) {
  return { kind: "provider" as const, provider, utcOffsetMinutes: 600 }
}

function request(generation: number, provider: string | null) {
  return {
    generation,
    target: provider == null ? null : providerTarget(provider),
    retargetCommitRequired: false,
    initialPresentation: null,
  }
}

function providerData(generatedAt: string): PopoverPeekData {
  return {
    kind: "provider",
    summary: { providers: [], generatedAt },
    live: { providers: [], errors: [], meters: [], generatedAt },
  }
}

function controllerWith(data: (generation: number) => Promise<PopoverPeekData>) {
  return new PopoverPeekController({
    data,
    listen: vi.fn(async () => () => undefined),
    state: vi.fn(),
  })
}

describe("PopoverPeekController", () => {
  it("marks a cold request as loading without a presented payload", () => {
    const controller = controllerWith(
      vi.fn(() => new Promise<PopoverPeekData>(() => undefined)),
    )

    controller.accept(request(1, "first"))

    expect(controller.getSnapshot()).toMatchObject({
      requested: { generation: 1, target: providerTarget("first") },
      presented: null,
      candidate: null,
      coldLoading: true,
      failed: null,
    })
  })

  it("uses an initial presentation immediately without starting another load", () => {
    const data = vi.fn(() => new Promise<PopoverPeekData>(() => undefined))
    const controller = controllerWith(data)
    const seeded = providerData("seeded")

    controller.accept({
      ...request(2, "second"),
      retargetCommitRequired: true,
      initialPresentation: seeded,
    })

    expect(controller.getSnapshot()).toMatchObject({
      requested: { generation: 2, target: providerTarget("second") },
      presented: { request: { generation: 2 }, data: seeded },
      candidate: null,
      coldLoading: false,
    })
    expect(data).not.toHaveBeenCalled()
  })

  it("retains A while B loads and promotes B only after presentation", async () => {
    const first = deferred<PopoverPeekData>()
    const second = deferred<PopoverPeekData>()
    const data = vi.fn().mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise)
    const controller = controllerWith(data)

    controller.accept(request(1, "first"))
    first.resolve(providerData("first"))
    await first.promise
    await Promise.resolve()
    controller.promote(1)

    controller.accept(request(2, "second"))

    expect(controller.getSnapshot()).toMatchObject({
      requested: { generation: 2, target: providerTarget("second") },
      presented: { request: { generation: 1 }, data: { summary: { generatedAt: "first" } } },
      candidate: null,
      coldLoading: false,
    })

    second.resolve(providerData("second"))
    await second.promise
    await Promise.resolve()

    expect(controller.getSnapshot()).toMatchObject({
      presented: { request: { generation: 1 } },
      candidate: { request: { generation: 2 }, data: { summary: { generatedAt: "second" } } },
    })

    controller.promote(2)
    expect(controller.getSnapshot()).toMatchObject({
      presented: { request: { generation: 2 }, data: { summary: { generatedAt: "second" } } },
      candidate: null,
    })
  })

  it("serializes loads, replaces the pending target, and rejects stale B", async () => {
    const first = deferred<PopoverPeekData>()
    const third = deferred<PopoverPeekData>()
    const data = vi.fn().mockReturnValueOnce(first.promise).mockReturnValueOnce(third.promise)
    const controller = controllerWith(data)

    controller.accept(request(1, "first"))
    controller.accept(request(2, "second"))
    controller.accept(request(3, "third"))

    expect(data).toHaveBeenCalledTimes(1)
    expect(data).toHaveBeenLastCalledWith(1)

    first.resolve(providerData("first"))
    await first.promise
    await Promise.resolve()

    expect(controller.getSnapshot().requested.generation).toBe(3)
    expect(controller.getSnapshot().candidate).toBeNull()
    expect(data).toHaveBeenCalledTimes(2)
    expect(data).toHaveBeenLastCalledWith(3)

    third.resolve(providerData("third"))
    await third.promise
    await Promise.resolve()

    expect(controller.getSnapshot().candidate).toMatchObject({
      request: { generation: 3 },
      data: { summary: { generatedAt: "third" } },
    })
  })

  it("ignores a same-target lifecycle echo without loading or publishing", () => {
    const data = vi.fn(() => new Promise<PopoverPeekData>(() => undefined))
    const controller = controllerWith(data)
    const listener = vi.fn()
    const unsubscribe = controller.subscribe(listener)

    controller.accept(request(4, "same"))
    listener.mockClear()
    controller.accept(request(4, "same"))

    expect(listener).not.toHaveBeenCalled()
    expect(data).toHaveBeenCalledOnce()
    unsubscribe()
  })

  it("publishes only a current failure as an unavailable candidate", async () => {
    const first = deferred<PopoverPeekData>()
    const second = deferred<PopoverPeekData>()
    const data = vi.fn().mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise)
    const controller = controllerWith(data)

    controller.accept(request(1, "first"))
    controller.accept(request(2, "second"))
    first.reject(new Error("stale"))
    await first.promise.catch(() => undefined)
    await Promise.resolve()

    expect(controller.getSnapshot().failed).toBeNull()

    second.reject(new Error("current"))
    await second.promise.catch(() => undefined)
    await Promise.resolve()

    expect(controller.getSnapshot().failed).toMatchObject({
      generation: 2,
      target: providerTarget("second"),
    })
    expect(controller.getSnapshot().candidate).toBeNull()
  })

  it("clears all presentation state synchronously on conceal", async () => {
    const loaded = deferred<PopoverPeekData>()
    const controller = controllerWith(vi.fn(() => loaded.promise))

    controller.accept(request(1, "first"))
    loaded.resolve(providerData("first"))
    await loaded.promise
    await Promise.resolve()
    controller.promote(1)

    controller.accept(request(2, null))

    expect(controller.getSnapshot()).toEqual({
      requested: {
        generation: 2,
        target: null,
        retargetCommitRequired: false,
        initialPresentation: null,
      },
      presented: null,
      candidate: null,
      coldLoading: false,
      failed: null,
    })
  })
})
