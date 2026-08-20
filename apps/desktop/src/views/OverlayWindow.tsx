// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { useCallback, useState, useSyncExternalStore } from "react"

import { X } from "lucide-react"

import { LedBar } from "../components/ui/LedBar"
import { resetsIn } from "../lib/usageBars"
import { OverlaySession } from "./overlay/OverlaySession"

const HUD_SEGMENTS = 20

/** Render the floating usage HUD. */
export function OverlayWindow() {
  const [session] = useState(() => new OverlaySession())
  const state = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
    session.getSnapshot,
  )
  const expanded = state.hovered && !state.dragging
  const lift =
    expanded && state.flipUp
      ? Math.max(0, state.panelHeights.expanded - state.panelHeights.collapsed)
      : 0
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
        className={`mx-2 select-none rounded-xl transition-all duration-150 ${
          expanded
            ? "bevel border border-separator px-3 pt-2 pb-3"
            : "border border-transparent px-3 pt-2 pb-2"
        }`}
        style={{
          marginTop: state.reserveAbove - lift,
          backgroundColor: expanded ? "var(--color-bg-hud)" : "transparent",
          visibility: state.swapping ? "hidden" : "visible",
        }}
        onMouseDown={(event) => session.startDrag(event)}
      >
        <div
          className={`flex items-center justify-between transition-opacity duration-150 ${
            expanded ? "opacity-100 mb-1.5" : "pointer-events-none h-0 opacity-0"
          }`}
        >
          <button
            type="button"
            aria-label="Open antiburn settings"
            onClick={() => session.openSettings()}
            className="font-bitcount text-[11px] text-label-tertiary lowercase hover:text-label-secondary transition-colors duration-[var(--duration-fast)] ease-out"
          >
            antiburn
          </button>
          <button
            type="button"
            aria-label="Close overlay"
            onClick={() => session.close()}
            className="p-1 -m-1 rounded-md text-label-tertiary hover:text-label-secondary hover:bg-surface-tertiary transition-colors duration-[var(--duration-fast)] ease-out"
          >
            <X size={13} />
          </button>
        </div>

        {state.bars.length === 0 ? (
          expanded ? (
            <p className="type-caption text-label-tertiary pointer-events-none">
              No usage limits detected yet.
            </p>
          ) : (
            <div className="pointer-events-none">
              <LedBar segments={HUD_SEGMENTS} split={[]} />
            </div>
          )
        ) : (
          <div className={`pointer-events-none ${expanded ? "space-y-2" : "space-y-[3px]"}`}>
            {state.bars.map((bar, index) => (
              <div key={bar.key}>
                {expanded && (
                  <div className="flex items-baseline justify-between gap-2 type-caption">
                    <span className="led-caption text-label-secondary truncate">
                      {bar.label}
                    </span>
                    <span className="stats-number text-[13px] text-label shrink-0">
                      {Math.round(bar.percent)}%
                    </span>
                  </div>
                )}
                <LedBar
                  segments={HUD_SEGMENTS}
                  className={expanded ? "mt-1" : ""}
                  split={[{ fraction: bar.percent / 100, color: bar.color }]}
                  blinkLast={state.sessionLive && index === 0}
                />
                {expanded && (
                  <p className="led-caption type-footnote text-label-secondary mt-0.5">
                    {resetsIn(bar.resetsAt, state.now)}
                  </p>
                )}
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  )
}
