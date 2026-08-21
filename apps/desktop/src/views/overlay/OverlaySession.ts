// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { invoke } from "@tauri-apps/api/core"
import { LogicalPosition } from "@tauri-apps/api/dpi"
import { listen } from "@tauri-apps/api/event"
import { currentMonitor, getCurrentWindow } from "@tauri-apps/api/window"
import type { MouseEvent as ReactMouseEvent } from "react"
import { flushSync } from "react-dom"

import { getLatestSessionActivity, getLiveUsage, openSettingsWindow } from "../../lib/ipc"
import { setFloatingHudEnabled } from "../../lib/overlayWindow"
import { deriveUsageBars, type UsageBarItem } from "../../lib/usageBars"

const REFRESH_MS = 60_000
const LIVE_WINDOW_SECS = 90
const LIVENESS_POLL_MS = 5_000
const RESET_CLOCK_MS = 30_000
const SCREEN_MARGIN = 8
const RESERVE_ABOVE = 220
const ESTIMATED_CHROME_PX = 48
const ESTIMATED_ROW_PX = 50
const HOVER_INTENT_MS = 250

export type OverlaySnapshot = {
  bars: UsageBarItem[]
  hovered: boolean
  dragging: boolean
  now: number
  sessionLive: boolean
  reserveAbove: number
  swapping: boolean
  flipUp: boolean
  panelHeights: {
    collapsed: number
    expanded: number
  }
}

const INITIAL_SNAPSHOT: OverlaySnapshot = {
  bars: [],
  hovered: false,
  dragging: false,
  now: Date.now(),
  sessionLive: false,
  reserveAbove: 0,
  swapping: false,
  flipUp: false,
  panelHeights: { collapsed: 0, expanded: 0 },
}

type DragOrigin = {
  pointerX: number
  pointerY: number
  windowX: number
  windowY: number
}

/** Own the external systems used by the floating HUD window. */
export class OverlaySession {
  private listeners = new Set<() => void>()
  private started = false
  private generation = 0
  private directionGeneration = 0
  private snapshot: OverlaySnapshot = INITIAL_SNAPSHOT
  private panel: HTMLDivElement | null = null
  private observer: ResizeObserver | null = null
  private hoverTimer: number | null = null
  private usagePoll: number | null = null
  private livenessPoll: number | null = null
  private resetClock: number | null = null
  private stopHoverListening: (() => void) | null = null
  private reserve = 0
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
    this.observer?.disconnect()
    this.observer = null
    this.panel = panel
    if (this.started) this.connectPanel()
  }

  requestHover = (hovered: boolean): void => {
    if (hovered) {
      if (this.hoverTimer != null || this.snapshot.hovered) return
      this.hoverTimer = window.setTimeout(() => {
        this.hoverTimer = null
        if (!this.started || this.snapshot.hovered) return
        this.commitLayout({ hovered: true })
        this.measurePanel()
        void this.decideDirection()
      }, HOVER_INTENT_MS)
      return
    }

    this.clearHoverTimer()
    if (!this.snapshot.hovered) return
    this.commitLayout({ hovered: false })
    this.measurePanel()
    this.reportHoverRegion()
  }

  startDrag = (event: ReactMouseEvent): void => {
    if ((event.target as HTMLElement).closest("button")) return
    const { screenX, screenY } = event
    void this.beginDrag(screenX, screenY)
  }

  close = (): void => {
    setFloatingHudEnabled(false)
    void getCurrentWindow().hide()
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
          this.measurePanel()
          void this.decideDirection()
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
    this.observer?.disconnect()
    this.observer = null
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
    this.reportHoverRegion()
    this.measurePanel()
    if (typeof ResizeObserver === "undefined") return
    this.observer = new ResizeObserver(() => this.reportHoverRegion())
    this.observer.observe(this.panel)
  }

  private reportHoverRegion = (): void => {
    if (!this.panel || this.snapshot.dragging) return
    const rect = this.panel.getBoundingClientRect()
    void invoke("set_overlay_hover_region", { top: rect.top, bottom: rect.bottom }).catch(
      () => {},
    )
  }

  private measurePanel(): void {
    if (!this.panel) return
    const height = this.panel.getBoundingClientRect().height
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

  private async beginDrag(screenX: number, screenY: number): Promise<void> {
    await this.applyReserve(0)
    if (!this.started) return
    const [monitor, position] = await Promise.all([
      currentMonitor(),
      getCurrentWindow().outerPosition(),
    ])
    if (!this.started) return
    const scale = monitor?.scaleFactor ?? 1
    this.dragOrigin = {
      pointerX: screenX,
      pointerY: screenY,
      windowX: position.x / scale,
      windowY: position.y / scale,
    }
    this.commitLayout({ dragging: true })
    this.addDragListeners()
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
    if (!event || !origin) return
    this.pendingMove = null
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
    this.commitLayout({ dragging: false })
    this.measurePanel()
    this.reportHoverRegion()
    void this.decideDirection()
  }

  private async decideDirection(): Promise<void> {
    if (!this.started || this.snapshot.dragging) return
    const directionGeneration = ++this.directionGeneration
    const [monitor, position] = await Promise.all([
      currentMonitor(),
      getCurrentWindow().outerPosition(),
    ])
    if (
      !this.started ||
      directionGeneration !== this.directionGeneration ||
      this.snapshot.dragging ||
      !monitor
    ) {
      return
    }
    const scale = monitor.scaleFactor
    const screenBottom = (monitor.position.y + monitor.size.height) / scale - SCREEN_MARGIN
    const panelTop = position.y / scale + this.reserve
    const needed = Math.max(
      this.snapshot.panelHeights.expanded,
      ESTIMATED_CHROME_PX + this.snapshot.bars.length * ESTIMATED_ROW_PX,
    )
    const flipUp = panelTop + needed > screenBottom
    this.update({ flipUp })
    await this.applyReserve(flipUp ? RESERVE_ABOVE : 0)
  }

  private async applyReserve(desired: number): Promise<void> {
    const current = this.reserve
    if (desired === current) return
    this.reserve = desired
    this.commitLayout({ swapping: true })
    try {
      await new Promise<number>((resolve) => window.requestAnimationFrame(resolve))
      const overlay = getCurrentWindow()
      const [monitor, position] = await Promise.all([currentMonitor(), overlay.outerPosition()])
      const scale = monitor?.scaleFactor ?? 1
      await overlay.setPosition(
        new LogicalPosition(position.x / scale, position.y / scale + current - desired),
      )
    } finally {
      this.commitLayout({ reserveAbove: desired, swapping: false })
      this.reportHoverRegion()
    }
  }
}
