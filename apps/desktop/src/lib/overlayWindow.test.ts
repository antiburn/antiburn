import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { HudVisibilitySession } from "./overlayWindow"

const invoke = vi.hoisted(() => vi.fn(async () => {}))
vi.mock("@tauri-apps/api/core", () => ({ invoke }))

const nativeVisibility = vi.hoisted(() => ({ visible: false }))
const isVisible = vi.hoisted(() => vi.fn(async () => nativeVisibility.visible))
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  WebviewWindow: {
    getByLabel: vi.fn(async () => ({ isVisible })),
  },
}))

const visibilityEvent = vi.hoisted(() => ({
  emit: null as ((visible: boolean) => void) | null,
  dispose: vi.fn(),
}))
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (_event: string, handler: (event: { payload: boolean }) => void) => {
    visibilityEvent.emit = (visible: boolean) => handler({ payload: visible })
    return visibilityEvent.dispose
  }),
}))

const stored = new Map<string, string>()
const storage = {
  getItem: (key: string) => stored.get(key) ?? null,
  setItem: (key: string, value: string) => stored.set(key, value),
  removeItem: (key: string) => stored.delete(key),
  clear: () => stored.clear(),
  key: (index: number) => [...stored.keys()][index] ?? null,
  get length() {
    return stored.size
  },
}

async function flush() {
  await Promise.resolve()
  await Promise.resolve()
}

describe("HudVisibilitySession", () => {
  beforeEach(() => {
    vi.stubGlobal("localStorage", storage)
    stored.clear()
    nativeVisibility.visible = false
    isVisible.mockReset()
    isVisible.mockImplementation(async () => nativeVisibility.visible)
    invoke.mockClear()
    visibilityEvent.emit = null
    visibilityEvent.dispose.mockClear()
  })

  afterEach(() => vi.unstubAllGlobals())

  it("publishes a native close and updates this webview's cached preference", async () => {
    stored.set("antiburn.showFloatingHud", "1")
    nativeVisibility.visible = true
    const session = new HudVisibilitySession()
    const listener = vi.fn()
    const unsubscribe = session.subscribe(listener)
    await flush()
    expect(session.getSnapshot()).toBe(true)

    visibilityEvent.emit!(false)

    expect(session.getSnapshot()).toBe(false)
    expect(stored.get("antiburn.showFloatingHud")).toBe("0")
    expect(listener).toHaveBeenCalled()
    unsubscribe()
  })

  it("does not let a stale visibility read overwrite a newer native event", async () => {
    let resolveVisibility!: (visible: boolean) => void
    isVisible.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveVisibility = resolve
        }),
    )
    const session = new HudVisibilitySession()
    const unsubscribe = session.subscribe(() => {})
    await flush()

    visibilityEvent.emit!(false)
    resolveVisibility(true)
    await flush()

    expect(session.getSnapshot()).toBe(false)
    unsubscribe()
  })
})
