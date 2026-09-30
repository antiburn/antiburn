// Prototype: a fake first-run scan. The sidebar starts it; the Overview fixes
// section reads it. The timers here are the external system, so no component
// needs an effect.

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

function update(next: Partial<FtueSnapshot>): void {
  snapshot = { ...snapshot, ...next }
  for (const listener of listeners) listener()
}

export function subscribeFtue(listener: () => void): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
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
