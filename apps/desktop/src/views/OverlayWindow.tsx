import { useCallback, useState, useSyncExternalStore } from "react"

import { X } from "lucide-react"

import { LedBar } from "../components/ui/LedBar"
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
  const showClose = state.hovered && !state.dragging
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
          className={`absolute top-2 right-3 translate-x-1/2 -translate-y-1/2 rounded-full border border-separator p-0.5 text-label-tertiary hover:text-label-secondary transition-opacity duration-[var(--duration-fast)] ease-out ${
            showClose ? "opacity-100" : "pointer-events-none opacity-0"
          }`}
          style={{ backgroundColor: "var(--color-bg-hud)" }}
        >
          <X size={10} />
        </button>

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
                expectedFraction={bar.expectedFraction}
              />
            ))}
          </div>
        )}
      </div>
    </div>
  )
}
