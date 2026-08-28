import { listen } from "@tauri-apps/api/event"
import { useCallback, useState, useSyncExternalStore } from "react"
import { flushSync } from "react-dom"

import { LedBar } from "../../components/ui/LedBar"
import {
  concealHudDetail,
  getHudDetailState,
  setHudDetailSize,
  type HudDetailState,
} from "../../lib/ipc"
import { resetsIn } from "../../lib/usageBars"

const HUD_SEGMENTS = 20

type DetailSnapshot = {
  bars: HudDetailState["bars"]
  now: number
  /** True when `bars` is empty because every meter is turned off. */
  noMeterSelected: boolean
  /** Counts the show requests, so each show restarts the enter animation. */
  shown: number
  /** True after a conceal request: the card is gone until the next payload. */
  concealed: boolean
}

const INITIAL_SNAPSHOT: DetailSnapshot = {
  bars: [],
  now: 0,
  shown: 0,
  concealed: false,
  noMeterSelected: false,
}

function resetDate(resetsAt: string | null): Date | null {
  if (!resetsAt) return null
  const date = new Date(resetsAt)
  return Number.isNaN(date.getTime()) ? null : date
}

/**
 * Own the external systems used by the hover detail window.
 *
 * The HUD session owns the data and pushes it here: a `hud-detail:state`
 * event on every show and refresh, plus one fetch at mount for the payload
 * that fired before this webview existed. After each render the session
 * reports the measured height, and the shell sizes, places, and shows the
 * window — the webview never touches its own window.
 */
class HudDetailSession {
  private listeners = new Set<() => void>()
  private started = false
  private generation = 0
  private snapshot: DetailSnapshot = INITIAL_SNAPSHOT
  private wrap: HTMLDivElement | null = null
  private disposers: Array<() => void> = []

  getSnapshot = (): DetailSnapshot => this.snapshot

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener)
    if (!this.started) this.start()
    return () => {
      this.listeners.delete(listener)
      if (this.listeners.size === 0) this.stop()
    }
  }

  registerWrap = (wrap: HTMLDivElement | null): void => {
    this.wrap = wrap
    this.reportSize()
  }

  private start(): void {
    this.started = true
    const generation = ++this.generation
    document.body.dataset.transparentWindow = "true"

    void getHudDetailState()
      .then((state) => {
        if (state && this.isCurrent(generation)) this.apply(state)
      })
      .catch(() => {})

    this.subscribeShell(generation, "hud-detail:state", (state: HudDetailState) =>
      this.apply(state),
    )
    this.subscribeShell(generation, "hud-detail:conceal", () => this.conceal())
  }

  private subscribeShell<Payload>(
    generation: number,
    event: string,
    handle: (payload: Payload) => void,
  ): void {
    void listen<Payload>(event, (received) => {
      if (this.isCurrent(generation)) handle(received.payload)
    })
      .then((dispose) => {
        if (this.isCurrent(generation)) this.disposers.push(dispose)
        else dispose()
      })
      .catch(() => {})
  }

  private stop(): void {
    this.started = false
    this.generation += 1
    for (const dispose of this.disposers) dispose()
    this.disposers = []
    delete document.body.dataset.transparentWindow
  }

  private isCurrent(generation: number): boolean {
    return this.started && this.generation === generation
  }

  private apply(state: HudDetailState): void {
    const shown = state.reason === "show" ? this.snapshot.shown + 1 : this.snapshot.shown
    // flushSync, so the measurement below reads the fresh layout.
    flushSync(() => {
      this.snapshot = {
        bars: state.bars,
        now: state.now,
        shown,
        concealed: false,
        noMeterSelected: state.noMeterSelected,
      }
      for (const listener of this.listeners) listener()
    })
    this.reportSize()
  }

  /**
   * Clear the card while the window can still paint, then report back.
   *
   * A hidden webview keeps its last frame, and macOS flashes that frame on
   * the next show. The two-frame wait lets the cleared card reach the screen
   * before the shell hides the window.
   */
  private conceal(): void {
    if (this.snapshot.concealed) return
    flushSync(() => {
      this.snapshot = { ...this.snapshot, concealed: true }
      for (const listener of this.listeners) listener()
    })
    window.requestAnimationFrame(() => {
      window.requestAnimationFrame(() => {
        void concealHudDetail().catch(() => {})
      })
    })
  }

  private reportSize(): void {
    // Before the first payload the card holds placeholder content. Reporting
    // that height would show the window at the wrong size for one frame.
    if (!this.wrap || this.snapshot.shown === 0 || this.snapshot.concealed) return
    const height = this.wrap.getBoundingClientRect().height
    if (height > 0) void setHudDetailSize(height).catch(() => {})
  }
}

/** Render the hover detail window: the HUD's stats, spelled out. */
export function HudDetailView() {
  const [session] = useState(() => new HudDetailSession())
  const state = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
    session.getSnapshot,
  )
  const wrapRef = useCallback(
    (node: HTMLDivElement | null) => session.registerWrap(node),
    [session],
  )

  if (state.concealed) {
    return <div ref={wrapRef} className="p-2" />
  }

  return (
    <div ref={wrapRef} className="p-2">
      <div
        key={state.shown}
        className="hud-detail-card hud-detail-in bevel select-none rounded-xl border border-separator px-3 pt-2 pb-3"
        style={{ backgroundColor: "var(--color-bg-hud)" }}
      >
        <p className="font-bitcount text-[11px] text-label-tertiary lowercase mb-1.5">
          antiburn
        </p>
        {state.bars.length === 0 ? (
          <p className="type-caption text-label-tertiary">
            {state.noMeterSelected ? "No meter selected." : "No usage limits detected yet."}
          </p>
        ) : (
          <div className="space-y-2">
            {state.bars.map((bar) => (
              <div key={bar.key}>
                <div className="flex items-baseline justify-between gap-2 type-caption">
                  <span className="led-caption text-label-secondary truncate">{bar.label}</span>
                  <span className="stats-number text-[13px] text-label shrink-0">
                    {Math.round(bar.percent)}%
                  </span>
                </div>
                <LedBar
                  segments={HUD_SEGMENTS}
                  className="mt-1"
                  split={[{ fraction: bar.percent / 100, color: bar.color }]}
                />
                <p className="led-caption type-footnote text-label-secondary mt-0.5">
                  {resetsIn(resetDate(bar.resetsAt), state.now)}
                </p>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  )
}
