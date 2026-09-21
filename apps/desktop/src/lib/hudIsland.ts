import { invoke } from "@tauri-apps/api/core"
import { listen } from "@tauri-apps/api/event"

import type { HudSpendRate } from "./hudIpc"

/** What the island is doing. `off` is the floating or edge-docked HUD. */
type HudIslandPhase = "off" | "preview" | "collapsed" | "expanded"

/** The island geometry the shell reports, in logical pixels. */
export interface HudIslandState {
  island: HudIslandPhase
  /** The drawable strip either side of the notch. */
  wing: number
  /** The transparent gutter outside each wing, where the top corners curve out. */
  fillet: number
  /** The notch width. The webview leaves it empty. */
  notch: number
  /** The notch height: the collapsed row. */
  height: number
}

/** The dock state the shell stores, as `set_hud_island` returns it. */
export interface HudDockSettings {
  docked: boolean
  edge: "left" | "right" | "top" | "bottom"
  island: boolean
}

export const HUD_ISLAND_OFF: HudIslandState = {
  island: "off",
  wing: 0,
  fillet: 0,
  notch: 0,
  height: 0,
}

const ISLAND_STATE_EVENT = "hud-island:state"

/** Whether a connected display has a notch for the HUD to sit in. */
export async function isHudIslandAvailable(): Promise<boolean> {
  return (await invoke<boolean>("hud_island_available")) === true
}

/** The island state now, for a HUD that just mounted. Off when the shell has none. */
export async function getHudIslandState(): Promise<HudIslandState> {
  const state = await invoke<HudIslandState | null | undefined>("hud_island_state")
  return state && typeof state.island === "string" ? state : HUD_ISLAND_OFF
}

/** Put the HUD in the notch, or take it out. Returns the stored dock state. */
export function setHudIsland(on: boolean): Promise<HudDockSettings> {
  return invoke<HudDockSettings>("set_hud_island", { on })
}

/** Open a collapsed island now: a mouse-down on it. It lingers as a peek does. */
export function expandHudIsland(): Promise<void> {
  return invoke("expand_hud_island")
}

/** Follow the island as it previews, collapses, and expands. */
export function onHudIslandState(
  handler: (state: HudIslandState) => void,
): Promise<() => void> {
  return listen<HudIslandState>(ISLAND_STATE_EVENT, (event) => handler(event.payload))
}

/**
 * The spend rate as a figure for the island's wing, or null when there is
 * nothing to show.
 *
 * A wing is 30px, so the figure is three characters and a dollar sign: `$12`,
 * `$1.2`, `$.05`. Below half a cent a minute the wing shows the usage LED
 * instead, as it does with no priced tokens in the window.
 */
export function islandSpendFigure(spend: HudSpendRate | null): string | null {
  if (!spend || spend.pricedShare <= 0) return null
  const rate = spend.usdPerMinute
  if (!(rate >= 0.005)) return null
  if (rate >= 9.95) return `$${Math.round(rate)}`
  if (rate >= 0.995) return `$${rate.toFixed(1)}`
  // "0.05" reads as "$.05": the zero is the character the wing cannot spare.
  return `$${rate.toFixed(2).slice(1)}`
}
