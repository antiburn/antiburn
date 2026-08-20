// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

/**
 * Format an ISO 8601 timestamp as a human-readable relative time — `"12s ago"`,
 * `"3m ago"`, `"2h ago"`, `"4d ago"`.
 *
 * Pass `{ compact: true }` to drop the trailing `" ago"`, for a place that is
 * already labelled as a time, such as a list row's activity time.
 *
 * A missing timestamp reads `"never"`; anything under five seconds, including
 * a clock skewed into the future, reads `"just now"` (`"now"` when compact).
 */
export function relativeTime(
  isoString: string | null | undefined,
  options?: { compact?: boolean },
): string {
  if (!isoString) return "never"
  const ms = Date.now() - new Date(isoString).getTime()
  if (Number.isNaN(ms)) return "never"
  if (ms < 0) return options?.compact ? "now" : "just now"
  const secs = Math.floor(ms / 1000)
  if (secs < 5) return options?.compact ? "now" : "just now"
  const suffix = options?.compact ? "" : " ago"
  if (secs < 60) return `${secs}s${suffix}`
  const mins = Math.floor(secs / 60)
  if (mins < 60) return `${mins}m${suffix}`
  const hours = Math.floor(mins / 60)
  if (hours < 24) return `${hours}h${suffix}`
  const days = Math.floor(hours / 24)
  return `${days}d${suffix}`
}
