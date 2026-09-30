// Prototype: a fake first-run scan. The Overview fixes section reads it, and
// the debug-only "Reset FTUE" tray item restarts it. The timers and the tray
// event are the external system, so no component needs an effect.

import { onFtueReset } from "../../../lib/ipc"

export interface FtueSnapshot {
  /** Share of sessions read, 0 to 1. */
  reading: number
  /** Share of the analysis done, 0 to 1. */
  analysis: number
  dismissed: boolean
}

export const FTUE_SESSION_TOTAL = 59

const DONE: FtueSnapshot = { reading: 1, analysis: 1, dismissed: false }

let snapshot: FtueSnapshot = DONE
let timer: ReturnType<typeof setTimeout> | undefined
const listeners = new Set<() => void>()

// The tray's reset event has one listener for as long as an Overview view is
// mounted, and none otherwise. `resetGeneration` drops a listener that
// resolves after a newer subscribe/unsubscribe cycle already moved on.
let unlistenReset: (() => void) | null = null
let resetGeneration = 0

function startResetListener(): void {
  const generation = ++resetGeneration
  void onFtueReset(() => startFtue()).then((stop) => {
    if (generation !== resetGeneration) {
      stop()
      return
    }
    unlistenReset = stop
  })
}

function stopResetListener(): void {
  resetGeneration += 1
  unlistenReset?.()
  unlistenReset = null
}

function update(next: Partial<FtueSnapshot>): void {
  snapshot = { ...snapshot, ...next }
  for (const listener of listeners) listener()
}

export function subscribeFtue(listener: () => void): () => void {
  listeners.add(listener)
  if (listeners.size === 1) startResetListener()
  return () => {
    listeners.delete(listener)
    if (listeners.size === 0) stopResetListener()
  }
}

export function ftueSnapshot(): FtueSnapshot {
  return snapshot
}

export function dismissFtueCallout(): void {
  update({ dismissed: true })
}

/** Reset to two empty bars, then fill them one after the other. */
export function startFtue(): void {
  if (timer !== undefined) clearTimeout(timer)
  update({ reading: 0, analysis: 0, dismissed: false })
  timer = setTimeout(() => step("reading"), 700)
}

function step(lane: "reading" | "analysis"): void {
  const current = snapshot[lane]
  // Fits and starts: mostly small hops, now and then a burst, now and then
  // a stall.
  const roll = Math.random()
  const hop =
    roll < 0.15 ? 0 : roll < 0.8 ? 0.01 + Math.random() * 0.05 : 0.08 + Math.random() * 0.12
  const next = Math.min(1, current + hop)
  update({ [lane]: next })

  if (next >= 1) {
    timer = lane === "reading" ? setTimeout(() => step("analysis"), 500) : undefined
    return
  }
  const delay = roll < 0.15 ? 600 + Math.random() * 900 : 60 + Math.random() * 260
  timer = setTimeout(() => step(lane), delay)
}
