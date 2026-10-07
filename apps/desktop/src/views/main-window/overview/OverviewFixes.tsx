import { Circle, CircleAlert, CircleCheck } from "lucide-react"

import { cn } from "../../../lib/cn"
import type { BurnCheckDetectorId } from "../../../lib/insightsIpc"
import { CHECK_PROBLEM_PHRASES } from "../../../lib/presentation/checkDefinitions"
import { type FixCategory, type FixStatus } from "./overviewProgressStore"
import { useOverviewProgress } from "./useOverviewProgress"

function statusLabel(status: FixStatus): string {
  switch (status) {
    case "needsFix":
      return "needs fix"
    case "awaitingVerification":
      return "awaiting verification"
    case "passing":
      return "passing"
    case "notChecked":
      return "not checked"
    case "snoozed":
      return "snoozed"
  }
}

/**
 * The persistent config checks list. The first-run takeover and its docked
 * steps and fixes result live in `FirstRunTakeover.tsx` and
 * `ProgressNav.tsx`; this keeps only the steady checklist, in its permanent
 * style, so it shows the same way in `firstRun` and `steady` mode alike.
 */
export function OverviewFixes({
  onOpenCheck,
}: {
  onOpenCheck: (check: BurnCheckDetectorId) => void
}) {
  const progress = useOverviewProgress()

  return (
    <section aria-label="Fixes" className="flex flex-col gap-(--space-sm)">
      <h2 className="type-caption text-label-secondary">Config checks</h2>
      <ul className="flex flex-col gap-1">
        {progress.categories.map((category) => (
          <CheckRow key={category.id} category={category} onOpen={onOpenCheck} />
        ))}
      </ul>
    </section>
  )
}

function CheckRow({
  category,
  onOpen,
}: {
  category: FixCategory
  onOpen: (check: BurnCheckDetectorId) => void
}) {
  const needsFix = category.status === "needsFix"
  const phrase = needsFix ? CHECK_PROBLEM_PHRASES[category.id] : null
  return (
    <li>
      <button
        type="button"
        onClick={() => onOpen(category.id)}
        className={cn(
          // An inset ring, so the scroll pane cannot clip the row's edges.
          "flex w-full items-center gap-3 rounded-(--radius-popover) px-3 py-1.5 text-start transition-[filter] duration-fast hover:brightness-97 active:brightness-95",
          needsFix
            ? "bg-brand-tint/12 ring-1 ring-inset ring-brand-tint/40"
            : "bg-session-card",
        )}
      >
        {needsFix ? (
          <CircleAlert size={16} strokeWidth={2} className="shrink-0 text-brand" />
        ) : category.status === "passing" ? (
          <CircleCheck size={16} strokeWidth={2} className="shrink-0 text-label-tertiary" />
        ) : (
          <Circle size={16} strokeWidth={2} className="shrink-0 text-label-tertiary" />
        )}
        <span className="flex min-w-0 items-baseline gap-2">
          <span
            className={cn(
              "shrink-0 type-body font-medium!",
              needsFix ? "text-label" : "text-label-secondary",
            )}
          >
            {category.label}
          </span>
          {phrase && (
            <span className="truncate type-footnote text-label-tertiary">{phrase}</span>
          )}
        </span>

        <span
          className={cn(
            "ms-auto shrink-0 font-mono type-metadata",
            needsFix ? "text-brand" : "text-label-tertiary",
          )}
        >
          {statusLabel(category.status)}
        </span>
      </button>
    </li>
  )
}
