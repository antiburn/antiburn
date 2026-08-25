// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { invoke } from "@tauri-apps/api/core"
import { listen } from "@tauri-apps/api/event"
import { WebviewWindow } from "@tauri-apps/api/webviewWindow"

const OVERLAY_WINDOW_LABEL = "antiburn-overlay"
const OVERLAY_VISIBILITY_EVENT = "overlay_visibility_changed"

export function openOverlayWindow(): Promise<void> {
  return invoke("open_overlay_window")
}

export async function hideOverlayWindow(): Promise<void> {
  await invoke("hide_overlay_window")
}

const HUD_PREF_KEY = "antiburn.showFloatingHud"

export function isFloatingHudEnabled(): boolean {
  try {
    return localStorage.getItem(HUD_PREF_KEY) === "1"
  } catch {
    return false
  }
}

export function setFloatingHudEnabled(enabled: boolean): void {
  try {
    localStorage.setItem(HUD_PREF_KEY, enabled ? "1" : "0")
  } catch {
    // The HUD still works when preference storage is unavailable.
  }
}

async function isOverlayWindowVisible(): Promise<boolean> {
  try {
    const overlay = await WebviewWindow.getByLabel(OVERLAY_WINDOW_LABEL)
    return (await overlay?.isVisible()) ?? false
  } catch {
    return false
  }
}

/** Keep HUD controls synchronized with the native window visibility. */
export class HudVisibilitySession {
  private listeners = new Set<() => void>()
  private started = false
  private generation = 0
  private revision = 0
  private visible = isFloatingHudEnabled()
  private stopVisibilityListening: (() => void) | null = null

  getSnapshot = (): boolean => this.visible

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener)
    if (!this.started) this.start()
    return () => {
      this.listeners.delete(listener)
      if (this.listeners.size === 0) this.stop()
    }
  }

  set = (visible: boolean): void => {
    this.revision += 1
    this.setVisible(visible)
    setFloatingHudEnabled(visible)
    void (visible ? openOverlayWindow() : hideOverlayWindow()).catch(() => {})
  }

  toggle = (): void => this.set(!this.visible)

  private start(): void {
    this.started = true
    const generation = ++this.generation
    const read = () => {
      const revision = ++this.revision
      void isOverlayWindowVisible().then((visible) => {
        if (this.started && this.generation === generation && this.revision === revision) {
          this.setVisible(visible)
        }
      })
    }
    this.read = read
    void listen<boolean>(OVERLAY_VISIBILITY_EVENT, (event) => {
      if (!this.started || this.generation !== generation) return
      const visible = Boolean(event.payload)
      this.revision += 1
      setFloatingHudEnabled(visible)
      this.setVisible(visible)
    })
      .then((dispose) => {
        if (this.started && this.generation === generation) {
          this.stopVisibilityListening = dispose
        } else {
          dispose()
        }
      })
      .catch(() => {})
    read()
    window.addEventListener("focus", read)
  }

  private read: (() => void) | null = null

  private stop(): void {
    this.started = false
    this.generation += 1
    this.revision += 1
    if (this.read) window.removeEventListener("focus", this.read)
    this.read = null
    this.stopVisibilityListening?.()
    this.stopVisibilityListening = null
  }

  private setVisible(visible: boolean): void {
    if (visible === this.visible) return
    this.visible = visible
    for (const listener of this.listeners) listener()
  }
}
