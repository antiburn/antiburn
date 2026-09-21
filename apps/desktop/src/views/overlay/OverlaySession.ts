import { LogicalPosition } from "@tauri-apps/api/dpi"
import { listen } from "@tauri-apps/api/event"
import { currentMonitor, getCurrentWindow } from "@tauri-apps/api/window"
import type { MouseEvent as ReactMouseEvent } from "react"
import { flushSync } from "react-dom"

import {
  getLiveUsage,
  hideHudDetail,
  isOverlayWorkActive,
  onLiveUsageChanged,
  refreshLiveUsage,
  resizeOverlayWindow,
  type LiveUsageSummaryPayload,
} from "../../lib/ipc"
import { hasWorkingActivity, liveSessions } from "../../lib/sessionLifecycle"
import {
  getHudTokenMap,
  showHudDetail,
  type HudDetailState,
  type HudSpendRate,
} from "../../lib/hudIpc"
import { devSpendRate, withDevBlock, type HudDevOverride } from "../../lib/hudDev"
import {
  getHudIslandState,
  HUD_ISLAND_OFF,
  islandSpendFigure,
  onHudIslandState,
  type HudIslandState,
} from "../../lib/hudIsland"
import { BurnWakeTracker, activityWake } from "../../lib/hudWake"
import {
  hideOverlayWindow,
  isHudTokenMapEnabled,
  onOverlayWorkChanged,
  recordHudPosition,
  setFloatingHudEnabled,
  takeHudAnalyticsOrigin,
  tearOffOverlayWindow,
  wakeOverlayWindow,
} from "../../lib/overlayWindow"
import { prefersReducedMotion } from "../../lib/popoverHeight"
import { liveDisplayableProviders, liveWindows } from "../../lib/presentation/liveUsage"
import {
  liveModels,
  liveProviders,
  sameProviderModels,
  type ProviderModels,
} from "../../lib/sessionLiveness"
import { SurfaceExposureTracker } from "../../lib/surfaceExposure"
import { blinkPeriod, describeSpend } from "../../lib/ledPeriod"
import {
  agentCount,
  deriveTokenMap,
  frameColor,
  mapVisible,
  type TokenMapLayout,
} from "../../lib/tokenMap"
// import { playPop } from "../../lib/hudSounds"
import {
  blockedBars,
  deriveUsageBars,
  limitsReset,
  noMeterSelected,
  resetDue,
  type UsageBarItem,
} from "../../lib/usageBars"

const REFRESH_MS = 60_000
const SHOW_DELAY_MS = 400
const TOKEN_MAP_WINDOW_SECS = 300
const TOKEN_MAP_POLL_MS = 5_000
/** How long the reset message and its confetti stay. */
const CELEBRATION_MS = 6_000

const EMPTY_TOKEN_MAP = deriveTokenMap(null)

export type OverlaySnapshot = {
  bars: UsageBarItem[]
  hovered: boolean
  dragging: boolean
  /** Whether any session is live, from the shell's lifecycle bus. */
  sessionLive: boolean
  /** The providers a live session draws on, sorted. Their bars blink. */
  liveProviders: readonly string[]
  /** The models a live session runs, sorted. A model-scoped bar reads this. */
  liveModels: ProviderModels
  /** True when `bars` is empty because every meter is turned off. */
  noMeterSelected: boolean
  tokenMap: TokenMapLayout
  /** True while two or more sessions are live, so the map draws above the bars. */
  showMap: boolean
  /** Milliseconds per blink of the live LED. */
  blinkPeriodMs: number
  /** The spend rate in words, or null when the window carried no tokens. */
  spend: string | null
  /** The spend rate as a figure for the island's wing, or null. */
  spendFigure: string | null
  /** Where the HUD sits in the notch, if it does, and the notch's shape. */
  island: HudIslandState
  /** The clock the countdown to a reset reads from. */
  now: number
  /** The reset message under the bars, or null. */
  celebration: string | null
}

const INITIAL_SNAPSHOT: OverlaySnapshot = {
  bars: [],
  hovered: false,
  dragging: false,
  sessionLive: false,
  liveProviders: [],
  liveModels: {},
  noMeterSelected: false,
  tokenMap: EMPTY_TOKEN_MAP,
  showMap: false,
  blinkPeriodMs: blinkPeriod(null, null).periodMs,
  spend: null,
  spendFigure: null,
  island: HUD_ISLAND_OFF,
  now: 0,
  celebration: null,
}

type DragOrigin = {
  pointerX: number
  pointerY: number
  windowX: number
  windowY: number
}

