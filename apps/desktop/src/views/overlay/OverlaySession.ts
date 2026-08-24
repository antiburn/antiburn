// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { LogicalPosition } from "@tauri-apps/api/dpi"
import { listen } from "@tauri-apps/api/event"
import { currentMonitor, getCurrentWindow } from "@tauri-apps/api/window"
import type { MouseEvent as ReactMouseEvent } from "react"
import { flushSync } from "react-dom"

import {
  getLatestSessionActivity,
  getLiveUsage,
  openSettingsWindow,
  resizeOverlayWindow,
} from "../../lib/ipc"
import { hideOverlayWindow, setFloatingHudEnabled } from "../../lib/overlayWindow"
import { prefersReducedMotion } from "../../lib/popoverHeight"
import { deriveUsageBars, type UsageBarItem } from "../../lib/usageBars"

const REFRESH_MS = 60_000
const LIVE_WINDOW_SECS = 90
const LIVENESS_POLL_MS = 5_000
const RESET_CLOCK_MS = 30_000
const SCREEN_MARGIN = 8
const ESTIMATED_CHROME_PX = 48
const ESTIMATED_ROW_PX = 50
const HOVER_INTENT_MS = 250

export type OverlaySnapshot = {
  bars: UsageBarItem[]
  hovered: boolean
  dragging: boolean
  now: number
  sessionLive: boolean
  flipUp: boolean
  panelHeights: {
    collapsed: number
    expanded: number
  }
}

type ExpansionDirection = boolean | "cancelled" | "unavailable"

const INITIAL_SNAPSHOT: OverlaySnapshot = {
  bars: [],
  hovered: false,
  dragging: false,
  now: Date.now(),
  sessionLive: false,
  flipUp: false,
  panelHeights: { collapsed: 0, expanded: 0 },
}

type DragOrigin = {
  pointerX: number
  pointerY: number
  windowX: number
  windowY: number
}

// aislop-ignore-next-line ai-slop/narrative-comment -- Issue #90 owns this standing finding.
/** Own the external systems used by the floating HUD window. */
export class OverlaySession {
  private listeners = new Set<() => void>()
  private started = false
  private generation = 0
  private directionGeneration = 0
  private snapshot: OverlaySnapshot = INITIAL_SNAPSHOT
  private panel: HTMLDivElement | null = null
  private hoverTimer: number | null = null
  private hoverRequested = false
  private usagePoll: number | null = null
  private livenessPoll: number | null = null
  private resetClock: number | null = null
  private stopHoverListening: (() => void) | null = null
  private dragOrigin: DragOrigin | null = null
  private pendingMove: MouseEvent | null = null
  private moveFrame = 0

