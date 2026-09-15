import { beforeEach, describe, expect, it, vi } from "vitest"

import {
  getPopoverPeekAnchorState,
  getPopoverPeekData,
  getPopoverPeekState,
  hidePopoverPeek,
  onPopoverPeekLifecycle,
  onPopoverPeekRequest,
  popoverPeekConcealed,
  popoverPeekPresented,
  popoverPeekReady,
  popoverPeekRetargetReady,
  showPopoverPeek,
  type PopoverPeekRequest,
} from "./popoverPeekIpc"

const shell = vi.hoisted(() => ({ present: false }))
const tauriInvoke = vi.hoisted(() => vi.fn())
const tauriListen = vi.hoisted(() => vi.fn())

vi.mock("@tauri-apps/api/core", () => ({
  invoke: tauriInvoke,
  isTauri: () => shell.present,
}))
vi.mock("@tauri-apps/api/event", () => ({ listen: tauriListen }))

const nativeInvoke = vi.fn()
const nativeListen = vi.fn()

function installNativeBridge(): void {
  Object.defineProperty(window, "__ANTIBURN_NATIVE_PEEK__", {
    configurable: true,
    value: { invoke: nativeInvoke, listen: nativeListen },
  })
}

describe("popover peek IPC", () => {
  beforeEach(() => {
    shell.present = false
    tauriInvoke.mockReset()
    tauriListen.mockReset()
    nativeInvoke.mockReset()
    nativeListen.mockReset()
    Reflect.deleteProperty(window, "__ANTIBURN_NATIVE_PEEK__")
  })

  it("uses the native bridge for companion commands before checking for Tauri", async () => {
    installNativeBridge()
    nativeInvoke.mockResolvedValue(true)

    await getPopoverPeekState()
    await getPopoverPeekData(17)
    await popoverPeekReady(5)
    await popoverPeekPresented(17, 196)
    await popoverPeekRetargetReady(18, null)
    await popoverPeekConcealed(19)

    expect(nativeInvoke.mock.calls).toEqual([
      ["get_popover_peek_state", undefined],
      ["get_popover_peek_data", { generation: 17 }],
      ["popover_peek_ready", { generation: 5 }],
      ["popover_peek_presented", { generation: 17, contentHeight: 196 }],
      ["popover_peek_retarget_ready", { generation: 18, contentHeight: null }],
      ["popover_peek_concealed", { generation: 19 }],
    ])
    expect(tauriInvoke).not.toHaveBeenCalled()
  })

  it("delivers the native request payload and returns its unlisten function", async () => {
    installNativeBridge()
    const unlisten = vi.fn()
    const delivery: { callback: ((payload: unknown) => void) | null } = { callback: null }
    nativeListen.mockImplementation(async (_event, callback) => {
      delivery.callback = callback
      return unlisten
    })
    const handler = vi.fn()
    const request: PopoverPeekRequest = {
      generation: 23,
      target: { kind: "checks" },
      retargetCommitRequired: false,
      initialPresentation: null,
    }

    const stop = await onPopoverPeekRequest(handler)
    delivery.callback?.(request)
    stop()

    expect(nativeListen).toHaveBeenCalledWith("anchored-window-request", expect.any(Function))
    expect(handler).toHaveBeenCalledWith(request)
    expect(unlisten).toHaveBeenCalledOnce()
    expect(tauriListen).not.toHaveBeenCalled()
  })

  it("keeps anchor operations on the Tauri transport", async () => {
    shell.present = true
    installNativeBridge()
    tauriInvoke.mockResolvedValue({
      generation: 3,
      target: null,
      rendererReady: true,
      visible: false,
      awaitingRetargetCommit: false,
      awaitingPresentation: false,
      awaitingConcealment: false,
    })
    tauriListen.mockResolvedValue(() => undefined)

    await showPopoverPeek({ kind: "checks" }, { top: 12, height: 30 })
    await hidePopoverPeek()
    await getPopoverPeekAnchorState()
    await onPopoverPeekLifecycle(() => undefined)

    expect(tauriInvoke).toHaveBeenNthCalledWith(1, "show_popover_peek", {
      target: { kind: "checks" },
      anchor: { top: 12, height: 30 },
      initialPresentation: null,
    })
    expect(tauriInvoke).toHaveBeenNthCalledWith(2, "hide_popover_peek")
    expect(tauriInvoke).toHaveBeenNthCalledWith(3, "get_popover_peek_state")
    expect(tauriListen).toHaveBeenCalledWith("anchored-window-state", expect.any(Function))
    expect(nativeInvoke).not.toHaveBeenCalled()
    expect(nativeListen).not.toHaveBeenCalled()
  })
})
