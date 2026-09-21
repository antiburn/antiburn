/**
 * Typed shell IPC edge for the HUD token map and the hover detail card.
 * The detail state extends the base state in `ipc.ts` with the map fields.
 */

import { invoke } from "@tauri-apps/api/core"

import {
  hasShell,
  getHudDetailState as getHudDetailStateBase,
  showHudDetail as showHudDetailBase,
  type HudDetailState as HudDetailBaseState,
} from "./ipc"

/** Tokens per mode over the token-map window. Mirrors `HudModeTokens`. */
export interface HudModeTokens {
  looking: number
  running: number
  changing: number
  delegating: number
  thinking: number
  talking: number
  other: number
}

export interface HudTokenMapSubagent {
  subagentId: string
  tokensPerMin: number
  modes: HudModeTokens
}

/** One session in the token map. `modes` covers the parent transcript only. */
export interface HudTokenMapSession {
  agent: string
  sessionId: string
  title: string | null
  lastTurnEpoch: number | null
  tokensPerMin: number
  modes: HudModeTokens
  subagents: HudTokenMapSubagent[]
}

/** Dollars per minute over the window, summed across every session. */
export interface HudSpendRate {
  usdPerMinute: number
  windowSecs: number
  /** Priced tokens over all tokens in the window, 0-1. Below 1 the rate is a floor. */
  pricedShare: number
}

export interface HudTokenMapPayload {
  nowEpoch: number
  windowSecs: number
  /** Busiest first. */
  sessions: HudTokenMapSession[]
  /** Null when no turn in the window carried tokens. */
  spend: HudSpendRate | null
}

/** Tokens per minute by mode for every session that wrote in the window. */
export async function getHudTokenMap(windowSecs?: number): Promise<HudTokenMapPayload | null> {
  if (!hasShell()) return null
  return invoke<HudTokenMapPayload>("get_hud_token_map", { windowSecs })
}

/** One live session as the hover detail window lists it. */
export interface HudDetailSession {
  key: string
  /** The session title, or the agent name when the title is unknown. */
  label: string
  agent: string
  tokensPerMin: number
  /** The parent transcript's tokens per mode in the window. */
  modes: HudModeTokens
  subagents: HudTokenMapSubagent[]
  /** The mode that paid for most of the session's tokens: a `HudModeTokens` key. */
  topMode: keyof HudModeTokens
  /** The blob's frame colour on the map, as a CSS colour value. */
  frameColor: string
}

/** The token-map summary the hover detail window spells out. */
export interface HudDetailMap {
  /** Tokens per minute one full dot stands for. */
  dotValue: number
  sessions: HudDetailSession[]
}

/** The payload the HUD pushes to the hover detail window. */
export interface HudDetailState extends HudDetailBaseState {
  /** Null when the map is off or no session wrote in the window. */
  map: HudDetailMap | null
  /** The spend rate in words, or null when the window carried no tokens. */
  spend: string | null
  /**
   * What the pointer is over: "usage" for the meter, or a `map.sessions` key
   * for one agent box. The card shows the matching content.
   */
  target: string
  /** The sub-agent whose dot the pointer is on, or null. */
  subagent?: string | null
}

/** Request the hover detail window with the newest usage payload and map. */
export async function showHudDetail(state: HudDetailState): Promise<void> {
  await showHudDetailBase(state)
}

/**
 * The newest detail payload, for a detail webview that mounts late.
 * The shell stores the state `showHudDetail` sent, so the map fields are present.
 */
export async function getHudDetailState(): Promise<HudDetailState | null> {
  return (await getHudDetailStateBase()) as HudDetailState | null
}
