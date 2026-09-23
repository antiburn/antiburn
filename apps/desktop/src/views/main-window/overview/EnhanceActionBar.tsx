import { Sparkles } from "lucide-react"

/** The bar under the Overview content. It gives the reader one clear next
 *  step: open the Enhance wizard. */
export function EnhanceActionBar({
  failingChecks,
  onOpen,
}: {
  /** Failing checks that are not snoozed, or null while the report loads. */
  failingChecks: number | null
  onOpen: () => void
}) {
  return (
    <footer
      aria-label="Next step"
      className="grid grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)] items-center gap-(--space-md) border-t border-separator pt-(--space-lg)"
    >
      <p role="status" className="type-caption text-label-secondary">
        {failingChecks == null ? (
          "Checking your setup…"
        ) : failingChecks === 0 ? (
          <>
            <span className="font-semibold text-label">No fixes needed</span> in your last 30
            days.
          </>
        ) : (
          <>
            <span className="font-semibold text-label">
              {failingChecks} {failingChecks === 1 ? "fix" : "fixes"} found
            </span>{" "}
            in your last 30 days.
          </>
        )}
      </p>

      <button type="button" onClick={onOpen} className="overview-enhance">
        <span aria-hidden="true" className="overview-enhance-glow" />
        <span className="overview-enhance-face flex items-center gap-(--space-sm) rounded-(--radius-popover) py-(--space-sm) ps-(--space-sm) pe-(--space-lg) type-body font-semibold whitespace-nowrap text-white">
          <span
            aria-hidden="true"
            className="overview-enhance-icon grid size-6 place-items-center rounded-control"
          >
            <Sparkles size={14} strokeWidth={2} />
          </span>
          Enhance my AI setup
        </span>
      </button>
    </footer>
  )
}
