import { LogicalPosition } from "@tauri-apps/api/dpi"
import { listen } from "@tauri-apps/api/event"
import { currentMonitor, getCurrentWindow } from "@tauri-apps/api/window"
import type { MouseEvent as ReactMouseEvent } from "react"
import { flushSync } from "react-dom"

import {
  getLatestSessionActivity,
  getLiveUsage,
  hideHudDetail,
  isOverlayWorkActive,
  onLiveUsageChanged,
  onSessionEntryChanged,
  onSessionsInvalidated,
  resizeOverlayWindow,
  SCAN_EVENTS,
  showHudDetail,
  type HudDetailState,
  type LiveUsageSummaryPayload,
} from "../../lib/ipc"
import {
  hideOverlayWindow,
  onOverlayWorkChanged,
  recordHudPosition,
  setFloatingHudEnabled,
  takeHudAnalyticsOrigin,
} from "../../lib/overlayWindow"
import { prefersReducedMotion } from "../../lib/popoverHeight"
import { liveDisplayableProviders, liveWindows } from "../../lib/presentation/liveUsage"
import { SurfaceExposureTracker } from "../../lib/surfaceExposure"
import { deriveUsageBars, noMeterSelected, type UsageBarItem } from "../../lib/usageBars"

const REFRESH_MS = 60_000
const LIVE_WINDOW_SECS = 90
const SHOW_DELAY_MS = 400
const MAX_TIMEOUT_MS = 2_147_483_647

export type OverlaySnapshot = {
  bars: UsageBarItem[]
  hovered: boolean
  dragging: boolean
  sessionLive: boolean
  /** True when `bars` is empty because every meter is turned off. */
  noMeterSelected: boolean
}

const INITIAL_SNAPSHOT: OverlaySnapshot = {
  bars: [],
  hovered: false,
  dragging: false,
  sessionLive: false,
  noMeterSelected: false,
}

type DragOrigin = {
  pointerX: number
  pointerY: number
  windowX: number
  windowY: number
}

function sameBars(left: UsageBarItem[], right: UsageBarItem[]): boolean {
  return (
    left.length === right.length &&
    left.every((bar, index) => {
      const other = right[index]
      return (
        other != null &&
        bar.key === other.key &&
        bar.label === other.label &&
        bar.percent === other.percent &&
        bar.resetsAt?.getTime() === other.resetsAt?.getTime() &&
        bar.color === other.color &&
        bar.expectedFraction === other.expectedFraction
      )
    })
  )
}

