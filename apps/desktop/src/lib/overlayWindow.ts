// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { invoke } from "@tauri-apps/api/core"
import { WebviewWindow } from "@tauri-apps/api/webviewWindow"
import { getCurrentWindow } from "@tauri-apps/api/window"

const OVERLAY_WINDOW_LABEL = "antiburn-overlay"

export function openOverlayWindow(): Promise<void> {
  return invoke("open_overlay_window")
}

export async function hideOverlayWindow(): Promise<void> {
  const overlay = await WebviewWindow.getByLabel(OVERLAY_WINDOW_LABEL)
  await overlay?.hide()
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

export async function isCurrentWindowVisible(): Promise<boolean> {
  try {
    return await getCurrentWindow().isVisible()
  } catch {
    return false
  }
}

export async function isOverlayWindowVisible(): Promise<boolean> {
  try {
    const overlay = await WebviewWindow.getByLabel(OVERLAY_WINDOW_LABEL)
    return (await overlay?.isVisible()) ?? false
  } catch {
    return false
  }
}

/** Keep a pop-out button synchronized with the native HUD window. */
export class HudVisibilitySession {
  private listeners = new Set<() => void>()
  private started = false
  private generation = 0
  private visible = false

  getSnapshot = (): boolean => this.visible

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener)
    if (!this.started) this.start()
    return () => {
      this.listeners.delete(listener)
      if (this.listeners.size === 0) this.stop()
    }
  }

  toggle = (): void => {
    const next = !this.visible
    this.setVisible(next)
    setFloatingHudEnabled(next)
    void (next ? openOverlayWindow() : hideOverlayWindow()).catch(() => {})
  }

  private start(): void {
    this.started = true
    const generation = ++this.generation
    const read = () => {
      void isOverlayWindowVisible().then((visible) => {
        if (this.started && this.generation === generation) this.setVisible(visible)
      })
    }
    this.read = read
    read()
    window.addEventListener("focus", read)
  }

  private read: (() => void) | null = null

  private stop(): void {
    this.started = false
    this.generation += 1
    if (this.read) window.removeEventListener("focus", this.read)
    this.read = null
  }

  private setVisible(visible: boolean): void {
    if (visible === this.visible) return
    this.visible = visible
    for (const listener of this.listeners) listener()
  }
}
