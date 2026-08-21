// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { cn } from "../../lib/cn"
import type { LiveUsageWindowPayload } from "../../lib/ipc"
import { liveMetricRows, liveWindowLabel } from "../../lib/presentation/liveUsage"

/**
 * What one limit window's history says about it — how fast, faster than
 * before?, will it last, how much was today.
 *
 * The rows come from `liveMetricRows`, which keeps their order fixed and
 * gives an unavailable figure its reason rather than dropping the row.
 *
 * A description list carries no accessible name of its own. The explicit
 * group role is what tells a screen-reader user which window these rows
 * describe.
 */
export function LiveMetricRows({
  window,
  now,
  className = "",
}: {
  window: LiveUsageWindowPayload
  /** Injected so the rendered output is a function of its inputs in tests. */
  now: number
  className?: string
}) {
  return (
    <dl
      role="group"
      aria-label={`${liveWindowLabel(window)} pace`}
      className={cn("space-y-0.5", className)}
    >
      {liveMetricRows(window, now).map((row) => (
        <div key={row.key} className="flex items-baseline justify-between gap-3">
          <dt className="type-caption text-label-tertiary">{row.label}</dt>
          <dd className={cn("type-caption tabular-nums", row.toneClass)}>{row.value}</dd>
        </div>
      ))}
    </dl>
  )
}
