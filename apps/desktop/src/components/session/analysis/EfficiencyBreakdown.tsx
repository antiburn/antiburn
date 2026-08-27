import { Info } from "lucide-react"

import { cn } from "../../../lib/cn"
import {
  efficiencyBandWord,
  efficiencyMetricDescription,
  efficiencyThermometer,
  efficiencyThresholdsText,
  formatCostPerMTok,
  formatSharePercent,
  type EfficiencyBand,
  type EfficiencyMetric,
  type EfficiencyMetrics,
  type EfficiencyProfile,
} from "../../../lib/presentation/sessionEfficiency"
import { Tooltip } from "../../presentation/Tooltip"

export interface EfficiencyBreakdownProps {
  metrics: EfficiencyMetrics
}

const BAND_TEXT_CLASS: Record<EfficiencyBand, string> = {
  good: "text-system-green",
  ok: "text-label-tertiary",
  bad: "text-system-orange",
}

const BAND_SEGMENT_CLASS: Record<EfficiencyBand, string> = {
  good: "bg-system-green/50",
  ok: "bg-separator",
  bad: "bg-system-orange/50",
}

type MetricKey = "costPerMTok" | "realWorkShare" | "rewriteShare"

const ROWS: { key: MetricKey; label: string; format: (value: number) => string }[] = [
  { key: "costPerMTok", label: "$/MTok", format: formatCostPerMTok },
  { key: "realWorkShare", label: "Real Work %", format: formatSharePercent },
  { key: "rewriteShare", label: "Rewrite Waste %", format: formatSharePercent },
]

function Thermometer({
  value,
  metricKey,
  profile,
  title,
}: {
  value: number
  metricKey: MetricKey
  profile: EfficiencyProfile
  title: string
}) {
  const scale = efficiencyThermometer(value, metricKey, profile)
  return (
    <div
      className="relative flex h-full items-center"
      title={title}
      data-testid={`thermometer-${metricKey}`}
      data-position={scale.position.toFixed(3)}
      aria-hidden
    >
      {scale.segments.map((segment, index) => (
        <span
          key={index}
          className={cn(
            "h-1.5 flex-1",
            index === 0 && "rounded-s-full",
            index === scale.segments.length - 1 && "rounded-e-full",
            BAND_SEGMENT_CLASS[segment],
          )}
        />
      ))}
      <span
        className="absolute inset-y-1 w-1.5 -translate-x-1/2 rounded-full border border-label bg-surface opacity-80 dark:bg-separator"
        style={{ left: `${scale.position * 100}%` }}
      />
    </div>
  )
}

function RowInfo({ label, metricKey }: { label: string; metricKey: MetricKey }) {
  return (
    <Tooltip
      label={efficiencyMetricDescription(metricKey)}
      side="top"
      interactive
      delayMs={150}
    >
      <button
        type="button"
        aria-label={`About ${label}`}
        className="leading-none text-label-tertiary transition-colors duration-[var(--duration-fast)] ease-out hover:text-label-secondary"
      >
        <Info size={12} aria-hidden="true" />
      </button>
    </Tooltip>
  )
}

function EfficiencyRowLine({
  label,
  metric,
  metricKey,
  format,
  profile,
}: {
  label: string
  metric: EfficiencyMetric | null
  metricKey: MetricKey
  format: (value: number) => string
  profile: EfficiencyProfile
}) {
  return (
    <div className="col-span-4 grid grid-cols-subgrid items-center gap-x-3 -mx-1 px-1 py-1 rounded-control type-caption transition-colors duration-[var(--duration-fast)] ease-out hover:bg-surface-hover">
      <span className="flex items-center gap-1 text-label-tertiary">
        {label}
        <RowInfo label={label} metricKey={metricKey} />
      </span>
      {metric ? (
        <Thermometer
          value={metric.value}
          metricKey={metricKey}
          profile={profile}
          title={efficiencyThresholdsText(metricKey, profile)}
        />
      ) : (
        <span />
      )}
      <span className="text-right text-label tabular-nums">
        {metric ? format(metric.value) : "—"}
      </span>
      {metric && (
        <span className={cn("type-caption whitespace-nowrap", BAND_TEXT_CLASS[metric.band])}>
          {efficiencyBandWord(metric.band, metricKey)}
        </span>
      )}
    </div>
  )
}

export function EfficiencyBreakdown({ metrics }: EfficiencyBreakdownProps) {
  return (
    <div className="grid grid-cols-[max-content_1fr_max-content_max-content] gap-y-1 mt-2 pt-2 border-t border-separator">
      {ROWS.map((row) => (
        <EfficiencyRowLine
          key={row.key}
          label={row.label}
          metric={metrics[row.key]}
          metricKey={row.key}
          format={row.format}
          profile={metrics.profile}
        />
      ))}
    </div>
  )
}
