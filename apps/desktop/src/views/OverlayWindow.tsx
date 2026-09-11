import { useCallback, useState, useSyncExternalStore } from "react"

import { X } from "lucide-react"

import { LedBar } from "../components/ui/LedBar"
import { liveWindowSweeps } from "../lib/presentation/liveUsage"
import type { UsageBarItem } from "../lib/usageBars"
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
  const showClose = state.hovered && !state.dragging
  // A bar scoped to one model sweeps only while a live session runs that
  // model. Every other bar sweeps for any live session on its provider.
  const sweeping = state.bars.map((bar) =>
    liveWindowSweeps(bar, state.liveProviders.includes(bar.provider), state.liveModels),
  )
  const panelRef = useCallback(
    (node: HTMLDivElement | null) => session.registerPanel(node),
    [session],
  )

  return (
    <div
      className="h-screen w-screen bg-transparent"
      onMouseEnter={() => session.requestHover(true)}
      onMouseLeave={() => session.requestHover(false)}
    >
      <div
        ref={panelRef}
        className="relative mx-2 select-none rounded-xl border border-transparent px-3 pt-2 pb-2 transition-colors duration-[var(--duration-fast)] ease-out"
        // At rest the HUD paints no surface and the bars sit on the desktop.
        // On hover it takes a surface, which groups the bars into one object
        // the reader can point at and drag.
        style={state.hovered ? { backgroundColor: "var(--color-bg-hud-hover)" } : undefined}
        onMouseDown={(event) => session.startDrag(event)}
      >
        <button
          type="button"
          aria-label="Close overlay"
          onClick={() => session.close()}
          className={`hud-close absolute top-2 right-3 translate-x-1/2 -translate-y-1/2 rounded-full border border-hud-control-edge p-0.5 text-hud-control-ink hover:text-label transition-opacity duration-[var(--duration-fast)] ease-out ${
            showClose ? "opacity-100" : "pointer-events-none opacity-0"
          }`}
          style={{ backgroundColor: "var(--color-bg-hud)" }}
        >
          <X size={10} />
        </button>

        {state.bars.length === 0 ? (
          <div
            className={`pointer-events-none ${state.sessionLive ? "led-clock led-clock-soft" : ""}`.trimEnd()}
          >
            <LedBar segments={HUD_SEGMENTS} split={[]} live={state.sessionLive} />
          </div>
        ) : (
          // `led-clock` runs the one sweep clock every live bar reads, so the
          // bars stay in phase whenever each bar joined.
          <div
            className={`pointer-events-none space-y-[3px] ${sweeping.some(Boolean) ? "led-clock led-clock-soft" : ""}`.trimEnd()}
          >
            {state.bars.map((bar, index) => (
              <LedBar
                key={bar.key}
                segments={HUD_SEGMENTS}
                split={[{ fraction: bar.percent / 100, color: bar.color }]}
                live={sweeping[index]!}
                row={providerRow(state.bars, index)}
                expectedFraction={bar.expectedFraction}
              />
            ))}
          </div>
        )}
      </div>
    </div>
  )
}
