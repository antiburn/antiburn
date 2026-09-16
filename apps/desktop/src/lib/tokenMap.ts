import type { HudModeTokens, HudTokenMapPayload, HudTokenMapSession } from "./ipc"

export type WorkMode = keyof HudModeTokens

/** Every mode, in the order dots lay out inside a blob. */
export const WORK_MODES: readonly WorkMode[] = [
  "looking",
  "running",
  "changing",
  "delegating",
  "thinking",
  "talking",
  "other",
]

/** Candidate tokens-per-minute values for one dot, finest first. */
export const DOT_VALUE_LADDER: readonly number[] = [
  250, 500, 1_000, 2_000, 5_000, 10_000, 20_000, 50_000, 100_000, 200_000, 500_000,
]

/** Thin frame colours, one per blob in order. Distinct from the mode palette. */
export const FRAME_COLORS: readonly string[] = [
  "var(--color-label-tertiary)",
  "var(--color-system-red)",
  "var(--color-system-indigo-text)",
  "var(--color-system-gold-text)",
  "var(--color-system-blue)",
  "var(--color-system-green)",
]

/** The frame colour of the blob at `index`. */
export function frameColor(index: number): string {
  return FRAME_COLORS[index % FRAME_COLORS.length]
}

/** Format a tokens-per-minute rate for a label: 60, 4.2k, 12k, 1.3M. */
export function formatRate(rate: number): string {
  if (rate >= 1_000_000) return `${trimZero(rate / 1_000_000)}M`
  if (rate >= 10_000) return `${Math.round(rate / 1_000)}k`
  if (rate >= 1_000) return `${trimZero(rate / 1_000)}k`
  return `${Math.round(rate)}`
}

function trimZero(value: number): string {
  return value.toFixed(1).replace(/\.0$/, "")
}

/**
 * A session whose last turn is older than this, in seconds, leaves the map.
 * The newest turn inside it pulses. It matches the VU meter's live window.
 */
export const LIVE_SECS = 90

export type TokenMapDot = {
  /** Grid cell column and row, in cells. */
  x: number
  y: number
  mode: WorkMode
  /** True for a sub-agent's dot: drawn with a smaller radius. */
  small: boolean
  /** True when the dot stands in for a session that rounds to zero dots. */
  dim: boolean
  /** True for the newest turn on the map. */
  live: boolean
  blob: number
}

export type TokenMapBlob = {
  key: string
  sessionId: string
  agent: string
  title: string | null
  /** Rectangle in cells. */
  x: number
  y: number
  w: number
  h: number
  tokensPerMin: number
  dots: number
  /** The mode that paid for most of the session's tokens in the window. */
  topMode: WorkMode
}

export type TokenMapLayout = {
  /** The square is `cells` by `cells` cells. */
  cells: number
  /** Tokens per minute one full dot stands for. */
  dotValue: number
  blobs: TokenMapBlob[]
  dots: TokenMapDot[]
  /** True when even the coarsest dot value could not fit every session. */
  overflow: boolean
  /** The top mode of the session with the newest turn, or null when none is live. */
  liveMode: WorkMode | null
}

export type TokenMapOptions = {
  cells?: number
  /** A ladder step is not chosen below this value, so a burst does not flicker the scale. */
  minDotValue?: number
}

/** One cell per LED, so the map shares the VU meter's grid. */
const DEFAULT_CELLS = 20
const GAP = 1

type DotSpec = { mode: WorkMode; small: boolean }

/** The session's total rate: parent plus every sub-agent. */
export function sessionRate(session: HudTokenMapSession): number {
  return (
    session.tokensPerMin +
    session.subagents.reduce((sum, subagent) => sum + subagent.tokensPerMin, 0)
  )
}

function dotsFor(
  modes: HudModeTokens,
  rate: number,
  dotValue: number,
  small: boolean,
): DotSpec[] {
  const total = WORK_MODES.reduce((sum, mode) => sum + modes[mode], 0)
  const count = Math.round(rate / dotValue)
  if (total === 0 || count === 0) return []
  // Largest-remainder split, so the dot count per mode sums to `count`.
  const shares = WORK_MODES.map((mode) => ({ mode, exact: (modes[mode] / total) * count }))
  const floors = shares.map((share) => ({ ...share, count: Math.floor(share.exact) }))
  let left = count - floors.reduce((sum, share) => sum + share.count, 0)
  const byRemainder = [...floors].sort((a, b) => b.exact - b.count - (a.exact - a.count))
  for (const share of byRemainder) {
    if (left === 0) break
    if (modes[share.mode] === 0) continue
    share.count += 1
    left -= 1
  }
  return floors.flatMap((share) =>
    Array.from({ length: share.count }, () => ({ mode: share.mode, small })),
  )
}

/** The dots one session gets at a dot value: parent first, then each sub-agent. */
function sessionDots(
  session: HudTokenMapSession,
  dotValue: number,
): { specs: DotSpec[]; dim: boolean } {
  const specs = [
    ...dotsFor(session.modes, session.tokensPerMin, dotValue, false),
    ...session.subagents.flatMap((subagent) =>
      dotsFor(subagent.modes, subagent.tokensPerMin, dotValue, true),
    ),
  ]
  if (specs.length > 0) return { specs, dim: false }
  // A quiet session keeps one dim dot in its top mode, so it is not lost.
  return { specs: [{ mode: topMode(session), small: false }], dim: true }
}