export class OverlaySession {
  private listeners = new Set<() => void>()
  private started = false
  private generation = 0
  private active = false
  private activityGeneration = 0
  private workRevision = 0
  private snapshot: OverlaySnapshot = INITIAL_SNAPSHOT
  private panel: HTMLDivElement | null = null
  private observer: ResizeObserver | null = null
  private showTimer: number | null = null
  private detailShown = false
  private usagePoll: number | null = null
  private livenessExpiry: number | null = null
  private latestActivity: number | null = null
  private livenessRevision = 0
  private stopWorkListening: (() => void) | null = null
  private stopHoverListening: (() => void) | null = null
  private stopUsageListening: (() => void) | null = null
  private stopSessionEntryListening: (() => void) | null = null
  private stopScanListening: (() => void) | null = null
  private stopInvalidationListening: (() => void) | null = null
  private stopVisibilityListening: (() => void) | null = null
  private stopDetailShownListening: (() => void) | null = null
  private dragOrigin: DragOrigin | null = null
  private pendingMove: MouseEvent | null = null
  private moveFrame = 0
  private readonly hudExposure = new SurfaceExposureTracker()
  private readonly detailExposure = new SurfaceExposureTracker()
  private hudExposureGeneration: number | null = null
  private hudOrigin: "user" | "automatic" | null = null
  private hudIdentity: string | null = null
  private hudVisibilityKnown = false
  private hudNativeVisible = false
  private detailRevision = 0
  private latestUsage: LiveUsageSummaryPayload | null = null
  private usageFailed = false

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
    if (this.active) this.connectPanel(this.activityGeneration)
  }

  requestHover = (hovered: boolean): void => {
    if (!this.active) return
    if (hovered) {
      if (!this.snapshot.hovered) this.update({ hovered: true })
      if (!this.snapshot.dragging) this.armShowTimer()
      return
    }

    if (this.snapshot.hovered) this.update({ hovered: false })
    this.clearShowTimer()
    this.hideDetail()
  }

  startDrag = (event: ReactMouseEvent): void => {
    if (
      !this.active ||
      (event.target as HTMLElement).closest("button") ||
      this.snapshot.dragging
    )
      return
    const { screenX, screenY } = event
    void this.beginDrag(screenX, screenY, this.activityGeneration)
  }

  close = (): void => {
    this.requestHover(false)
    setFloatingHudEnabled(false)
    void hideOverlayWindow().catch(() => {})
  }

  private start(): void {
    this.started = true
    const generation = ++this.generation
    document.body.dataset.transparentWindow = "true"
    void this.startWorkLifecycle(generation)
  }

  private async startWorkLifecycle(generation: number): Promise<void> {
    let dispose: (() => void) | null = null
    for (let attempt = 0; attempt < 2 && dispose == null; attempt += 1) {
      try {
        dispose = await onOverlayWorkChanged((active) => {
          if (!this.isLifecycleCurrent(generation)) return
          this.workRevision += 1
          this.setActive(active)
        })
      } catch {
        // The second attempt repairs a transient native listener failure.
      }
    }
    if (!this.isLifecycleCurrent(generation)) {
      dispose?.()
      return
    }
    this.stopWorkListening = dispose
    const revision = this.workRevision
    const active = await isOverlayWorkActive().catch(() => true)
    if (!this.isLifecycleCurrent(generation) || revision !== this.workRevision) return
    this.setActive(active)
  }

  private setActive(active: boolean): void {
    if (active === this.active) return
    if (active) this.startActivity()
    else {
      this.concealHudExposure()
      this.stopActivity()
    }
  }

  private startActivity(): void {
    this.active = true
    this.hudVisibilityKnown = false
    const generation = ++this.activityGeneration
    this.update({ hovered: false, dragging: false, sessionLive: false })
    this.connectPanel(generation)
    this.resumeHudExposure()

    const applyUsage = (response: LiveUsageSummaryPayload | null) => {
      if (!this.isCurrent(generation)) return
      this.latestUsage = response
      this.usageFailed = false
      const changed = this.commitLayout({
        bars: deriveUsageBars(response),
        noMeterSelected: noMeterSelected(response),
      })
      if (changed) void this.syncWindow(true, generation)
      if (changed && this.detailShown) {
        void showHudDetail(this.detailState("refresh")).catch(() => {})
      }
      this.observeHudUsage(response)
    }

    const refreshUsage = () => {
      void getLiveUsage()
        .then(applyUsage)
        .catch(() => {
          if (!this.isCurrent(generation)) return
          this.usageFailed = true
          if (this.hudExposureGeneration !== null) {
            this.hudExposure.observe("error", this.hudExposureGeneration)
          }
        })
    }
    refreshUsage()
    this.usagePoll = window.setInterval(refreshUsage, REFRESH_MS)

    // The poll is the slow floor. A meter switched off in settings must leave
    // the HUD at once, so the HUD also takes the summary the shell pushes.
    void onLiveUsageChanged(applyUsage)
      .then((dispose) => {
        if (this.isCurrent(generation)) this.stopUsageListening = dispose
        else dispose()
      })
      .catch(() => {})

    this.listenForActivity(generation)
    this.refreshLatestActivity(generation)

    void listen<boolean>("overlay_hover", (event) => {
      if (this.isCurrent(generation)) this.requestHover(Boolean(event.payload))
    })
      .then((dispose) => {
        if (this.isCurrent(generation)) this.stopHoverListening = dispose
        else dispose()
      })
      .catch(() => {})

    void listen<boolean>("overlay_visibility_changed", (event) => {
      if (!this.isCurrent(generation)) return
      this.hudVisibilityKnown = true
      this.hudNativeVisible = event.payload
      if (event.payload) void this.captureHudExposure(generation)
      else this.concealHudExposure()
    })
      .then((dispose) => {
        if (this.isCurrent(generation)) {
          this.stopVisibilityListening = dispose
          void this.captureHudExposure(generation)
        } else dispose()
      })
      .catch(() => {
        if (this.isCurrent(generation)) void this.captureHudExposure(generation)
      })

    void listen("hud-detail:shown", () => {
      if (this.isCurrent(generation)) this.recordDetailExposure()
    })
      .then((dispose) => {
        if (this.isCurrent(generation)) this.stopDetailShownListening = dispose
        else dispose()
      })
      .catch(() => {})
  }

  private stop(): void {
    this.started = false
    this.generation += 1
    this.stopActivity()
    this.stopWorkListening?.()
    this.stopWorkListening = null
    this.hudExposure.suspend()
    this.detailExposure.suspend()
    this.observer?.disconnect()
    this.observer = null
    delete document.body.dataset.transparentWindow
  }

  private stopActivity(): void {
    if (!this.active) return
    this.active = false
    this.activityGeneration += 1
    this.livenessRevision += 1
    this.clearShowTimer()
    this.hideDetail()
    this.clearUsagePoll()
    this.clearLivenessExpiry()
    this.stopHoverListening?.()
    this.stopHoverListening = null
    this.stopUsageListening?.()
    this.stopUsageListening = null
    this.stopSessionEntryListening?.()
    this.stopSessionEntryListening = null
    this.stopScanListening?.()
    this.stopScanListening = null
    this.stopInvalidationListening?.()
    this.stopInvalidationListening = null
    this.stopVisibilityListening?.()
    this.stopVisibilityListening = null
    this.stopDetailShownListening?.()
    this.stopDetailShownListening = null
    this.removeDragListeners()
    this.observer?.disconnect()
    this.observer = null
    this.dragOrigin = null
    this.pendingMove = null
    this.latestActivity = null
    this.update({ hovered: false, dragging: false, sessionLive: false })
  }

  private isCurrent(generation: number): boolean {
    return this.started && this.active && this.activityGeneration === generation
  }

  private isLifecycleCurrent(generation: number): boolean {
    return this.started && this.generation === generation
  }

  private clearUsagePoll(): void {
    if (this.usagePoll != null) window.clearInterval(this.usagePoll)
    this.usagePoll = null
  }

  private clearLivenessExpiry(): void {
    if (this.livenessExpiry != null) window.clearTimeout(this.livenessExpiry)
    this.livenessExpiry = null
  }

  private listenForActivity(generation: number): void {
    void onSessionEntryChanged((entry) => {
      if (!this.isCurrent(generation)) return
      const latest = Date.parse(entry.timestamp) / 1000
      if (!Number.isFinite(latest)) return
      this.livenessRevision += 1
      this.setLatestActivity(
        this.latestActivity == null ? latest : Math.max(this.latestActivity, latest),
        generation,
      )
    })
      .then((dispose) => {
        if (this.isCurrent(generation)) this.stopSessionEntryListening = dispose
        else dispose()
      })
      .catch(() => {})

    void listen(SCAN_EVENTS.finished, () => {
      if (this.isCurrent(generation)) this.refreshLatestActivity(generation)
    })
      .then((dispose) => {
        if (this.isCurrent(generation)) this.stopScanListening = dispose
        else dispose()
      })
      .catch(() => {})

    void onSessionsInvalidated(() => {
      if (this.isCurrent(generation)) this.refreshLatestActivity(generation)
    })
      .then((dispose) => {
        if (this.isCurrent(generation)) this.stopInvalidationListening = dispose
        else dispose()
      })
      .catch(() => {})
  }

  private refreshLatestActivity(generation: number): void {
    const revision = ++this.livenessRevision
    void getLatestSessionActivity()
      .then((latest) => {
        if (!this.isCurrent(generation) || revision !== this.livenessRevision) return
        this.setLatestActivity(latest, generation)
      })
      .catch(() => {})
  }

  private setLatestActivity(latest: number | null, generation: number): void {
    this.latestActivity = latest
    this.clearLivenessExpiry()
    if (latest == null) {
      this.update({ sessionLive: false })
      return
    }
    const expiresAt = latest * 1000 + LIVE_WINDOW_SECS * 1000
    const remaining = expiresAt - Date.now()
    if (remaining < 0) {
      this.update({ sessionLive: false })
      return
    }
    this.update({ sessionLive: true })
    this.livenessExpiry = window.setTimeout(
      () => {
        this.livenessExpiry = null
        if (!this.isCurrent(generation) || this.latestActivity !== latest) return
        if (Date.now() <= expiresAt) {
          this.setLatestActivity(latest, generation)
        } else {
          this.update({ sessionLive: false })
        }
      },
      Math.min(remaining + 1, MAX_TIMEOUT_MS),
    )
  }

  private armShowTimer(): void {
    if (this.showTimer != null || this.detailShown) return
    this.showTimer = window.setTimeout(() => {
      this.showTimer = null
      if (!this.active || !this.snapshot.hovered || this.snapshot.dragging) return
      this.detailShown = true
      const revision = ++this.detailRevision
      const state = this.detailState("show")
      void showHudDetail(state).catch(() => {
        if (revision === this.detailRevision) this.detailShown = false
      })
    }, SHOW_DELAY_MS)
  }

  private clearShowTimer(): void {
    if (this.showTimer != null) window.clearTimeout(this.showTimer)
    this.showTimer = null
  }

  private hideDetail(): void {
    if (!this.detailShown) return
    this.detailShown = false
    this.detailRevision += 1
    this.detailExposure.conceal("hud_detail")
    void hideHudDetail().catch(() => {})
  }

  private detailState(reason: HudDetailState["reason"]): HudDetailState {
    return {
      reason,
      now: Date.now(),
      noMeterSelected: this.snapshot.noMeterSelected,
      bars: this.snapshot.bars.map((bar) => ({
        key: bar.key,
        label: bar.label,
        percent: bar.percent,
        resetsAt: bar.resetsAt ? bar.resetsAt.toISOString() : null,
        color: bar.color,
        expectedFraction: bar.expectedFraction,
      })),
    }
  }

  private async captureHudExposure(generation: number): Promise<void> {
    const origin = await takeHudAnalyticsOrigin().catch(() => null)
    if (
      !origin ||
      !this.isCurrent(generation) ||
      (this.hudVisibilityKnown && !this.hudNativeVisible)
    )
      return
    this.hudOrigin = origin
    const state = this.latestUsage
      ? this.snapshot.bars.length > 0
        ? "ready"
        : "empty"
      : this.usageFailed
        ? "error"
        : null
    const identity = `${generation}:${this.detailRevision}`
    this.hudIdentity = identity
    this.hudExposureGeneration = this.hudExposure.expose({
      surface: "hud",
      origin,
      identity,
      ...(state ? { state } : {}),
    })
    if (this.latestUsage) this.observeHudUsage(this.latestUsage)
  }

  private observeHudUsage(response: LiveUsageSummaryPayload | null): void {
    const generation = this.hudExposureGeneration
    if (generation === null) return
    this.hudExposure.observe(this.snapshot.bars.length > 0 ? "ready" : "empty", generation)
    if (response && this.hudOrigin === "user") {
      for (const provider of liveDisplayableProviders(response)) {
        if (!liveWindows(provider).some((window) => window.usedPercent !== null)) continue
        this.hudExposure.observeLiveUsage(response, provider.provider, generation)
      }
    }
  }

  private recordDetailExposure(): void {
    if (!this.detailShown) return
    this.detailExposure.expose({
      surface: "hud_detail",
      origin: "user",
      identity: this.detailRevision,
      state: this.snapshot.bars.length > 0 ? "ready" : "empty",
    })
  }

  private concealHudExposure(): void {
    this.hudExposure.conceal("hud", this.hudExposureGeneration ?? undefined)
    this.hudExposureGeneration = null
    this.hudOrigin = null
    this.hudIdentity = null
    this.detailRevision += 1
    this.detailExposure.conceal("hud_detail")
  }

  private resumeHudExposure(): void {
    if (!this.hudOrigin || !this.hudIdentity || this.hudExposureGeneration === null) return
    this.hudExposureGeneration = this.hudExposure.expose({
      surface: "hud",
      origin: this.hudOrigin,
      identity: this.hudIdentity,
      ...(this.latestUsage
        ? { state: this.snapshot.bars.length > 0 ? ("ready" as const) : ("empty" as const) }
        : {}),
    })
  }

  private update(change: Partial<OverlaySnapshot>): boolean {
    const next = { ...this.snapshot, ...change }
    if (
      sameBars(this.snapshot.bars, next.bars) &&
      this.snapshot.hovered === next.hovered &&
      this.snapshot.dragging === next.dragging &&
      this.snapshot.sessionLive === next.sessionLive &&
      this.snapshot.noMeterSelected === next.noMeterSelected
    ) {
      return false
    }
    this.snapshot = next
    for (const listener of this.listeners) listener()
    return true
  }

  private commitLayout(change: Partial<OverlaySnapshot>): boolean {
    let changed = false
    flushSync(() => {
      changed = this.update(change)
    })
    return changed
  }

  private connectPanel(generation: number): void {
    if (!this.panel || !this.isCurrent(generation)) return
    void this.syncWindow(false, generation)
    if (typeof ResizeObserver === "undefined") return
    this.observer = new ResizeObserver(() => void this.syncWindow(true, generation))
    this.observer.observe(this.panel)
  }

  private syncWindow(animate: boolean, generation: number): Promise<void> {
    if (!this.panel || !this.isCurrent(generation)) return Promise.resolve()
    const height = Math.ceil(this.panel.getBoundingClientRect().height)
    return resizeOverlayWindow(height, false, animate && !prefersReducedMotion()).catch(
      () => {},
    )
  }

  private async beginDrag(screenX: number, screenY: number, generation: number): Promise<void> {
    this.clearShowTimer()
    this.hideDetail()
    this.update({ dragging: true })
    this.dragOrigin = null
    this.addDragListeners()
    await this.syncWindow(false, generation)
    if (!this.isCurrent(generation) || !this.snapshot.dragging) return

    let monitor
    let position
    try {
      const result = await Promise.all([currentMonitor(), getCurrentWindow().outerPosition()])
      monitor = result[0]
      position = result[1]
    } catch {
      if (this.isCurrent(generation)) this.settleDrag()
      return
    }
    if (!this.isCurrent(generation) || !this.snapshot.dragging) return
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
    this.settleDrag()
    // The drag is what makes this display the preferred one, so the record
    // happens here and not in `settleDrag`, which a failed drag start shares.
    void recordHudPosition().catch(() => {})
  }

  private settleDrag(): void {
    this.removeDragListeners()
    this.dragOrigin = null
    this.update({ dragging: false })
    if (this.snapshot.hovered) this.armShowTimer()
  }
}
