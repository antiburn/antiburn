// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { Check } from "lucide-react"

import { cn } from "../../lib/cn"
import type { MockSessionHygieneCheck } from "../../lib/presentation/mockSessionHygiene"

/** The names of the failed checks, in the order the check set declares them. */
function failedShortTitles(checks: MockSessionHygieneCheck[]): string[] {
  return checks.filter((check) => !check.passed).map((check) => check.shortTitle)
}

/** The whole result as one sentence, for assistive tech. */
function statusSentence(checks: MockSessionHygieneCheck[]): string {
  const failed = failedShortTitles(checks)
  if (failed.length === 0) {
    return `${checks.length} of ${checks.length} checks passed`
  }
  return `${failed.length} of ${checks.length} checks failed: ${failed.join(", ")}`
}

/**
 * The hygiene result for one session row, at the start of the models line.
 *
 * A row that passes every check shows a green check. A row with a failure
 * shows the count of failures in red, which stays visible at rest.
 *
 * The label after the mark is hidden until the row is hovered.
 * `session-rows.css` opens it, and the row must carry the `session-row` class
 * for that hover rule to find it. The label is hidden from assistive tech,
 * because this element's `aria-label` already carries the full sentence.
 */
export function SessionHygieneStatus({ checks }: { checks: MockSessionHygieneCheck[] }) {
  const failedCount = checks.filter((check) => !check.passed).length
  const passed = failedCount === 0

  return (
    <span
      aria-label={statusSentence(checks)}
      className={cn(
        "session-hygiene-status flex shrink-0 items-center type-footnote",
        passed ? "text-system-green" : "text-system-red",
      )}
    >
      {passed ? (
        <>
          <Check size={12} strokeWidth={3} className="shrink-0" aria-hidden="true" />
          <span aria-hidden="true" className="session-hygiene-reveal">
            <span>
              {/* The gap sits inside the track, so it collapses with it. */}
              <span className="block pl-[5px] font-medium whitespace-nowrap">
                {checks.length}/{checks.length} checks passed
              </span>
            </span>
          </span>
        </>
      ) : (
        <span aria-hidden="true" className="flex items-center gap-x-[5px] whitespace-nowrap">
          <span className="font-semibold tabular-nums">
            {failedCount}/{checks.length}
          </span>
          <span className="font-medium">checks failed</span>
        </span>
      )}
    </span>
  )
}

/**
 * The names of the failed checks, on a line of their own below the models.
 *
 * The line is closed at rest and opens on row hover. It renders nothing when
 * every check passes. `SessionHygieneStatus` names the same checks in its
 * `aria-label`, so this line stays out of the accessibility tree.
 */
export function SessionHygieneFailureLine({ checks }: { checks: MockSessionHygieneCheck[] }) {
  const failed = failedShortTitles(checks)
  if (failed.length === 0) return null

  return (
    <div aria-hidden="true" className="session-hygiene-failures type-footnote text-system-red">
      <span>
        <span className="block truncate pt-1 font-medium">{failed.join(", ")}</span>
      </span>
    </div>
  )
}
