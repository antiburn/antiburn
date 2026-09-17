import { useCallback, useState, useSyncExternalStore } from "react"

import { LedBar } from "../components/ui/LedBar"
import { Confetti } from "../components/ui/Confetti"
import { blockedBars, resetsIn } from "../lib/usageBars"
import { TokenMap } from "../components/ui/TokenMap"
import { OverlaySession } from "./overlay/OverlaySession"

const HUD_SEGMENTS = 20

/** Render the content-sized usage HUD. The detail window owns the full stats. */
export function OverlayWindow() {
  const [session] = useState(() => new OverlaySession())
  const state = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
    session.getSnapshot,
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
          <div className="pointer-events-none">
            <LedBar segments={HUD_SEGMENTS} split={[]} />
          </div>
        ) : (
          <div className="pointer-events-none space-y-[3px]">
            {state.bars.map((bar, index) => (
              <LedBar
                key={bar.key}
                segments={HUD_SEGMENTS}
                split={[{ fraction: bar.percent / 100, color: bar.color }]}
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
