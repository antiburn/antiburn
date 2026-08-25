// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { cn } from "../../../lib/cn"
import { Info } from "lucide-react"

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

const BAND_MARK_CLASS: Record<EfficiencyBand, string> = {
  good: "bg-system-green",
  ok: "bg-label",
  bad: "bg-system-orange",
}

type MetricKey = "costPerMTok" | "newWorkShare" | "rewriteShare"

const ROWS: { key: MetricKey; label: string; format: (value: number) => string }[] = [
  { key: "costPerMTok", label: "$/MTok", format: formatCostPerMTok },
  { key: "newWorkShare", label: "New Work %", format: formatSharePercent },
  { key: "rewriteShare", label: "Rewrite %", format: formatSharePercent },
]

function Thermometer({
  value,
  band,
  metricKey,
  profile,
}: {
  value: number
  band: EfficiencyBand
  metricKey: MetricKey
  profile: EfficiencyProfile
}) {
  const scale = efficiencyThermometer(value, metricKey, profile)
  return (
    <div
      className="relative flex h-1.5 overflow-hidden rounded-full"
      data-testid={`thermometer-${metricKey}`}
      data-position={scale.position.toFixed(3)}
      aria-hidden
    >
      {scale.segments.map((segment, index) => (
        <span key={index} className={`flex-1 ${BAND_SEGMENT_CLASS[segment]}`} />
      ))}
      <span
        className={`absolute inset-y-0 w-0.5 -translate-x-1/2 ${BAND_MARK_CLASS[band]}`}
        style={{ left: `${scale.position * 100}%` }}
      />
    </div>
  )
}

/** The info mark after a row label. Its tooltip explains the metric and its bands. */
function RowInfo({
  label,
  metricKey,
  profile,
}: {
  label: string
  metricKey: MetricKey
  profile: EfficiencyProfile
}) {
  const text = `${efficiencyMetricDescription(metricKey)} ${efficiencyThresholdsText(metricKey, profile)}`
  return (
    <Tooltip label={text} side="top" interactive delayMs={150}>
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
        <RowInfo label={label} metricKey={metricKey} profile={profile} />
      </span>
      {metric ? (
        <Thermometer
          value={metric.value}
          band={metric.band}
          metricKey={metricKey}
          profile={profile}
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
