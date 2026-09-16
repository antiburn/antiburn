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
import { formatRate, WORK_MODES, type WorkMode } from "../../lib/tokenMap"
import { resetsIn } from "../../lib/usageBars"

const HUD_SEGMENTS = 20

type DetailSnapshot = {
  bars: HudDetailState["bars"]
  map: HudDetailState["map"]
  /** The spend rate in words, or null when the window carried no tokens. */
  spend: string | null
  /** "usage" for the meter card, or a session key for that agent's card. */
  target: string
  /** The sub-agent under the pointer, or null. */
  subagent: string | null
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
  map: null,
  spend: null,
  target: "usage",
  subagent: null,
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
        map: state.map ?? null,
        spend: state.spend ?? null,
        target: state.target ?? "usage",
        subagent: state.subagent ?? null,
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

/** Spell out the token map: one row per session, then the mode colours. */
function MapLegend({ map }: { map: NonNullable<HudDetailState["map"]> }) {
  return (
    <div className="mb-2 border-b border-separator pb-2" data-testid="hud-detail-map">
      <ul className="space-y-0.5">
        {map.sessions.map((session) => (
          <li key={session.key} className="flex items-baseline gap-1.5 type-caption">
            <span
              aria-hidden="true"
              className="inline-block size-2 shrink-0 self-center rounded-sm border"
              style={{ borderColor: session.frameColor }}
            />
            <span className="text-label truncate">{session.label}</span>
            <span className="stats-number text-label shrink-0 ml-auto">
              {formatRate(session.tokensPerMin)}/min
            </span>
            <span
              aria-label={`mostly ${session.topMode}`}
              className="inline-block size-2 shrink-0 self-center rounded-full"
              style={{ backgroundColor: `var(--color-mode-${session.topMode})` }}
            />
          </li>
        ))}
      </ul>
      <ul className="mt-1.5 flex flex-wrap gap-x-2 gap-y-0.5">
        {WORK_MODES.map((mode) => (
          <li
            key={mode}
            className="flex items-center gap-1 led-caption type-footnote text-label-secondary"
          >
            <span
              aria-hidden="true"
              className="inline-block size-1.5 rounded-full"
              style={{ backgroundColor: `var(--color-mode-${mode})` }}
            />
            {mode}
          </li>
        ))}
      </ul>
      <p className="led-caption type-footnote text-label-secondary mt-1">
        ● = {formatRate(map.dotValue)} tokens/min
      </p>
    </div>
  )
}

/** The mode split of one transcript as LED segments, busiest mode first. */
function modeSplit(
  modes: HudDetailSession_["modes"],
): Array<{ fraction: number; color: string }> {
  const total = WORK_MODES.reduce((sum, mode) => sum + modes[mode], 0)
  if (total === 0) return []
  return [...WORK_MODES]
    .filter((mode) => modes[mode] > 0)
    .sort((a, b) => modes[b] - modes[a])
    .map((mode: WorkMode) => ({
      fraction: modes[mode] / total,
      color: `var(--color-mode-${mode})`,
    }))
}

type HudDetailSession_ = NonNullable<HudDetailState["map"]>["sessions"][number]

/** One agent box, spelled out: the session, its rate, its modes, its sub-agents. */
/** The mode that paid for most of `modes`; the first mode in order on a tie. */
function topModeOf(modes: HudDetailSession_["modes"]): WorkMode {
  return WORK_MODES.reduce((best, mode) => (modes[mode] > modes[best] ? mode : best))
}

function SessionCard({
  session,
  dotValue,
  subagent,
}: {
  session: HudDetailSession_
  dotValue: number
  /** The sub-agent under the pointer; its row is lit and names its top mode. */
  subagent: string | null
}) {
  return (
    <div data-testid="hud-detail-session">
      <div className="flex items-baseline justify-between gap-2 type-caption">
        <span className="flex min-w-0 items-baseline gap-1.5">
          <span
            aria-hidden="true"
            className="inline-block size-2 shrink-0 self-center rounded-sm border"
            style={{ borderColor: session.frameColor }}
          />
          <span className="text-label truncate">{session.label}</span>
        </span>
        <span className="stats-number text-[13px] text-label shrink-0">
          {formatRate(session.tokensPerMin)}/min
        </span>
      </div>
      <p className="led-caption type-footnote text-label-secondary mt-0.5">
        {session.agent} · mostly {session.topMode}
      </p>
      <LedBar segments={HUD_SEGMENTS} className="mt-1" split={modeSplit(session.modes)} />
      {session.subagents.length > 0 && (
        <ul className="mt-1.5 space-y-0.5">
          {session.subagents.map((entry) => {
            const lit = entry.subagentId === subagent
            return (
              <li
                key={entry.subagentId}
                data-lit={lit || undefined}
                className="flex items-baseline justify-between gap-2 type-caption"
              >
                <span
                  className={`led-caption truncate ${lit ? "text-label" : "text-label-secondary"}`}
                >
                  sub-agent {entry.subagentId.slice(0, 8)}
                  {lit ? ` · mostly ${topModeOf(entry.modes)}` : ""}
                </span>
                <span className="stats-number text-label shrink-0">
                  {formatRate(entry.tokensPerMin)}/min
                </span>
              </li>
            )
          })}
        </ul>
      )}
      <p className="led-caption type-footnote text-label-secondary mt-1.5">
        ● = {formatRate(dotValue)} tokens/min
      </p>
    </div>
  )
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

  const hoveredSession =
    state.target === "usage"
      ? null
      : (state.map?.sessions.find((session) => session.key === state.target) ?? null)

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
        {hoveredSession ? (
          <SessionCard
            session={hoveredSession}
            dotValue={state.map!.dotValue}
            subagent={state.subagent}
          />
        ) : (
          <>
            {state.map && <MapLegend map={state.map} />}
            {state.spend && (
              <p
                className="led-caption type-footnote text-label-secondary mb-1.5"
                data-testid="hud-detail-spend"
              >
                {state.spend}
              </p>
            )}
            {state.bars.length === 0 ? (
              <p className="type-caption text-label-tertiary">
                {state.noMeterSelected ? "No meter selected." : "No usage limits detected yet."}
              </p>
            ) : (
              <div className="space-y-2">
                {state.bars.map((bar) => (
                  <div key={bar.key}>
                    <div className="flex items-baseline justify-between gap-2 type-caption">
                      <span className="led-caption text-label-secondary truncate">
                        {bar.label}
                      </span>
                      <span className="stats-number text-[13px] text-label shrink-0">
                        {Math.round(bar.percent)}%
                      </span>
                    </div>
                    <LedBar
                      segments={HUD_SEGMENTS}
                      className="mt-1"
                      split={[{ fraction: bar.percent / 100, color: bar.color }]}
                      expectedFraction={bar.expectedFraction}
                    />
                    <p className="led-caption type-footnote text-label-secondary mt-0.5">
                      {resetsIn(resetDate(bar.resetsAt), state.now)}
                    </p>
                  </div>
                ))}
              </div>
            )}
          </>
        )}
      </div>
    </div>
  )
}
