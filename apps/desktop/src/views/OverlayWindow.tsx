import { useCallback, useState, useSyncExternalStore, type CSSProperties } from "react"

import { LedBar } from "../components/ui/LedBar"
import { Confetti } from "../components/ui/Confetti"
import { TokenMap } from "../components/ui/TokenMap"
import { blockedBars, resetsIn } from "../lib/usageBars"
import { OverlaySession, type OverlaySnapshot } from "./overlay/OverlaySession"

const HUD_SEGMENTS = 20
/** The content padding on each side of the panel, in pixels (`p-2.5`). */
const PANEL_PAD_PX = 10
/** The panel content width in the floating frame: the window minus its margins and padding. */
const FLOATING_CONTENT_PX = 140
/** The diameter of one LED segment, in pixels (`h-1.5 w-1.5`). */
const LED_DOT_PX = 6
/** The smallest gap between bar rows, which the narrow floating frame keeps. */
const LED_ROW_GAP_MIN_PX = 3

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
  const onIsland = state.island.island !== "off"
  const contentWidth = onIsland ? islandContentWidth(state) : FLOATING_CONTENT_PX
  const content = (
    <HudContent
      state={state}
      session={session}
      contentWidth={contentWidth}
      // The island's live LED sits in its wing, so its bars do not blink.
      blink={!onIsland}
    />
  )

  return (
    <div
      className="h-screen w-screen bg-transparent"
      onMouseEnter={() => session.requestHover(true)}
      onMouseLeave={() => session.requestHover(false)}
    >
      {onIsland ? (
        <IslandPanel state={state} panelRef={panelRef} session={session}>
          {content}
        </IslandPanel>
      ) : (
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
          {content}
        </div>
      )}
    </div>
  )
}

/**
 * The content width when the HUD is in the notch, in pixels.
 *
 * Collapsed and expanded, the panel spans the notch and both wings. The drag
 * preview keeps the floating window, so it keeps the floating width.
 */
function islandContentWidth(state: OverlaySnapshot): number {
  const { island } = state
  if (island.island === "preview") return FLOATING_CONTENT_PX
  return island.notch + island.wing * 2 - PANEL_PAD_PX * 2
}

/**
 * The island: a black panel that sits in the notch.
 *
 * The top row is the notch row: a wing either side of an empty span the
 * notch covers. Collapsed, that row is the whole panel. Expanded, and in
 * the drag preview, the HUD content hangs below it. The gutters either side
 * hold the top corners, which curve out into the screen edge.
 */
function IslandPanel({
  state,
  panelRef,
  session,
  children,
}: {
  state: OverlaySnapshot
  panelRef: (node: HTMLDivElement | null) => void
  session: OverlaySession
  children: React.ReactNode
}) {
  const { island } = state
  const preview = island.island === "preview"
  const collapsed = island.island === "collapsed"
  const open = !collapsed
  const shape = [
    "hud-island relative select-none bg-hud-island",
    open ? "hud-island-open" : "hud-island-closed",
    preview ? "mx-2" : "hud-island-fillets",
  ].join(" ")
  const gutter: CSSProperties | undefined = preview
    ? undefined
    : { marginLeft: island.fillet, marginRight: island.fillet }
  const wing: CSSProperties = { width: island.wing }
  const liveColor = state.tokenMap.liveMode
    ? `var(--color-mode-${state.tokenMap.liveMode})`
    : "var(--color-brand-tint)"
  const topBar = state.bars[0] ?? null
  const figure = state.spendFigure

  return (
    <div
      ref={panelRef}
      className={shape}
      style={gutter}
      data-island={island.island}
      onMouseDown={(event) => session.startDrag(event)}
    >
      <div className="flex items-center" style={{ height: island.height }}>
        <div className="flex shrink-0 items-center justify-center" style={wing}>
          <span
            data-testid="island-live-led"
            className={`h-1.5 w-1.5 rounded-full ${
              state.sessionLive ? "led-lit led-blink" : "led-off bg-led-off"
            }`}
            style={
              state.sessionLive
                ? ({
                    backgroundColor: liveColor,
                    "--led-on": liveColor,
                    "--led-period": `${state.blinkPeriodMs}ms`,
                  } as CSSProperties)
                : undefined
            }
          />
        </div>
        {/* The notch covers this span. In the preview there is no notch, so it stretches. */}
        <div
          className={preview ? "flex-1" : "shrink-0"}
          style={preview ? undefined : { width: island.notch }}
        />
        <div className="flex shrink-0 items-center justify-center" style={wing}>
          {figure ? (
            <span
              data-testid="island-spend"
              className="led-caption type-footnote text-hud-island-ink leading-none"
            >
              {figure}
            </span>
          ) : (
            <span
              data-testid="island-usage-led"
              className={`h-1.5 w-1.5 rounded-full ${topBar ? "led-lit" : "led-off bg-led-off"}`}
              style={topBar ? { backgroundColor: topBar.color } : undefined}
            />
          )}
        </div>
      </div>
      {open && <div className="p-2.5">{children}</div>}
    </div>
  )
}

