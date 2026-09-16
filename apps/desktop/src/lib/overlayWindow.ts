import { invoke } from "@tauri-apps/api/core"
import { listen } from "@tauri-apps/api/event"
import { WebviewWindow } from "@tauri-apps/api/webviewWindow"
import { getCurrentWindow } from "@tauri-apps/api/window"

import type { SurfaceOrigin } from "./ipc"

const OVERLAY_WINDOW_LABEL = "antiburn-overlay"
const OVERLAY_VISIBILITY_EVENT = "overlay_visibility_changed"
const OVERLAY_WORK_EVENT = "overlay_work_changed"
const OVERLAY_DOCK_EVENT = "overlay_dock_changed"

/** The display edge the HUD docks against. Mirrors `DockEdge` in the shell. */
export type HudDockEdge = "left" | "right" | "top" | "bottom"

/** The edge-dock settings. Mirrors `DockSettings` in the shell. */
export interface HudDockSettings {
  enabled: boolean
  edge: HudDockEdge
}

export const HUD_DOCK_EDGES: ReadonlyArray<{ value: HudDockEdge; label: string }> = [
  { value: "left", label: "Left" },
  { value: "right", label: "Right" },
  { value: "top", label: "Top" },
  { value: "bottom", label: "Bottom" },
]

export const DEFAULT_HUD_DOCK: HudDockSettings = { enabled: false, edge: "right" }

/** Read a dock payload the shell sent. Anything malformed means "dock off". */
export function parseHudDock(payload: unknown): HudDockSettings {
  if (typeof payload !== "object" || payload === null) return DEFAULT_HUD_DOCK
  const { enabled, edge } = payload as Partial<HudDockSettings>
  const known = HUD_DOCK_EDGES.some((option) => option.value === edge)
  return { enabled: enabled === true, edge: known && edge ? edge : DEFAULT_HUD_DOCK.edge }
}

/** The edge-dock settings the shell holds. They live there, not in this webview. */
export async function getHudDock(): Promise<HudDockSettings> {
  return parseHudDock(await invoke<unknown>("get_hud_dock"))
}

/** Store the edge-dock settings and apply them to the HUD. */
export function setHudDock(settings: HudDockSettings): Promise<void> {
  return invoke("set_hud_dock", { settings })
}

/** Slide the HUD off its dock edge now. */
export function dockOverlayWindow(): Promise<void> {
  return invoke("dock_overlay")
}

/** Bring a docked HUD back for a while. The shell logs `reason`. */
export function wakeOverlayWindow(reason: "activity" | "burn"): Promise<void> {
  return invoke("wake_overlay", { reason })
}

/** Subscribe to dock settings changes from any window. */
export async function onHudDockChanged(
  handler: (settings: HudDockSettings) => void,
): Promise<() => void> {
  return listen<unknown>(OVERLAY_DOCK_EVENT, (event) => handler(parseHudDock(event.payload)))
}

export function openOverlayWindow(origin: SurfaceOrigin): Promise<void> {
  return invoke("open_overlay_window", { origin })
}

/** Take the reason for the latest successful hidden-to-visible HUD transition. */
export function takeHudAnalyticsOrigin(): Promise<SurfaceOrigin | null> {
  return invoke<SurfaceOrigin | null>("take_hud_analytics_origin")
}

export async function hideOverlayWindow(): Promise<void> {
  await invoke("hide_overlay_window")
}

/** Subscribe to native HUD work transitions. */
export async function onOverlayWorkChanged(
  handler: (active: boolean) => void,
): Promise<() => void> {
  return listen<boolean>(OVERLAY_WORK_EVENT, (event) => handler(Boolean(event.payload)))
}

/**
 * Remember where the HUD sits, after a drag moved it.
 *
 * The shell reads the window position itself, so this carries no coordinates.
 */
export function recordHudPosition(): Promise<void> {
  return invoke("record_hud_position")
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

const TOKEN_MAP_PREF_KEY = "antiburn.showHudTokenMap"

/** Whether the HUD draws the token map above its bars. On until switched off. */
export function isHudTokenMapEnabled(): boolean {
  try {
    return localStorage.getItem(TOKEN_MAP_PREF_KEY) !== "0"
  } catch {
    return true
  }
}

export function setHudTokenMapEnabled(enabled: boolean): void {
  try {
    localStorage.setItem(TOKEN_MAP_PREF_KEY, enabled ? "1" : "0")
  } catch {
    // The map still draws when preference storage is unavailable.
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
    void (visible ? openOverlayWindow("user") : hideOverlayWindow()).catch(() => {})
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
