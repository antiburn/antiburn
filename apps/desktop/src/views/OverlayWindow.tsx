// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { useCallback, useState, useSyncExternalStore } from "react"

import { X } from "lucide-react"

import { LedBar } from "../components/ui/LedBar"
import { OverlaySession } from "./overlay/OverlaySession"

const HUD_SEGMENTS = 20

/**
 * Render the floating usage HUD: bars only, at a fixed size. The spelled-out
 * stats live in the separate hover detail window the shell owns.
 */
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
    <div className="h-screen w-screen bg-transparent">
      <div
        ref={panelRef}
        onMouseEnter={() => session.requestHover(true)}
        onMouseLeave={() => session.requestHover(false)}
        className="relative mx-2 select-none rounded-xl border border-transparent px-3 pt-2 pb-2"
        onMouseDown={(event) => session.startDrag(event)}
      >
        {/* top-2/right-3 name the corner of the drawn bars (pt-2, px-3). The
            translate centers the chip on that corner point. */}
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
              />
            ))}
          </div>
        )}
      </div>
    </div>
  )
}