/** The map, the bars, and the caption under them. The same in every frame. */
function HudContent({
  state,
  session,
  contentWidth,
  blink,
}: {
  state: OverlaySnapshot
  session: OverlaySession
  contentWidth: number
  blink: boolean
}) {
  const blocked = blockedBars(state.bars)[0] ?? null
  // The bars sit their segments edge to edge across the content, so a wide
  // frame spreads them. The rows take that same gap, and the dots read as a
  // grid instead of rows of different pitch.
  const rowGap = Math.max(
    LED_ROW_GAP_MIN_PX,
    (contentWidth - HUD_SEGMENTS * LED_DOT_PX) / (HUD_SEGMENTS - 1),
  )
  const blinkColor = state.tokenMap.liveMode
    ? `var(--color-mode-${state.tokenMap.liveMode})`
    : null
  // The island is black in both themes, so its captions take its own ink.
  const ink = blink ? "text-label" : "text-hud-island-ink"
  const inkSecondary = blink ? "text-label-secondary" : "text-hud-island-ink"
  return (
    <>
      {state.showMap && (
        <div className="mb-2">
          <TokenMap
            layout={state.tokenMap}
            contentWidth={contentWidth}
            onHoverBlob={session.setHoverBlob}
          />
        </div>
      )}

      {state.bars.length === 0 ? (
        <div className="hud-leds pointer-events-none">
          <LedBar
            segments={HUD_SEGMENTS}
            split={[]}
            blinkLast={blink && state.sessionLive}
            blinkPeriodMs={state.blinkPeriodMs}
            blinkColor={blinkColor}
          />
        </div>
      ) : (
        // The HUD shows the spend-rate blink alone. The live sweep stays on
        // the popover meters.
        <div
          className="hud-leds pointer-events-none flex flex-col"
          style={{ rowGap: `${rowGap}px` }}
        >
          {state.bars.map((bar, index) => (
            <LedBar
              key={bar.key}
              segments={HUD_SEGMENTS}
              split={[{ fraction: bar.percent / 100, color: bar.color }]}
              blinkLast={blink && state.sessionLive && index === 0}
              blinkPeriodMs={state.blinkPeriodMs}
              blinkColor={blinkColor}
              expectedFraction={bar.expectedFraction}
            />
          ))}
        </div>
      )}

      {state.celebration ? (
        <div className="relative mt-1.5" data-testid="hud-celebration">
          <Confetti />
          <p className={`led-caption type-footnote ${ink} text-center`}>{state.celebration}</p>
        </div>
      ) : (
        blocked && (
          <p
            // The time leads: the line clips at its tail on a narrow HUD.
            className={`led-caption type-footnote ${inkSecondary} mt-1.5 truncate`}
            data-testid="hud-countdown"
          >
            {resetsIn(blocked.resetsAt, state.now)} · {blocked.label}
          </p>
        )
      )}
    </>
  )
}
