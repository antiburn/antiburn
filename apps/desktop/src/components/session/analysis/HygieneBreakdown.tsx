import { Check, ChevronRight, X, type LucideIcon } from "lucide-react"
import { useId, useState } from "react"

import { cn } from "../../../lib/cn"
import {
  sessionHygieneDocumentation,
  type SessionHygieneCheck,
} from "../../../lib/presentation/sessionHygiene"

export interface HygieneBreakdownProps {
  checks: SessionHygieneCheck[]
}

interface HygieneStatusPresentation {
  Icon: LucideIcon
  iconSize: number
  label: string
  strokeWidth: number
  textClass: string
}

type AssessedHygieneCheck = SessionHygieneCheck & { status: "finding" | "clean" }

function isAssessed(check: SessionHygieneCheck): check is AssessedHygieneCheck {
  return check.status !== "notAssessed"
}

const STATUS_PRESENTATION: Record<AssessedHygieneCheck["status"], HygieneStatusPresentation> = {
  finding: {
    Icon: X,
    iconSize: 12,
    label: "failing",
    strokeWidth: 2.5,
    textClass: "text-system-red-text",
  },
  clean: {
    Icon: Check,
    iconSize: 12,
    label: "passed",
    strokeWidth: 2.5,
    textClass: "text-system-green",
  },
}

function HygieneGuidance({ check }: { check: AssessedHygieneCheck }) {
  const documentation = sessionHygieneDocumentation(check)

  return (
    <div className="space-y-1 rounded-control border-x border-b border-separator px-3 pb-3 text-pretty type-footnote text-label-secondary">
      {documentation.findingDetails.length > 0 && (
        <div className="mb-2">
          {documentation.findingDetails.map((sentence) => (
            <p key={sentence} className="type-footnote font-semibold! text-system-red-text">
              {sentence}
            </p>
          ))}
        </div>
      )}
      <p className="type-footnote font-semibold! text-label-secondary">
        {documentation.summary}
      </p>
      {documentation.guidance.length > 0 && (
        <ul className="mt-2 list-disc space-y-0.5 pl-5 text-sm/5">
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
  open,
  onToggle,
}: {
  check: AssessedHygieneCheck
  open: boolean
  onToggle: () => void
}) {
  const bodyId = useId()
  const status = STATUS_PRESENTATION[check.status]

  return (
    <>
      <button
        type="button"
        aria-label={`${check.name} details`}
        aria-expanded={open}
        aria-controls={bodyId}
        onClick={onToggle}
        className="-mx-1 grid grid-cols-[minmax(0,1fr)_max-content] items-center gap-x-3 rounded-control px-1 py-1 text-left type-caption transition-colors duration-[var(--duration-fast)] ease-out hover:bg-surface-hover active:transform-none active:opacity-100"
      >
        <span className="truncate text-label-tertiary">{check.name}</span>
        <span className={cn("inline-flex items-center gap-1", status.textClass)}>
          <status.Icon
            size={status.iconSize}
            strokeWidth={status.strokeWidth}
            aria-hidden="true"
          />
          <span>{status.label}</span>
        </span>
      </button>

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

export function HygieneBreakdown({ checks }: HygieneBreakdownProps) {
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

  const toggleCheck = (checkId: SessionHygieneCheck["id"]) => {
    setOpenCheck((current) => (current === checkId ? null : checkId))
  }

  const toggleRollup = () => {
    if (rollupOpen && rolledChecks.some((check) => check.id === openCheck)) {
      setOpenCheck(null)
    }
    setRollupOpen((current) => !current)
  }

  if (assessedCount === 0) return null

  return (
    <div className="grid gap-y-1" aria-label="Session hygiene checks">
      <button
        type="button"
        aria-expanded={rollupOpen}
        onClick={toggleRollup}
        className="-mx-1 flex items-center justify-between gap-x-3 rounded-control px-1 py-1 text-left type-caption text-label-tertiary cursor-pointer! transition-colors duration-[var(--duration-fast)] ease-out hover:bg-surface-hover active:transform-none active:opacity-100"
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

      {rollupOpen &&
        rolledChecks.map((check) => (
          <HygieneRow
            key={check.id}
            check={check}
            open={openCheck === check.id}
            onToggle={() => toggleCheck(check.id)}
          />
        ))}

      {findings.map((check) => (
        <HygieneRow
          key={check.id}
          check={check}
          open={openCheck === check.id}
          onToggle={() => toggleCheck(check.id)}
        />
      ))}
    </div>
  )
}