  getSnapshot = (): OverlaySnapshot => this.snapshot

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener)
    if (!this.started) this.start()
    return () => {
      this.listeners.delete(listener)
      if (this.listeners.size === 0) this.stop()
    }
  }

  registerPanel = (panel: HTMLDivElement | null): void => {
    if (panel === this.panel) return
    this.panel = panel
    if (this.started) this.connectPanel()
  }

  requestHover = (hovered: boolean): void => {
    if (hovered) {
      if (this.hoverRequested) return
      this.hoverRequested = true
      if (this.snapshot.dragging) {
        this.commitLayout({ hovered: true })
        return
      }
      this.hoverTimer = window.setTimeout(() => {
        this.hoverTimer = null
        void this.expandAfterIntent()
      }, HOVER_INTENT_MS)
      return
    }

    this.hoverRequested = false
    this.directionGeneration += 1
    this.clearHoverTimer()
    if (!this.snapshot.hovered) return
    const anchorBottom = this.snapshot.flipUp
    this.commitLayout({ hovered: false })
    if (this.snapshot.dragging) return
    void this.syncWindow(anchorBottom, true)
  }

  startDrag = (event: ReactMouseEvent): void => {
    if ((event.target as HTMLElement).closest("button")) return
    const { screenX, screenY } = event
    void this.beginDrag(screenX, screenY)
  }

  close = (): void => {
    setFloatingHudEnabled(false)
    void hideOverlayWindow().catch(() => {})
  }

  openSettings = (): void => {
    void openSettingsWindow("general").catch(() => {})
  }

  private start(): void {
    this.started = true
    const generation = ++this.generation
    document.body.dataset.transparentWindow = "true"
    this.connectPanel()

    const refreshUsage = () => {
      void getLiveUsage()
        .then((response) => {
          if (!this.isCurrent(generation)) return
          this.commitLayout({ bars: deriveUsageBars(response) })
          void this.resizeAfterDataChange()
        })
        .catch(() => {})
    }
    refreshUsage()
    this.usagePoll = window.setInterval(refreshUsage, REFRESH_MS)

    const refreshLiveness = () => {
      void getLatestSessionActivity()
        .then((latest) => {
          if (!this.isCurrent(generation)) return
          this.update({
            sessionLive: latest != null && Date.now() / 1000 - latest <= LIVE_WINDOW_SECS,
          })
        })
        .catch(() => {})
    }
    refreshLiveness()
    this.livenessPoll = window.setInterval(refreshLiveness, LIVENESS_POLL_MS)
    this.resetClock = window.setInterval(() => this.update({ now: Date.now() }), RESET_CLOCK_MS)

    void listen<boolean>("overlay_hover", (event) => {
      if (this.isCurrent(generation)) this.requestHover(Boolean(event.payload))
    })
      .then((dispose) => {
        if (this.isCurrent(generation)) this.stopHoverListening = dispose
        else dispose()
      })
      .catch(() => {})
  }

  private stop(): void {
    this.started = false
    this.generation += 1
    this.directionGeneration += 1
    this.clearHoverTimer()
    this.clearInterval("usagePoll")
    this.clearInterval("livenessPoll")
    this.clearInterval("resetClock")
    this.stopHoverListening?.()
    this.stopHoverListening = null
    this.removeDragListeners()
    this.hoverRequested = false
    delete document.body.dataset.transparentWindow
  }

  private isCurrent(generation: number): boolean {
    return this.started && this.generation === generation
  }

  private clearInterval(field: "usagePoll" | "livenessPoll" | "resetClock"): void {
    const identifier = this[field]
    if (identifier != null) window.clearInterval(identifier)
    this[field] = null
  }

  private clearHoverTimer(): void {
    if (this.hoverTimer != null) window.clearTimeout(this.hoverTimer)
    this.hoverTimer = null
  }

  private update(change: Partial<OverlaySnapshot>): void {
    this.snapshot = { ...this.snapshot, ...change }
    for (const listener of this.listeners) listener()
  }

  private commitLayout(change: Partial<OverlaySnapshot>): void {
    flushSync(() => this.update(change))
  }

  private expanded(): boolean {
    return this.snapshot.hovered && !this.snapshot.dragging
  }

  private connectPanel(): void {
    if (!this.panel) return
    void this.syncWindow(false, false)
  }

  private recordPanelHeight(height: number): void {
    const previous = this.snapshot.panelHeights
    if (this.expanded()) {
      if (height <= previous.expanded) return
      this.update({ panelHeights: { ...previous, expanded: height } })
      return
    }
    if (height !== previous.collapsed) {
      this.update({ panelHeights: { ...previous, collapsed: height } })
    }
  }

  private syncWindow(anchorBottom: boolean, animate: boolean): Promise<void> {
    if (!this.panel) return Promise.resolve()
    const height = Math.ceil(this.panel.getBoundingClientRect().height)
    this.recordPanelHeight(height)
    return resizeOverlayWindow(height, anchorBottom, animate && !prefersReducedMotion()).catch(
      () => {},
    )
  }

  private async resizeAfterDataChange(): Promise<void> {
    if (!this.started) return
    if (this.snapshot.dragging) {
      await this.syncWindow(false, false)
      return
    }
    if (!this.snapshot.hovered) {
      await this.syncWindow(this.snapshot.flipUp, true)
      return
    }

    const flipUp = await this.directionForExpansion()
    if (flipUp === "cancelled" || !this.started || !this.expanded()) return
    if (flipUp === "unavailable") {
      await this.syncWindow(this.snapshot.flipUp, true)
      return
    }
    if (flipUp !== this.snapshot.flipUp) this.update({ flipUp })
    await this.syncWindow(flipUp, true)
  }

  private async beginDrag(screenX: number, screenY: number): Promise<void> {
    if (this.snapshot.dragging) return
    this.directionGeneration += 1
    this.clearHoverTimer()
    const anchorBottom = this.snapshot.flipUp
    this.commitLayout({ dragging: true })
    this.dragOrigin = null
    this.addDragListeners()
    await this.syncWindow(anchorBottom, false)
    if (!this.started || !this.snapshot.dragging) return
    this.update({ flipUp: false })
    let monitor
    let position
    try {
      const result = await Promise.all([currentMonitor(), getCurrentWindow().outerPosition()])
      monitor = result[0]
      position = result[1]
    } catch {
      await this.settleDragCollapsed()
      return
    }
    if (!this.started || !this.snapshot.dragging) return
    const scale = monitor?.scaleFactor ?? 1
    this.dragOrigin = {
      pointerX: screenX,
      pointerY: screenY,
      windowX: position.x / scale,
      windowY: position.y / scale,
    }
  }

  private addDragListeners(): void {
    window.addEventListener("mousemove", this.moveDrag)
    window.addEventListener("mouseup", this.stopDrag, true)
    window.addEventListener("blur", this.stopDrag)
  }

  private removeDragListeners(): void {
    window.removeEventListener("mousemove", this.moveDrag)
    window.removeEventListener("mouseup", this.stopDrag, true)
    window.removeEventListener("blur", this.stopDrag)
    if (this.moveFrame) window.cancelAnimationFrame(this.moveFrame)
    this.moveFrame = 0
    this.pendingMove = null
  }

  private moveDrag = (event: MouseEvent): void => {
    this.pendingMove = event
    if (!this.moveFrame) this.moveFrame = window.requestAnimationFrame(this.applyDragMove)
  }

  private applyDragMove = (): void => {
    this.moveFrame = 0
    const origin = this.dragOrigin
    const event = this.pendingMove
    this.pendingMove = null
    if (!event || !origin) return
    void getCurrentWindow().setPosition(
      new LogicalPosition(
        origin.windowX + (event.screenX - origin.pointerX),
        origin.windowY + (event.screenY - origin.pointerY),
      ),
    )
  }

  private stopDrag = (): void => {
    if (!this.snapshot.dragging) return
    this.removeDragListeners()
    this.dragOrigin = null
    void this.finishDrag()
  }

  private async finishDrag(): Promise<void> {
    if (!this.started || !this.snapshot.dragging) return
    while (this.started && this.snapshot.dragging && this.hoverRequested) {
      const flipUp = await this.directionForExpansion()
      if (flipUp === "cancelled") continue
      if (flipUp === "unavailable") break
      if (!this.started || !this.snapshot.dragging) return
      if (!this.hoverRequested) continue
      this.commitLayout({ dragging: false, hovered: true, flipUp })
      await this.syncWindow(flipUp, true)
      return
    }

    await this.settleDragCollapsed()
  }

  private async settleDragCollapsed(): Promise<void> {
    if (!this.started || !this.snapshot.dragging) return
    this.removeDragListeners()
    this.dragOrigin = null
    this.commitLayout({ dragging: false, hovered: false, flipUp: false })
    await this.syncWindow(false, false)
  }

  private async expandAfterIntent(): Promise<void> {
    if (!this.started || this.snapshot.dragging || !this.hoverRequested) return
    const flipUp = await this.directionForExpansion()
    if (typeof flipUp !== "boolean") return
    if (!this.started || this.snapshot.dragging || !this.hoverRequested) return
    this.commitLayout({ hovered: true, flipUp })
    await this.syncWindow(flipUp, true)
  }

  private async directionForExpansion(): Promise<ExpansionDirection> {
    const directionGeneration = ++this.directionGeneration
    let monitor
    let position
    try {
      const result = await Promise.all([currentMonitor(), getCurrentWindow().outerPosition()])
      monitor = result[0]
      position = result[1]
    } catch {
      if (!this.started || directionGeneration !== this.directionGeneration) return "cancelled"
      return "unavailable"
    }
    if (!this.started || directionGeneration !== this.directionGeneration) return "cancelled"
    if (!monitor) return false
    const scale = monitor.scaleFactor
    const screenBottom = (monitor.position.y + monitor.size.height) / scale - SCREEN_MARGIN
    const panelTop = position.y / scale
    const needed = Math.max(
      this.snapshot.panelHeights.expanded,
      ESTIMATED_CHROME_PX + this.snapshot.bars.length * ESTIMATED_ROW_PX,
    )
    return panelTop + needed > screenBottom
  }
}
