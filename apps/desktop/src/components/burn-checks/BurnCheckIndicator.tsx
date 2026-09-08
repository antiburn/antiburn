import { Check, CircleAlert, CircleDashed, CircleMinus } from "lucide-react"

import type { BurnCheckPresentation } from "../../lib/presentation/burnChecks"
import { SegmentedRadialDial } from "../ui/SegmentedRadialDial"

const SEGMENT_CLASS = {
  failed: "text-burn-check-failure-fill",
  passed: "text-burn-check-pass-fill",
  unassessed: "text-burn-check-neutral",
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
    return (
      <SegmentedRadialDial
        size={size}
        strokeWidth={size <= 16 ? 2.5 : 3}
        gapAngle={5}
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
    const markSize = size <= 16 ? size - 2 : size - 4
    return (
      <span
        {...common}
        data-burn-check-indicator="pass"
        className="inline-grid shrink-0 place-items-center rounded-full bg-burn-check-pass-fill text-white"
        style={{ width: markSize, height: markSize, margin: (size - markSize) / 2 }}
      >
        <Check
          size={Math.max(7, Math.round(markSize * 0.5))}
          strokeWidth={2.5}
          aria-hidden="true"
        />
      </span>
    )
  }

  const Icon =
    presentation.indicator.kind === "fail"
      ? CircleAlert
      : presentation.indicator.kind === "running"
        ? CircleDashed
        : CircleMinus
  const tone =
    presentation.indicator.kind === "fail"
      ? "text-burn-check-failure-fill"
      : "text-burn-check-neutral"
  return (
    <Icon
      {...common}
      data-burn-check-indicator={presentation.indicator.kind}
      size={size}
      strokeWidth={presentation.indicator.kind === "fail" ? 2.5 : 2}
      className={`shrink-0 ${tone}`}
    />
  )
}