/** The mode with the most parent tokens; the first mode in order on a tie. */
function topMode(session: HudTokenMapSession): WorkMode {
  return WORK_MODES.reduce((best, mode) =>
    session.modes[mode] > session.modes[best] ? mode : best,
  )
}

type Placed = { x: number; y: number; w: number; h: number }

/**
 * Shelf-pack near-square rectangles into the square, largest first.
 * Returns null when a rectangle does not fit.
 */
function shelfPack(sizes: number[], cells: number): Placed[] | null {
  const placed: Placed[] = []
  let x = 0
  let y = 0
  let shelf = 0
  for (const count of sizes) {
    const w = Math.min(cells, Math.ceil(Math.sqrt(count)))
    const h = Math.ceil(count / w)
    if (w > cells || h > cells) return null
    if (x + w > cells) {
      x = 0
      y += shelf + GAP
      shelf = 0
    }
    if (y + h > cells) return null
    placed.push({ x, y, w, h })
    x += w + GAP
    shelf = Math.max(shelf, h)
  }
  return placed
}

/**
 * Turn the payload into positioned dots inside a fixed square.
 *
 * The dot value floats: the finest ladder step at which every session fits
 * wins. More sessions or a burst push the step up; the square never overflows.
 */
export function deriveTokenMap(
  payload: HudTokenMapPayload | null,
  options: TokenMapOptions = {},
): TokenMapLayout {
  const cells = options.cells ?? DEFAULT_CELLS
  const now = payload?.nowEpoch ?? 0
  const sessions = (payload?.sessions ?? [])
    .filter(
      (session) => session.lastTurnEpoch != null && now - session.lastTurnEpoch <= LIVE_SECS,
    )
    .sort((a, b) => sessionRate(b) - sessionRate(a))
  const empty: TokenMapLayout = {
    cells,
    dotValue: DOT_VALUE_LADDER[0],
    blobs: [],
    dots: [],
    overflow: false,
    liveMode: null,
  }
  if (sessions.length === 0) return empty

  const ladder = DOT_VALUE_LADDER.filter((value) => value >= (options.minDotValue ?? 0))
  const candidates =
    ladder.length > 0 ? ladder : [DOT_VALUE_LADDER[DOT_VALUE_LADDER.length - 1]]

  let chosen: {
    dotValue: number
    perSession: ReturnType<typeof sessionDots>[]
    placed: Placed[]
  } | null = null
  for (const dotValue of candidates) {
    const perSession = sessions.map((session) => sessionDots(session, dotValue))
    const placed = shelfPack(
      perSession.map((entry) => entry.specs.length),
      cells,
    )
    if (placed) {
      chosen = { dotValue, perSession, placed }
      break
    }
  }
  if (!chosen) {
    // Even the coarsest step overflows: keep the sessions that fit, drop the rest.
    const dotValue = candidates[candidates.length - 1]
    const perSession = sessions.map((session) => sessionDots(session, dotValue))
    const placed: Placed[] = []
    for (let count = sessions.length; count > 0; count -= 1) {
      const attempt = shelfPack(
        perSession.slice(0, count).map((entry) => entry.specs.length),
        cells,
      )
      if (attempt) {
        placed.push(...attempt)
        break
      }
    }
    chosen = { dotValue, perSession: perSession.slice(0, placed.length), placed }
    return build(sessions.slice(0, placed.length), chosen, cells, payload, true)
  }
  return build(sessions, chosen, cells, payload, false)
}

function build(
  sessions: HudTokenMapSession[],
  chosen: { dotValue: number; perSession: ReturnType<typeof sessionDots>[]; placed: Placed[] },
  cells: number,
  payload: HudTokenMapPayload | null,
  overflow: boolean,
): TokenMapLayout {
  const now = payload?.nowEpoch ?? 0
  const newest = sessions.reduce<number | null>((best, session) => {
    const at = session.lastTurnEpoch
    if (at == null) return best
    return best == null || at > best ? at : best
  }, null)
  const liveIndex =
    newest != null && now - newest <= LIVE_SECS
      ? sessions.findIndex((session) => session.lastTurnEpoch === newest)
      : -1

  const blobs: TokenMapBlob[] = []
  const dots: TokenMapDot[] = []
  sessions.forEach((session, index) => {
    const rect = chosen.placed[index]
    const { specs, dim } = chosen.perSession[index]
    blobs.push({
      key: `${session.agent}:${session.sessionId}`,
      sessionId: session.sessionId,
      agent: session.agent,
      title: session.title,
      x: rect.x,
      y: rect.y,
      w: rect.w,
      h: rect.h,
      tokensPerMin: sessionRate(session),
      dots: specs.length,
      topMode: topMode(session),
    })
    specs.forEach((spec, position) => {
      dots.push({
        x: rect.x + (position % rect.w),
        y: rect.y + Math.floor(position / rect.w),
        mode: spec.mode,
        small: spec.small,
        dim,
        live: index === liveIndex && position === specs.length - 1,
        blob: index,
      })
    })
  })
  const liveMode = liveIndex >= 0 ? topMode(sessions[liveIndex]) : null
  return { cells, dotValue: chosen.dotValue, blobs, dots, overflow, liveMode }
}
