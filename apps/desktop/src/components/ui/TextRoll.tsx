import { memo, useState, type CSSProperties } from "react"

import { cn } from "../../lib/cn"
import "./text-roll.css"

/**
 * Per-character jitter so a multi-digit roll reads as a cascade of individual
 * tumblers rather than one rigid block. Values are deterministic per index
 * (stable across renders, no impure Math.random in render) and intentionally
 * internal: consumers tune the shared feel via the `--text-roll-*` custom
 * properties, not the per-glyph personality.
 */
const DURATION_JITTER = 0.25
const STAGGER_JITTER = 0.2
const MAX_TILT_DEG = 3

/** Defaults mirrored from text-roll.css, used only to rank which cell's
 *  animation finishes last (the settle signal). If a consumer overrides the
 *  CSS variables with wildly different ratios the ranking can be slightly off,
 *  which at worst snaps the true last cell to rest a moment early. */
const BASE_DURATION_MS = 300
const BASE_STAGGER_MS = 45

/** Deterministic pseudo-random value in [-1, 1] for a (index, channel) pair. */
function jitter(index: number, channel: number): number {
  const seed = Math.sin((index + 1) * 12.9898 + channel * 78.233) * 43758.5453
  return (seed - Math.floor(seed)) * 2 - 1
}

interface CellPlan {
  key: string
  char: string
  prevChar: string | undefined
  changed: boolean
  style: CSSProperties | undefined
  /** Approximate animation end time, for picking the settle reporter. */
  endMs: number
}

function planCells(chars: string[], prevChars: string[] | null): CellPlan[] {
  // Right-aligned diff: when the length changes, characters are matched by
  // their distance from the END of the string. For numeric labels this keeps
  // the decimal point and trailing digits aligned ($9.99 -> $10.00 rolls the
  // leading digits and leaves "." alone) instead of rolling punctuation into
  // digits the way a left-aligned index diff would.
  const offset = prevChars === null ? 0 : prevChars.length - chars.length

  return chars.map((char, index) => {
    const prevChar = prevChars?.[index + offset]
    const changed = prevChars !== null && prevChar !== char
    let style: CSSProperties | undefined
    let endMs = 0
    if (changed) {
      const durationMultiplier = 1 + DURATION_JITTER * jitter(index, 1)
      const staggerPosition = index * (1 + STAGGER_JITTER * jitter(index, 2))
      const tiltDeg = MAX_TILT_DEG * jitter(index, 3)
      style = {
        "--tr-dm": durationMultiplier.toFixed(3),
        "--tr-sp": staggerPosition.toFixed(3),
        "--tr-tilt": `${tiltDeg.toFixed(2)}deg`,
      } as CSSProperties
      endMs = staggerPosition * BASE_STAGGER_MS + durationMultiplier * BASE_DURATION_MS
    }
    return {
      // Keyed by distance from the right edge so the stable tail of the
      // string keeps element identity when the length changes.
      key: `c${chars.length - index}`,
      char,
      prevChar,
      changed,
      style,
      endMs,
    }
  })
}

export interface TextRollProps {
  /** The text to display. Changed characters roll to their new value. */
  text: string
  /** Extra classes for the root span (inherits font styles from context). */
  className?: string
}

/**
 * Inline text whose characters roll vertically (odometer-style) whenever the
 * `text` prop changes, diffing old and new strings right-aligned so the
 * stable tail of a numeric label stays still.
 *
 * Deliberately declarative: CSS animations restart when an element mounts, so
 * React keys do all the sequencing — no refs, no effects, no measurement, no
 * timers. While a roll is in flight each changed cell holds two spans (the
 * outgoing face parked by `animation-fill-mode: both`); when the last cell's
 * animation ends, `prev` is cleared and every cell collapses back to a single
 * static span, releasing the animation classes and their `will-change` hint.
 *
 * A re-render with an unchanged `text` (e.g. a parent polling interval) keys
 * nothing new and therefore animates nothing. A change arriving mid-roll
 * re-keys the still-changing cells (face keys derive from the characters, and
 * consecutive changes always differ per cell), restarting them toward the new
 * value; cells that already match snap to rest — standard odometer behavior.
 *
 * Known tradeoff: when the text shrinks, the surplus leading cell unmounts
 * with no exit animation (the container's width snap is instant by design).
 */
export const TextRoll = memo(function TextRoll({ text, className }: TextRollProps) {
  // Derived-state-during-render (the React-documented alternative to a
  // useEffect reset): adopt the new text and remember the previous one so the
  // render below can diff them.
  const [state, setState] = useState({ text, prev: null as string | null })
  if (text !== state.text) {
    setState((s) => ({ text, prev: s.text }))
  }

  // Code-point split. Sufficient for the numeric/ASCII labels this is built
  // for; swap in Intl.Segmenter here if grapheme clusters ever matter.
  const chars = Array.from(state.text)
  const prevChars = state.prev === null ? null : Array.from(state.prev)
  const cells = planCells(chars, prevChars)

  // The changed cell whose animation ends last reports the settle, collapsing
  // the roll structure. Guarded against a newer text having arrived meanwhile.
  const settleIndex = cells.reduce(
    (last, cell, index) =>
      cell.changed && cell.endMs > (cells[last]?.endMs ?? -1) ? index : last,
    -1,
  )
  const settledText = state.text
  const handleSettle = () => {
    setState((s) => (s.text === settledText ? { text: s.text, prev: null } : s))
  }

  return (
    <span className={cn("text-roll", className)}>
      {/* Assistive tech reads the plain string; the animated cells below are
          presentation-only (and excluded from text selection, so copying the
          element yields the string exactly once). */}
      <span className="text-roll-sr">{state.text}</span>
      <span className="text-roll-cells" aria-hidden="true">
        {cells.map((cell, index) => (
          <span key={cell.key} className="text-roll-cell" style={cell.style}>
            {cell.changed && cell.prevChar !== undefined && (
              <span key={`out-${cell.prevChar}`} className="text-roll-out">
                {cell.prevChar}
              </span>
            )}
            {/* The in-flow span doubles as the cell's sizer: transforms do
                not affect layout, so the cell keeps its width even while the
                glyph is translated out of view. */}
            <span
              key={cell.changed ? `in-${cell.char}` : "static"}
              className={cell.changed ? "text-roll-in" : undefined}
              onAnimationEnd={index === settleIndex ? handleSettle : undefined}
            >
              {cell.char}
            </span>
          </span>
        ))}
      </span>
    </span>
  )
})
