import { CircleDashed } from "lucide-react"

import type { BurnCheckPresentation } from "../../lib/presentation/burnChecks"
import { SegmentedRadialDial } from "../ui/SegmentedRadialDial"
import { BURN_CHECK_MARKS } from "./burnCheckMarks"

const SEGMENT_CLASS = {
  failed: BURN_CHECK_MARKS.finding.iconClass,
  passed: BURN_CHECK_MARKS.clean.iconClass,
  unassessed: BURN_CHECK_MARKS.notAssessed.iconClass,
} as const

export function BurnCheckIndicator({
  presentation,
  size,
  labelled = false,
}: {
  presentation: BurnCheckPresentation
  size: number
  labelled?: boolean
}) {
  const label = labelled ? presentation.accessibleDescription : undefined
  const common = {
    role: labelled ? ("img" as const) : undefined,
    "aria-label": label,
    "aria-hidden": labelled ? undefined : (true as const),
  }

  if (presentation.indicator.kind === "segments") {
    const dialSize = size <= 16 ? size - 2 : size
    return (
      <SegmentedRadialDial
        size={dialSize}
        strokeWidth={size <= 16 ? 1.5 : 3}
        gapAngle={size <= 16 ? 14 : 5}
        {...(label ? { label } : {})}
        segments={presentation.indicator.segments.map((segment) => ({
          id: segment.outcome,
          value: segment.value,
          className: SEGMENT_CLASS[segment.outcome],
        }))}
      />
    )
  }

  if (presentation.indicator.kind === "pass") {
    const mark = BURN_CHECK_MARKS.clean
    const markSize = size <= 16 ? size - 1 : size - 4
    return (
      <mark.Icon
        {...common}
        data-burn-check-indicator="pass"
        size={markSize}
        strokeWidth={mark.strokeWidth}
        className={`shrink-0 ${mark.iconClass}`}
      />
    )
  }

  const mark =
    presentation.indicator.kind === "fail"
      ? BURN_CHECK_MARKS.finding
      : BURN_CHECK_MARKS.notAssessed
  const Icon = presentation.indicator.kind === "running" ? CircleDashed : mark.Icon
  const markSize = size <= 16 ? size - 1 : size
  return (
    <Icon
      {...common}
      data-burn-check-indicator={presentation.indicator.kind}
      size={markSize}
      strokeWidth={mark.strokeWidth}
      className={`shrink-0 ${mark.iconClass}`}
    />
  )
}
