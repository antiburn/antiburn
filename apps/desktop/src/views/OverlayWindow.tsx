import { useCallback, useState, useSyncExternalStore } from "react"

import { LedBar } from "../components/ui/LedBar"
import { Confetti } from "../components/ui/Confetti"
import { TokenMap } from "../components/ui/TokenMap"
import { liveWindowSweeps } from "../lib/presentation/liveUsage"
import { blockedBars, resetsIn, type UsageBarItem } from "../lib/usageBars"
import { OverlaySession } from "./overlay/OverlaySession"

const HUD_SEGMENTS = 20

/**
 * The bar's row within its provider's run of bars. The bars of one provider
 * sit together, so the count of bars above it with the same provider is
 * its row.
 */
function providerRow(bars: readonly UsageBarItem[], index: number): number {
  const provider = bars[index]!.provider
  let row = 0
  for (let above = 0; above < index; above += 1) {
    if (bars[above]!.provider === provider) row += 1
  }
  return row
}

/** Render the content-sized usage HUD. The detail window owns the full stats. */
export function OverlayWindow() {
  const [session] = useState(() => new OverlaySession())
  const state = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
    session.getSnapshot,
  )
  // A bar scoped to one model sweeps only while a live session runs that
  // model. Every other bar sweeps for any live session on its provider.
  const sweeping = state.bars.map((bar) =>
    liveWindowSweeps(bar, state.liveProviders.includes(bar.provider), state.liveModels),
  )
  const panelRef = useCallback(
    (node: HTMLDivElement | null) => session.registerPanel(node),
    [session],
  )
  const blocked = blockedBars(state.bars)[0] ?? null

  return (
    <div
      className="h-screen w-screen bg-transparent"
      onMouseEnter={() => session.requestHover(true)}
      onMouseLeave={() => session.requestHover(false)}
    >
      <div
        ref={panelRef}
        className="hud-frame relative mx-2 select-none rounded-xl bg-hud-frame p-2.5"
        // The HUD paints the same translucent frame at rest and on hover, so
        // the bars read as one object and the desktop still shows through.
        // `hud-frame` draws the gradient stroke around it.
        onMouseDown={(event) => session.startDrag(event)}
      >
        {/*
          The close control is off for now (Keith, 2026-09-16). The menu bar
          toggle still hides the HUD. Restore this block to bring it back:

          const showClose = state.hovered && !state.dragging
          <button
            type="button"
            aria-label="Close overlay"
            onClick={() => session.close()}
            className={`hud-control absolute top-2 right-3 translate-x-1/2 -translate-y-1/2 rounded-full border border-hud-control-edge p-0.5 text-hud-control-ink hover:text-label transition-opacity duration-[var(--duration-fast)] ease-out ${
              showClose ? "opacity-100" : "pointer-events-none opacity-0"
            }`}
            style={{ backgroundColor: "var(--color-bg-hud)" }}
          >
            <X size={10} />
          </button>
        */}

        {state.showMap && (
          <div className="mb-2">
            <TokenMap layout={state.tokenMap} onHoverBlob={session.setHoverBlob} />
          </div>
        )}

        {state.bars.length === 0 ? (
          <div
            className={`hud-leds pointer-events-none ${state.sessionLive ? "led-clock led-clock-soft" : ""}`.trimEnd()}
          >
            <LedBar segments={HUD_SEGMENTS} split={[]} live={state.sessionLive} />
          </div>
        ) : (
          // `led-clock` runs the one sweep clock every live bar reads, so the
          // bars stay in phase whenever each bar joined.
          <div
            className={`hud-leds pointer-events-none space-y-[3px] ${sweeping.some(Boolean) ? "led-clock led-clock-soft" : ""}`.trimEnd()}
          >
            {state.bars.map((bar, index) => (
              <LedBar
                key={bar.key}
                segments={HUD_SEGMENTS}
                split={[{ fraction: bar.percent / 100, color: bar.color }]}
                live={sweeping[index]!}
                row={providerRow(state.bars, index)}
                blinkLast={state.sessionLive && index === 0}
                blinkPeriodMs={state.blinkPeriodMs}
                blinkColor={
                  state.tokenMap.liveMode
                    ? `var(--color-mode-${state.tokenMap.liveMode})`
                    : null
                }
                expectedFraction={bar.expectedFraction}
              />
            ))}
          </div>
        )}

        {state.celebration ? (
          <div className="relative mt-1.5" data-testid="hud-celebration">
            <Confetti />
            <p className="led-caption type-footnote text-label text-center">
              {state.celebration}
            </p>
          </div>
        ) : (
          blocked && (
            <p
              // The time leads: the line clips at its tail on a narrow HUD.
              className="led-caption type-footnote text-label-secondary mt-1.5 truncate"
              data-testid="hud-countdown"
            >
              {resetsIn(blocked.resetsAt, state.now)} · {blocked.label}
            </p>
          )
        )}
      </div>
    </div>
  )
}
