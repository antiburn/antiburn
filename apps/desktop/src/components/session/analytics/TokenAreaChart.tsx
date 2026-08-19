// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { Area, AreaChart, ResponsiveContainer, Tooltip, XAxis, YAxis } from "recharts"

import { formatCompact, tokenSeries } from "../../../lib/presentation/sessionAnalytics"
import type { SessionBucket } from "../../../lib/types/session"
import { GLASS_TOOLTIP_STYLE } from "./tooltip"

export interface TokenAreaChartProps {
  buckets: SessionBucket[]
}

/** Tooltip rows keyed by series: the swatch color matches the area stroke. */
const TOKEN_ROWS: Record<string, { label: string; colorVar: string }> = {
  tokensIn: { label: "In", colorVar: "var(--color-token-in)" },
  tokensOut: { label: "Out", colorVar: "var(--color-token-out)" },
}

interface TokenTooltipProps {
  active?: boolean
  label?: number | string
  payload?: Array<{ dataKey?: string | number; value?: number | string }>
}

/** Legible label-token text with a swatch per series row. */
function TokenTooltip({ active, label, payload }: TokenTooltipProps) {
  if (!active || !payload?.length) return null
  return (
    <div
      className="text-label"
      style={{
        ...GLASS_TOOLTIP_STYLE,
        lineHeight: 1.4,
        padding: "6px 9px",
        whiteSpace: "nowrap",
      }}
    >
      <div className="mb-1">{label}% through</div>
      <div className="flex flex-col gap-1">
        {payload.map((entry) => {
          const row = TOKEN_ROWS[String(entry.dataKey)]
          if (!row) return null
          return (
            <div key={row.label} className="flex items-center gap-1.5 text-label-secondary">
              <span
                className="h-2 w-2 shrink-0 rounded-full"
                style={{ backgroundColor: row.colorVar }}
              />
              <span>
                {row.label} : {formatCompact(Number(entry.value))}
              </span>
            </div>
          )
        })}
      </div>
    </div>
  )
}

/** Stacked area of input against output tokens across session progress. */
export function TokenAreaChart({ buckets }: TokenAreaChartProps) {
  const data = tokenSeries(buckets)

  return (
    <ResponsiveContainer width="100%" height={110}>
      <AreaChart data={data} margin={{ top: 4, right: 4, bottom: 0, left: 0 }}>
        <XAxis dataKey="progress" hide />
        <YAxis hide />
        <Tooltip cursor={{ stroke: "var(--color-separator)" }} content={<TokenTooltip />} />
        <Area
          type="monotone"
          dataKey="tokensIn"
          stackId="t"
          stroke="var(--color-token-in)"
          fill="var(--color-token-in)"
          fillOpacity={0.5}
          isAnimationActive={false}
        />
        <Area
          type="monotone"
          dataKey="tokensOut"
          stackId="t"
          stroke="var(--color-token-out)"
          fill="var(--color-token-out)"
          fillOpacity={0.6}
          isAnimationActive={false}
        />
      </AreaChart>
    </ResponsiveContainer>
  )
}
