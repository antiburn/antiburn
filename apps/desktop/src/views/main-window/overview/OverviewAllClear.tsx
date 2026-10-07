import { useState, type CSSProperties } from "react"

import { prefersReducedMotion } from "../../../lib/popoverHeight"

/*
 * The passed-check mark from the Checks list (`BURN_CHECK_MARKS.clean`, the
 * lucide circle check), drawn large. The tick runs from the short leg to the
 * tip, so it draws the way a hand writes it.
 */
const MARK_SIZE = 72

/*
 * Confetti in the mark's own style: short round-capped strokes in theme
 * colours.
 */
const CONFETTI_COUNT = 18
const CONFETTI_SIZE = 14
const CONFETTI_COLOURS = [
  "var(--color-burn-check-pass-fill)",
  "var(--color-brand)",
  "var(--color-system-gold)",
  "var(--color-system-green)",
  "var(--color-system-indigo)",
]

type Confetto = {
  colour: string
  style: CSSProperties
}

/*
 * One burst. Each piece flies out from the mark's centre on its own angle,
 * mostly upward, then drops and fades.
 */
function confettiBurst(): Confetto[] {
  return Array.from({ length: CONFETTI_COUNT }, (_, index) => {
    const angle = (index / CONFETTI_COUNT) * 2 * Math.PI + Math.random() * 0.3
    const distance = 50 + Math.random() * 45
    return {
      colour: CONFETTI_COLOURS[index % CONFETTI_COLOURS.length],
      style: {
        "--confetti-x": `${Math.cos(angle) * distance}px`,
        "--confetti-y": `${Math.sin(angle) * distance * 0.8 - 12}px`,
        "--confetti-turn": `${(Math.random() - 0.5) * 540}deg`,
        animationDelay: `${Math.random() * 80}ms`,
      } as CSSProperties,
    }
  })
}

/**
 * The Overview's all-clear card, shown when no config check needs a fix. The
 * passed-check mark draws in: the ring draws round from the top and the tick
 * strokes in. As the tick lands, confetti bursts from the mark. A click on the mark throws
 * more confetti. Under reduced motion the mark shows complete and no
 * confetti flies.
 */
export function OverviewAllClear({ onShowChecks }: { onShowChecks: () => void }) {
  const [burst, setBurst] = useState(() => ({ id: 0, pieces: confettiBurst() }))
  const reducedMotion = prefersReducedMotion()

  return (
    <div className="flex flex-col items-center gap-(--space-md) py-(--space-md)">
      <button
        type="button"
        aria-label="Throw confetti"
        disabled={reducedMotion}
        onClick={() => setBurst({ id: burst.id + 1, pieces: confettiBurst() })}
        className="relative rounded-full"
      >
        <svg
          viewBox="0 0 24 24"
          width={MARK_SIZE}
          height={MARK_SIZE}
          fill="none"
          stroke="currentColor"
          strokeWidth={1.75}
          strokeLinecap="round"
          strokeLinejoin="round"
          className="overview-all-clear-mark text-burn-check-pass-fill"
          aria-hidden="true"
        >
          <circle cx="12" cy="12" r="10" pathLength={1} className="overview-all-clear-ring" />
          <path d="M8 12l2.5 2.5L16 9" pathLength={1} className="overview-all-clear-tick" />
        </svg>
        {!reducedMotion && (
          // A new key mounts the pieces again, so each burst plays from the start.
          <span
            key={burst.id}
            className={
              burst.id === 0
                ? "overview-all-clear-confetti overview-all-clear-confetti-first"
                : "overview-all-clear-confetti"
            }
            aria-hidden="true"
          >
            {burst.pieces.map((piece, index) => (
              <svg
                key={index}
                viewBox="0 0 10 10"
                width={CONFETTI_SIZE}
                height={CONFETTI_SIZE}
                fill="none"
                stroke={piece.colour}
                strokeWidth={2}
                strokeLinecap="round"
                strokeLinejoin="round"
                className="overview-all-clear-confetto"
                style={piece.style}
              >
                <path d="M3 5h4" />
              </svg>
            ))}
          </span>
        )}
      </button>
      <p className="type-title-3 font-normal! text-label-secondary">
        <button
          type="button"
          onClick={onShowChecks}
          className="underline decoration-label-tertiary/50 underline-offset-[3px] transition-colors duration-fast hover:text-label hover:decoration-label-secondary"
        >
          All checks
        </button>{" "}
        passed or snoozed
      </p>
    </div>
  )
}
