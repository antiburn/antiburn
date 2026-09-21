const PIECES = 14
const MODES = ["looking", "running", "changing", "delegating", "thinking", "other"] as const

/**
 * A short burst of confetti over the parent. The parent is `relative`; the
 * pieces rise, drift, and fade in `hud.css` (`hud-confetti`). Pure CSS: it
 * plays once on mount and leaves nothing behind.
 */
export function Confetti() {
  return (
    <div aria-hidden="true" className="pointer-events-none absolute inset-0 overflow-visible">
      {Array.from({ length: PIECES }, (_, index) => (
        <span
          key={index}
          className="hud-confetti absolute bottom-0 h-1.5 w-1"
          style={{
            left: `${(index + 0.5) * (100 / PIECES)}%`,
            backgroundColor: `var(--color-mode-${MODES[index % MODES.length]})`,
            animationDelay: `${(index % 4) * 60}ms`,
            // Alternate the drift, so the burst spreads both ways.
            ["--confetti-drift" as string]: `${index % 2 === 0 ? -1 : 1}`,
          }}
        />
      ))}
    </div>
  )
}
