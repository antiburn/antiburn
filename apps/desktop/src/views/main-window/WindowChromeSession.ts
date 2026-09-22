import { isTauri } from "@tauri-apps/api/core"
import { getCurrentWindow } from "@tauri-apps/api/window"

type NativeWindow = Pick<
  ReturnType<typeof getCurrentWindow>,
  "isMaximized" | "onResized" | "minimize" | "toggleMaximize" | "close"
>
const STATE_ERROR = "Window state is unavailable. Try the window control again."

type WindowAction = "minimize" | "toggleMaximize" | "close"

export class WindowChromeSession {
  private snapshot = { maximized: false, error: "" }
  private listeners = new Set<() => void>()
  private disconnect: (() => void) | undefined
  private generation = 0
  private request = 0

  private native: () => NativeWindow | null

  constructor(
    native: () => NativeWindow | null = () => (isTauri() ? getCurrentWindow() : null),
  ) {
    this.native = native
  }

  getSnapshot = () => this.snapshot

  subscribe = (listener: () => void) => {
    this.listeners.add(listener)
    if (this.listeners.size === 1) void this.connect()
    return () => {
      this.listeners.delete(listener)
      if (this.listeners.size === 0) {
        this.generation++
        this.request++
        this.disconnect?.()
        this.disconnect = undefined
      }
    }
  }

  private publish(patch: Partial<typeof this.snapshot>) {
    this.snapshot = { ...this.snapshot, ...patch }
    this.listeners.forEach((listener) => listener())
  }

  private async refresh(native: NativeWindow, generation: number) {
    if (generation !== this.generation) return
    const request = ++this.request
    try {
      const maximized = await native.isMaximized()
      if (generation === this.generation && request === this.request) {
        this.publish({
          maximized,
          error: this.snapshot.error === STATE_ERROR ? "" : this.snapshot.error,
        })
      }
    } catch {
      if (generation === this.generation && request === this.request) {
        this.publish({ error: STATE_ERROR })
      }
    }
  }

  private async connect() {
    const native = this.native()
    if (!native) return
    const generation = ++this.generation
    try {
      const disconnect = await native.onResized(() => void this.refresh(native, generation))
      if (generation !== this.generation) {
        disconnect()
        return
      }
      this.disconnect = disconnect
      await this.refresh(native, generation)
    } catch {
      if (generation === this.generation) {
        this.publish({ error: STATE_ERROR })
      }
    }
  }

  perform = async (action: WindowAction) => {
    const native = this.native()
    if (!native) return
    this.publish({ error: "" })
    try {
      await native[action]()
      if (action === "toggleMaximize") await this.refresh(native, this.generation)
    } catch {
      this.publish({ error: "Could not change the window. Please try again." })
    }
  }
}