function sameList(left: readonly string[], right: readonly string[]): boolean {
  return left.length === right.length && left.every((item, index) => item === right[index])
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
  /** The agent box under the pointer, by blob key, or null over the meter. */
  private hoverBlob: string | null = null
  /** The sub-agent whose dot is under the pointer, or null. */
  private hoverSubagent: string | null = null
  /** Whether the map showed before the last poll, for its show hysteresis. */
  private previousShowMap = false
  private usagePoll: number | null = null
  private tokenMapPoll: number | null = null
  /** The dot value the map last used, held for a window so a burst does not flicker the scale. */
  private dotValueFloor = 0
  private dotValueFloorSince = 0
  private stopWorkListening: (() => void) | null = null
  private stopHoverListening: (() => void) | null = null
  private stopUsageListening: (() => void) | null = null
  private stopLifecycleListening: (() => void) | null = null
  private stopVisibilityListening: (() => void) | null = null
  private stopDetailShownListening: (() => void) | null = null
  private stopDevListening: (() => void) | null = null
  private stopIslandListening: (() => void) | null = null
  /** A spend rate the "HUD Dev" menu pinned, in place of the measured one. */
  private devSpend: HudSpendRate | null = null
  /** Until when the "HUD Dev" menu holds the first bar at its limit. */
  private devBlockUntil = 0
  private devBlockTimer = 0
  /** When work last ran, in epoch seconds, for the quiet-spell wake. */
  private lastEventActivity: number | null = null
  private burnWake = new BurnWakeTracker()
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
  private celebrationTimer = 0
  /** The reset time a fresh read was already asked for, so it is asked once. */
  private resetAskedFor = 0
  private latestSpend: HudSpendRate | null = null
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

  /** Note the agent box under the pointer. An open detail follows at once. */
  setHoverBlob = (key: string | null, subagentId: string | null = null): void => {
    if (this.hoverBlob === key && this.hoverSubagent === subagentId) return
    this.hoverBlob = key
    this.hoverSubagent = subagentId
    if (!this.detailShown) return
    this.detailRevision += 1
    void showHudDetail(this.detailState("show")).catch(() => {})
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
    this.update({
      hovered: false,
      dragging: false,
      sessionLive: false,
      liveProviders: [],
      liveModels: {},
      celebration: null,
    })
    this.connectPanel(generation)
    this.resumeHudExposure()

    const applyUsage = (response: LiveUsageSummaryPayload | null) => {
      if (!this.isCurrent(generation)) return
      this.latestUsage = response
      this.usageFailed = false
      const bars = withDevBlock(deriveUsageBars(response), this.devBlockUntil, Date.now())
      const freed = limitsReset(this.snapshot.bars, bars)
      if (freed.length > 0) this.celebrate(freed[0]!.providerName)
      this.update({ now: Date.now() })
      const changed = this.commitLayout({
        bars,
        noMeterSelected: noMeterSelected(response),
        blinkPeriodMs: blinkPeriod(this.latestSpend, response).periodMs,
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

    // Liveness comes from the lifecycle registry alone: the tracker takes
    // the versioned snapshot behind its own listener and applies only
    // newer deltas, so this surface never derives windows from row
    // timestamps or scan events.
    const syncLiveness = () => {
      if (!this.isCurrent(generation)) return
      const live = liveSessions.getSnapshot()
      const working = hasWorkingActivity(live)
      // A write after a quiet spell wakes a docked HUD. The registry reports
      // the work, so the wake reads the gap since work last ran.
      const nowSecs = Date.now() / 1000
      if (working && activityWake(this.lastEventActivity, nowSecs)) {
        void wakeOverlayWindow("activity").catch(() => {})
      }
      if (working) this.lastEventActivity = nowSecs
      this.update({
        sessionLive: working,
        liveProviders: liveProviders(live),
        liveModels: liveModels(live),
      })
    }
    this.stopLifecycleListening = liveSessions.subscribe(syncLiveness)
    syncLiveness()

    const refreshTokenMap = () => {
      this.tickCountdown(applyUsage)
      if (!isHudTokenMapEnabled()) {
        this.latestSpend = null
        this.previousShowMap = false
        const hadMap = this.snapshot.showMap
        this.commitLayout({
          tokenMap: EMPTY_TOKEN_MAP,
          showMap: false,
          blinkPeriodMs: blinkPeriod(null, this.latestUsage).periodMs,
          spend: null,
          spendFigure: null,
        })
        if (hadMap) void this.syncWindow(true, generation)
        // The detail window spells the map out. It needs the empty state too.
        if (hadMap && this.detailShown) {
          void showHudDetail(this.detailState("refresh")).catch(() => {})
        }
        return
      }
      void getHudTokenMap(TOKEN_MAP_WINDOW_SECS)
        .then((payload) => {
          if (!this.isCurrent(generation)) return
          const tokenMap = deriveTokenMap(payload, { minDotValue: this.dotValueFloor })
          this.holdDotValue(tokenMap.dotValue, payload?.windowSecs ?? TOKEN_MAP_WINDOW_SECS)
          this.latestSpend = this.devSpend ?? payload?.spend ?? null
          if (this.burnWake.observe(this.latestSpend?.usdPerMinute ?? null)) {
            void wakeOverlayWindow("burn").catch(() => {})
          }
          const hadMap = this.snapshot.showMap
          const showMap = mapVisible(this.previousShowMap, hadMap, agentCount(tokenMap))
          this.previousShowMap = hadMap
          this.commitLayout({
            tokenMap,
            showMap,
            blinkPeriodMs: blinkPeriod(this.latestSpend, this.latestUsage).periodMs,
            spend: describeSpend(this.latestSpend),
            spendFigure: islandSpendFigure(this.latestSpend),
          })
          if (hadMap !== showMap) void this.syncWindow(true, generation)
          if (this.detailShown) {
            void showHudDetail(this.detailState("refresh")).catch(() => {})
          }
        })
        .catch(() => {})
    }
    refreshTokenMap()
    this.tokenMapPoll = window.setInterval(refreshTokenMap, TOKEN_MAP_POLL_MS)

    if (import.meta.env.DEV) {
      void listen<HudDevOverride>("hud_dev", (event) => {
        if (this.isCurrent(generation)) this.applyDevOverride(event.payload, applyUsage)
      })
        .then((dispose) => {
          if (this.isCurrent(generation)) this.stopDevListening = dispose
          else dispose()
        })
        .catch(() => {})
    }

    // The shell owns the island. The HUD asks once, then follows its events.
    void getHudIslandState()
      .then((island) => {
        if (this.isCurrent(generation)) this.applyIsland(island)
      })
      .catch(() => {})
    void onHudIslandState((island) => {
      if (this.isCurrent(generation)) this.applyIsland(island)
    })
      .then((dispose) => {
        if (this.isCurrent(generation)) this.stopIslandListening = dispose
        else dispose()
      })
      .catch(() => {})

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
    this.clearShowTimer()
    this.hideDetail()
    this.clearUsagePoll()
    this.clearTokenMapPoll()
    this.stopHoverListening?.()
    this.stopHoverListening = null
    this.stopUsageListening?.()
    this.stopUsageListening = null
    this.stopLifecycleListening?.()
    this.stopLifecycleListening = null
    this.stopVisibilityListening?.()
    this.stopVisibilityListening = null
    this.stopDetailShownListening?.()
    this.stopDetailShownListening = null
    this.stopDevListening?.()
    this.stopDevListening = null
    this.stopIslandListening?.()
    this.stopIslandListening = null
    this.lastEventActivity = null
    this.burnWake = new BurnWakeTracker()
    window.clearTimeout(this.celebrationTimer)
    this.celebrationTimer = 0
    window.clearTimeout(this.devBlockTimer)
    this.devBlockTimer = 0
    this.devBlockUntil = 0
    this.resetAskedFor = 0
    this.removeDragListeners()
    this.observer?.disconnect()
    this.observer = null
    this.dragOrigin = null
    this.pendingMove = null
    this.update({
      hovered: false,
      dragging: false,
      sessionLive: false,
      liveProviders: [],
      liveModels: {},
      celebration: null,
      island: HUD_ISLAND_OFF,
    })
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

  private clearTokenMapPoll(): void {
    if (this.tokenMapPoll != null) window.clearInterval(this.tokenMapPoll)
    this.tokenMapPoll = null
  }

  /**
   * Keep the scale from stepping down until a full window has passed. A step
   * up applies at once, so the square never overflows.
   */
  private holdDotValue(dotValue: number, windowSecs: number): void {
    const now = Date.now()
    if (dotValue > this.dotValueFloor) {
      this.dotValueFloor = dotValue
      this.dotValueFloorSince = now
      return
    }
    if (now - this.dotValueFloorSince >= windowSecs * 1000) {
      this.dotValueFloor = 0
      this.dotValueFloorSince = now
    }
  }

  /**
   * Take the island's new shape. A collapse hides the detail: the notch row
   * has nothing to explain. An expansion under the pointer shows it.
   */
  private applyIsland(island: HudIslandState): void {
    const wasCollapsed = this.snapshot.island.island === "collapsed"
    this.update({ island })
    if (island.island === "collapsed") {
      this.clearShowTimer()
      this.hideDetail()
    } else if (wasCollapsed && this.snapshot.hovered && !this.snapshot.dragging) {
      this.armShowTimer()
    }
  }

  private armShowTimer(): void {
    if (this.showTimer != null || this.detailShown) return
    // The collapsed island is the notch row alone. It has no detail.
    if (this.snapshot.island.island === "collapsed") return
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
      map: this.detailMap(),
      spend: this.snapshot.spend,
      target: this.detailTarget(),
      subagent: this.hoverSubagent,
      island: this.snapshot.island.island !== "off",
    }
  }

  /** The hovered box when it is still on the map, else the usage meter. */
  private detailTarget(): string {
    const key = this.hoverBlob
    if (key != null && this.snapshot.tokenMap.blobs.some((blob) => blob.key === key)) {
      return key
    }
    return "usage"
  }

  private detailMap(): HudDetailState["map"] {
    const { tokenMap } = this.snapshot
    if (tokenMap.dots.length === 0) return null
    return {
      dotValue: tokenMap.dotValue,
      sessions: tokenMap.blobs.map((blob, index) => ({
        key: blob.key,
        label: blob.title ?? blob.agent,
        agent: blob.agent,
        tokensPerMin: blob.tokensPerMin,
        topMode: blob.topMode,
        frameColor: frameColor(index),
        modes: blob.modes,
        subagents: blob.subagents,
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
        if (
          !liveWindows(provider, { includeIdleModelLimits: true }).some(
            (window) => window.usedPercent !== null,
          )
        )
          continue
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

  /**
   * Keep the countdown current while a limit blocks a tool. Once the reset
   * time passes, ask the shell for a fresh read: the cached summary can lag
   * the reset by minutes, and the HUD should notice on its own.
   */
  private tickCountdown(apply: (response: LiveUsageSummaryPayload | null) => void): void {
    const blocked = blockedBars(this.snapshot.bars)
    if (blocked.length === 0) return
    const now = Date.now()
    this.update({ now })
    const due = blocked.find((bar) => bar.resetsAt != null && bar.resetsAt.getTime() <= now)
    if (!due || !resetDue(this.snapshot.bars, now)) return
    const at = due.resetsAt!.getTime()
    if (this.resetAskedFor === at) return
    this.resetAskedFor = at
    void refreshLiveUsage()
      .then(apply)
      .catch(() => {})
  }

  /** Apply one override from the tray's "HUD Dev" menu. Debug builds only. */
  private applyDevOverride(
    override: HudDevOverride,
    apply: (response: LiveUsageSummaryPayload | null) => void,
  ): void {
    switch (override.kind) {
      case "spend": {
        this.devSpend = devSpendRate(override.usdPerMinute, TOKEN_MAP_WINDOW_SECS)
        this.latestSpend = this.devSpend ?? this.latestSpend
        this.commitLayout({
          blinkPeriodMs: blinkPeriod(this.latestSpend, this.latestUsage).periodMs,
          spend: describeSpend(this.latestSpend),
          spendFigure: islandSpendFigure(this.latestSpend),
        })
        return
      }
      case "block": {
        window.clearTimeout(this.devBlockTimer)
        this.devBlockUntil = Date.now() + override.secs * 1_000
        apply(this.latestUsage)
        // The usage poll runs once a minute. This timer ends a shorter block
        // at the time the menu asked for.
        this.devBlockTimer = window.setTimeout(() => {
          this.devBlockTimer = 0
          apply(this.latestUsage)
        }, override.secs * 1_000)
        return
      }
      case "celebrate":
        this.celebrate(this.snapshot.bars[0]?.providerName ?? "claude")
    }
  }

  /** Show the reset message with confetti, and peek a docked HUD in. */
  private celebrate(providerName: string): void {
    window.clearTimeout(this.celebrationTimer)
    this.update({ celebration: `${providerName.toLowerCase()} usage reset` })
    void wakeOverlayWindow("reset").catch(() => {})
    this.celebrationTimer = window.setTimeout(() => {
      this.celebrationTimer = 0
      this.update({ celebration: null })
    }, CELEBRATION_MS)
  }

  private update(change: Partial<OverlaySnapshot>): boolean {
    const next = { ...this.snapshot, ...change }
    if (
      sameBars(this.snapshot.bars, next.bars) &&
      this.snapshot.hovered === next.hovered &&
      this.snapshot.dragging === next.dragging &&
      this.snapshot.sessionLive === next.sessionLive &&
      sameList(this.snapshot.liveProviders, next.liveProviders) &&
      sameProviderModels(this.snapshot.liveModels, next.liveModels) &&
      this.snapshot.noMeterSelected === next.noMeterSelected &&
      this.snapshot.tokenMap === next.tokenMap &&
      this.snapshot.showMap === next.showMap &&
      this.snapshot.blinkPeriodMs === next.blinkPeriodMs &&
      this.snapshot.spend === next.spend &&
      this.snapshot.spendFigure === next.spendFigure &&
      this.snapshot.island === next.island &&
      this.snapshot.now === next.now &&
      this.snapshot.celebration === next.celebration
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
    // A drag on a docked HUD tears it off. The drop decides whether it docks
    // again, in `recordHudPosition`. The tear pop is off for now.
    void tearOffOverlayWindow()
      // .then((torn) => {
      //   if (torn) playPop()
      // })
      .catch(() => {})
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
