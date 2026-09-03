import { describe, expect, it, vi } from "vitest"

import type { AnchorRegion } from "./anchorRegion"
import {
  AnchoredTriggerController,
  type AnchoredTriggerBridge,
  type AnchoredWindowLifecycleEvent,
  type AnchoredWindowRequest,
} from "./anchoredTrigger"

type Target = { id: string }

const ANCHOR: AnchorRegion = { top: 12, height: 24 }

function harness(
  request: AnchoredTriggerBridge<Target>["request"] = vi.fn(async (target: Target) => ({
    generation: 1,
    target,
    retargetCommitRequired: false,
  })),
) {
  let lifecycle: ((event: AnchoredWindowLifecycleEvent<Target>) => void) | null = null
  const bridge: AnchoredTriggerBridge<Target> = {
    request,
    conceal: vi.fn(async () => undefined),
    listen: vi.fn(async (handler) => {
      lifecycle = handler
      return () => undefined
    }),
    state: vi.fn(async () => state(0, null)),
  }
  const controller = new AnchoredTriggerController(
    "detail",
    (left, right) => left.id === right.id,
    bridge,
  )
  const unsubscribe = controller.subscribe(() => undefined)
  return {
    bridge,
    controller,
    emit(state: AnchoredWindowLifecycleEvent<Target>["state"]) {
      lifecycle?.({ companionLabel: "detail", state })
    },
    unsubscribe,
  }
}

function state(generation: number, target: Target | null) {
  return {
    generation,
    target,
    rendererReady: true,
    visible: target != null,
    awaitingRetargetCommit: false,
    awaitingPresentation: false,
    awaitingConcealment: target == null,
  }
}

describe("AnchoredTriggerController", () => {
  it("keeps each queued presentation paired with its target", async () => {
    const pending = new Map<
      string,
      {
        resolve: (request: AnchoredWindowRequest<Target>) => void
      }
    >()
    const request = vi.fn(
      (target: Target, _anchor: AnchorRegion, _presentation: string | undefined) =>
        new Promise<AnchoredWindowRequest<Target>>((resolve) => {
          pending.set(target.id, { resolve })
        }),
    )
    const bridge: AnchoredTriggerBridge<Target, string> = {
      request,
      conceal: vi.fn(async () => undefined),
      listen: vi.fn(async () => () => undefined),
      state: vi.fn(async () => state(0, null)),
    }
    const controller = new AnchoredTriggerController(
      "detail",
      (left: Target, right: Target) => left.id === right.id,
      bridge,
    )

    const second = controller.hover({ id: "second" }, ANCHOR, "second presentation")
    const third = controller.hover({ id: "third" }, ANCHOR, "third presentation")

    expect(request).toHaveBeenNthCalledWith(1, { id: "second" }, ANCHOR, "second presentation")
    pending.get("second")?.resolve({
      generation: 2,
      target: { id: "second" },
      retargetCommitRequired: true,
    })
    await second
    expect(request).toHaveBeenNthCalledWith(2, { id: "third" }, ANCHOR, "third presentation")
    pending.get("third")?.resolve({
      generation: 3,
      target: { id: "third" },
      retargetCommitRequired: true,
    })
    await third

    await controller.leave()
    const reentry = controller.hover({ id: "third" }, ANCHOR, "replacement presentation")
    pending.get("third")?.resolve({
      generation: 3,
      target: { id: "third" },
      retargetCommitRequired: false,
    })
    await reentry
    expect(controller.getSnapshot()).toMatchObject({
      activation: "hovered",
      target: { id: "third" },
      generation: 3,
    })
  })

  it("retains hover until native lifecycle concealment", async () => {
    const { bridge, controller, emit, unsubscribe } = harness()
    const target = { id: "alpha" }

    await controller.hover(target, ANCHOR)
    expect(controller.getSnapshot()).toMatchObject({ activation: "hovered", target })

    await controller.leave()
    expect(controller.getSnapshot().activation).toBe("hovered")
    expect(bridge.conceal).toHaveBeenCalledOnce()

    emit(state(1, target))
    expect(controller.getSnapshot().activation).toBe("hovered")
    emit(state(2, null))
    expect(controller.getSnapshot()).toEqual({
      activation: "idle",
      target: null,
      generation: 2,
    })
    unsubscribe()
  })

  it("reconciles lifecycle state after a listener retry", async () => {
    vi.useFakeTimers()
    const bridge: AnchoredTriggerBridge<Target> = {
      request: vi.fn(async (target) => ({
        generation: 1,
        target,
        retargetCommitRequired: false,
      })),
      conceal: vi.fn(async () => undefined),
      listen: vi
        .fn()
        .mockRejectedValueOnce(new Error("listener unavailable"))
        .mockResolvedValueOnce(() => undefined),
      state: vi.fn(async () => state(2, null)),
    }
    const controller = new AnchoredTriggerController(
      "detail",
      (left: Target, right: Target) => left.id === right.id,
      bridge,
    )
    const unsubscribe = controller.subscribe(() => undefined)

    try {
      await controller.hover({ id: "stale" }, ANCHOR)
      await vi.advanceTimersByTimeAsync(250)

      expect(controller.getSnapshot()).toEqual({
        activation: "idle",
        target: null,
        generation: 2,
      })
    } finally {
      unsubscribe()
      vi.useRealTimers()
    }
  })

  it("serializes a late old success behind the latest local target", async () => {
    const pending = new Map<
      string,
      {
        resolve: (request: AnchoredWindowRequest<Target>) => void
      }
    >()
    const request = vi.fn(
      (target: Target) =>
        new Promise<AnchoredWindowRequest<Target>>((resolve) => {
          pending.set(target.id, { resolve })
        }),
    )
    const { controller, emit, unsubscribe } = harness(request)

    const oldRequest = controller.hover({ id: "old" }, ANCHOR)
    const newRequest = controller.hover({ id: "new" }, ANCHOR)
    expect(request).toHaveBeenCalledTimes(1)
    pending.get("old")?.resolve({
      generation: 4,
      target: { id: "old" },
      retargetCommitRequired: false,
    })
    await oldRequest
    expect(controller.getSnapshot()).toMatchObject({
      activation: "hovered",
      target: { id: "new" },
    })
    expect(request).toHaveBeenCalledTimes(2)

    emit(state(4, null))
    expect(controller.getSnapshot()).toMatchObject({
      activation: "hovered",
      target: { id: "new" },
    })

    pending.get("new")?.resolve({
      generation: 5,
      target: { id: "new" },
      retargetCommitRequired: false,
    })
    await newRequest
    emit(state(4, { id: "old" }))
    expect(controller.getSnapshot()).toMatchObject({
      activation: "hovered",
      target: { id: "new" },
      generation: 5,
    })

    emit(state(6, null))
    expect(controller.getSnapshot()).toEqual({
      activation: "idle",
      target: null,
      generation: 6,
    })

    const leavingRequest = controller.hover({ id: "leaving" }, ANCHOR)
    await controller.leave()
    emit(state(7, null))
    expect(controller.getSnapshot()).toEqual({
      activation: "idle",
      target: null,
      generation: 7,
    })
    pending.get("leaving")?.resolve({
      generation: 8,
      target: { id: "leaving" },
      retargetCommitRequired: false,
    })
    await leavingRequest
    expect(controller.getSnapshot()).toEqual({
      activation: "idle",
      target: null,
      generation: 7,
    })
    unsubscribe()
  })
})
