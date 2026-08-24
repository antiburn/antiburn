// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { invoke } from "@tauri-apps/api/core"
import { LogicalPosition } from "@tauri-apps/api/dpi"
import { listen } from "@tauri-apps/api/event"
import { currentMonitor, getCurrentWindow } from "@tauri-apps/api/window"
import type { MouseEvent as ReactMouseEvent } from "react"

import {
  getLatestSessionActivity,
  getLiveUsage,
  hideHudDetail,
  showHudDetail,
  type HudDetailState,
} from "../../lib/ipc"
import { setFloatingHudEnabled } from "../../lib/overlayWindow"
import { deriveUsageBars, type UsageBarItem } from "../../lib/usageBars"

const REFRESH_MS = 60_000
const LIVE_WINDOW_SECS = 90
const LIVENESS_POLL_MS = 5_000
const RESET_CLOCK_MS = 30_000
/** The tooltip delay before the detail window shows. */
const SHOW_DELAY_MS = 400

export type OverlaySnapshot = {
  bars: UsageBarItem[]
  hovered: boolean
  dragging: boolean
  now: number
  sessionLive: boolean
}

const INITIAL_SNAPSHOT: OverlaySnapshot = {
  bars: [],
  hovered: false,
  dragging: false,
  now: Date.now(),
  sessionLive: false,
}

type DragOrigin = {
  pointerX: number
  pointerY: number
  windowX: number
  windowY: number
}

export class OverlaySession {
  private listeners = new Set<() => void>()
  private started = false
  private generation = 0
  private snapshot: OverlaySnapshot = INITIAL_SNAPSHOT
  private panel: HTMLDivElement | null = null
  private observer: ResizeObserver | null = null
  private showTimer: number | null = null
  private detailShown = false
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
    this.observer?.disconnect()
    this.observer = null
    this.panel = panel
    if (this.started) this.connectPanel()
  }

  requestHover = (hovered: boolean): void => {
    if (hovered) {
      if (!this.snapshot.hovered) this.update({ hovered: true })
      // A drag suppresses the timer until mouse up.
      if (!this.snapshot.dragging) this.armShowTimer()
      return
    }

    if (this.snapshot.hovered) this.update({ hovered: false })
    this.clearShowTimer()
    this.hideDetail()
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

  private start(): void {
    this.started = true
    const generation = ++this.generation
    document.body.dataset.transparentWindow = "true"
    this.connectPanel()

    const refreshUsage = () => {
      void getLiveUsage()
        .then((response) => {
          if (!this.isCurrent(generation)) return
          this.update({ bars: deriveUsageBars(response) })
          // A visible detail window repaints with the fresh bars.
          if (this.detailShown) {
            void showHudDetail(this.detailState("refresh")).catch(() => {})
          }
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
    this.clearShowTimer()
    this.hideDetail()
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

  private armShowTimer(): void {
    if (this.showTimer != null || this.detailShown) return
    this.showTimer = window.setTimeout(() => {
      this.showTimer = null
      if (!this.started || !this.snapshot.hovered || this.snapshot.dragging) return
      this.detailShown = true
      void showHudDetail(this.detailState("show")).catch(() => {})
    }, SHOW_DELAY_MS)
  }

  private clearShowTimer(): void {
    if (this.showTimer != null) window.clearTimeout(this.showTimer)
    this.showTimer = null
  }

  private hideDetail(): void {
    if (!this.detailShown) return
    this.detailShown = false
    void hideHudDetail().catch(() => {})
  }

  private detailState(reason: HudDetailState["reason"]): HudDetailState {
    return {
      reason,
      now: Date.now(),
      bars: this.snapshot.bars.map((bar) => ({
        key: bar.key,
        label: bar.label,
        percent: bar.percent,
        resetsAt: bar.resetsAt ? bar.resetsAt.toISOString() : null,
        color: bar.color,
      })),
    }
  }

  private update(change: Partial<OverlaySnapshot>): void {
    this.snapshot = { ...this.snapshot, ...change }
    for (const listener of this.listeners) listener()
  }

  private connectPanel(): void {
    if (!this.panel) return
    this.reportHoverRegion()
    if (typeof ResizeObserver === "undefined") return
    // The panel height moves with the bar count and fonts. The shell needs
    // the fresh edges for its cursor watcher and for detail placement.
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

  private async beginDrag(screenX: number, screenY: number): Promise<void> {
    // A drag cancels the show timer and hides the detail window at once.
    this.clearShowTimer()
    this.hideDetail()
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
    this.update({ dragging: true })
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
    this.update({ dragging: false })
    this.reportHoverRegion()
    // The timer restarts from zero when the pointer is still on the HUD.
    if (this.snapshot.hovered) this.armShowTimer()
  }
}
