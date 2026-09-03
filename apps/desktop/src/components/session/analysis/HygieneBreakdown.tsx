import { ChevronRight, CircleCheck, CircleX, type LucideIcon } from "lucide-react"
import { useId, useState } from "react"

import { cn } from "../../../lib/cn"
import {
  sessionHygieneDocumentation,
  sessionHygieneExplainers,
  type SessionHygieneCheck,
} from "../../../lib/presentation/sessionHygiene"
import { RowInfo } from "./RowInfo"

export interface HygieneBreakdownProps {
  checks: SessionHygieneCheck[]
  /**
   * Roll the passing checks up behind their count, so only findings show. Set
   * it false where the surface has room for every check, such as a tab of its
   * own.
   */
  collapsePassing?: boolean
}

interface HygieneStatusPresentation {
  Icon: LucideIcon
  label: string
  textClass: string
}

type AssessedHygieneCheck = SessionHygieneCheck & { status: "finding" | "clean" }

function isAssessed(check: SessionHygieneCheck): check is AssessedHygieneCheck {
  return check.status !== "notAssessed"
}

/* One icon size for every status, so the trailing icon column lines up. */
const STATUS_ICON_SIZE = 14

const STATUS_PRESENTATION: Record<AssessedHygieneCheck["status"], HygieneStatusPresentation> = {
  finding: {
    Icon: CircleX,
    label: "failing",
    textClass: "text-system-red-text",
  },
  clean: {
    Icon: CircleCheck,
    label: "passed",
    textClass: "text-system-green",
  },
}

function HygieneGuidance({ check }: { check: AssessedHygieneCheck }) {
  const documentation = sessionHygieneDocumentation(check)

  return (
    <div className="space-y-1 rounded-control border-x border-b border-separator px-3 pb-3 text-pretty type-callout text-label-secondary">
      {documentation.findingDetails.length > 0 && (
        <div className="mb-2">
          {documentation.findingDetails.map((sentence) => (
            <p key={sentence} className="type-callout font-semibold! text-system-red-text">
              {sentence}
            </p>
          ))}
        </div>
      )}
      <p className="type-callout font-semibold! text-label-secondary">
        {documentation.summary}
      </p>
      {documentation.guidance.length > 0 && (
        <ul className="mt-2 list-disc space-y-0.5 pl-5 type-callout">
          {documentation.guidance.map((sentence) => (
            <li key={sentence} className="mt-1">
              {sentence}
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}

function HygieneRow({
  check,
  explainer,
  open,
  onToggle,
}: {
  check: AssessedHygieneCheck
  /** One sentence on what the check tests, for the row's info button. */
  explainer?: string | undefined
  open: boolean
  onToggle: () => void
}) {
  const bodyId = useId()
  const status = STATUS_PRESENTATION[check.status]

  return (
    <>
      {/* The info button cannot sit inside the row button, so the row is a
          grid that holds the two of them side by side. */}
      <div className="group -mx-1 grid grid-cols-[minmax(0,1fr)_max-content_max-content_max-content] items-center gap-x-2 rounded-control px-1 type-body transition-colors duration-[var(--duration-fast)] ease-out hover:bg-surface-hover">
        <button
          type="button"
          aria-label={`${check.name} details`}
          aria-expanded={open}
          aria-controls={bodyId}
          onClick={onToggle}
          className="col-span-3 grid grid-cols-subgrid items-center gap-x-2 py-1.5 text-left active:transform-none active:opacity-100"
        >
          <span className="truncate text-label-tertiary">{check.name}</span>
          <span className={status.textClass}>{status.label}</span>
          <status.Icon
            size={STATUS_ICON_SIZE}
            strokeWidth={2}
            aria-hidden="true"
            className={cn("shrink-0", status.textClass)}
          />
        </button>
        {explainer && <RowInfo label={check.name} body={explainer} />}
      </div>

      {open && (
        <div
          id={bodyId}
          role="region"
          aria-label={`${check.name} guidance`}
          className="px-1 pb-2"
        >
          <HygieneGuidance check={check} />
        </div>
      )}
    </>
  )
}

export function HygieneBreakdown({ checks, collapsePassing = true }: HygieneBreakdownProps) {
  const [rollupOpen, setRollupOpen] = useState(false)
  const [openCheck, setOpenCheck] = useState<SessionHygieneCheck["id"] | null>(null)
  const assessedChecks = checks.filter(isAssessed)
  const passing = assessedChecks.filter((check) => check.status === "clean")
  const findings = assessedChecks.filter((check) => check.status === "finding")
  const rolledChecks = passing
  const assessedCount = passing.length + findings.length
  const allAssessedPass = passing.length === assessedCount
  const rollupLabel =
    allAssessedPass && assessedCount < checks.length
      ? "All assessed checks passed"
      : `${passing.length}/${assessedCount} passed`

  const explainers = sessionHygieneExplainers()
  const explainerFor = (checkId: SessionHygieneCheck["id"]) =>
    explainers.find((entry) => entry.id === checkId)?.explainer

  const toggleCheck = (checkId: SessionHygieneCheck["id"]) => {
    setOpenCheck((current) => (current === checkId ? null : checkId))
  }

  const toggleRollup = () => {
    if (rollupOpen && rolledChecks.some((check) => check.id === openCheck)) {
      setOpenCheck(null)
    }
    setRollupOpen((current) => !current)
  }

  // Findings lead when every check shows, so a problem is the first thing
  // read. The rolled-up layout puts them last, below the count they sit
  // behind.
  const shownChecks = collapsePassing ? findings : [...findings, ...rolledChecks]

  if (assessedCount === 0) return null

  return (
    <div className="grid gap-y-1" aria-label="Session hygiene checks">
      {/* The full layout skips the rollup words: every row carries its own
          verdict mark, so a count above them restates the list. */}
      {collapsePassing && (
        <button
          type="button"
          aria-expanded={rollupOpen}
          onClick={toggleRollup}
          className="-mx-1 flex items-center justify-between gap-x-3 rounded-control px-1 py-1 text-left type-body text-label-tertiary cursor-pointer! transition-colors duration-[var(--duration-fast)] ease-out hover:bg-surface-hover active:transform-none active:opacity-100"
        >
          <span>{rollupLabel}</span>
          <ChevronRight
            size={14}
            aria-hidden="true"
            className={cn(
              "shrink-0 transition-transform duration-[var(--duration-fast)] ease-out",
              rollupOpen && "rotate-90",
            )}
          />
        </button>
      )}

      {collapsePassing &&
        rollupOpen &&
        rolledChecks.map((check) => (
          <HygieneRow
            key={check.id}
            check={check}
            explainer={explainerFor(check.id)}
            open={openCheck === check.id}
            onToggle={() => toggleCheck(check.id)}
          />
        ))}

      {shownChecks.map((check) => (
        <HygieneRow
          key={check.id}
          check={check}
          explainer={explainerFor(check.id)}
          open={openCheck === check.id}
          onToggle={() => toggleCheck(check.id)}
        />
      ))}
    </div>
  )
}
