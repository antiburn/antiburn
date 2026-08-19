// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { AlertTriangle } from "lucide-react"

import { scoreColor, scoreLabel } from "../../../lib/presentation/sessionAnalytics"

export interface PatternScoreProps {
  /** 0–100 session-health score. */
  score: number
  /** Contributing signals, worst first, as short human sentences. */
  signals: string[]
}

/**
 * The session-health stat: a 0–100 score, its band label, a proportional bar,
 * and the signals that pulled it down.
 *
 * The band is stated in words as well as color, so the reading does not depend
 * on distinguishing green from amber.
 */
export function PatternScore({ score, signals }: PatternScoreProps) {
  const color = scoreColor(score)

  return (
    <div>
      <div className="flex items-baseline gap-2">
        <span className="type-large-title leading-none" style={{ color }}>
          {score}
        </span>
        <span className="type-callout text-label-tertiary">/ 100</span>
        <span className="ml-auto type-callout font-medium" style={{ color }}>
          {scoreLabel(score)}
        </span>
      </div>

      <div className="ui-progress mt-2 h-1.5">
        <div
          className="h-full rounded-full transition-[width] duration-300 ease-out"
          style={{ width: `${score}%`, backgroundColor: color }}
        />
      </div>

      {signals.length > 0 && (
        <ul className="mt-2.5 space-y-1">
          {signals.map((signal) => (
            <li
              key={signal}
              className="flex items-center gap-1.5 type-caption text-label-secondary"
            >
              <AlertTriangle
                size={11}
                aria-hidden="true"
                className="shrink-0 text-system-orange"
              />
              {signal}
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}
