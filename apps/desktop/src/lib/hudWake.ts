/**
 * When a docked HUD should come back on its own.
 *
 * Two signals, both from numbers the HUD already polls. Each starts cold, so
 * a fresh dock never wakes on its first sample.
 */

import { SPEND_CEIL_USD_PER_MIN } from "./ledPeriod"

/** Quiet longer than this, then a new transcript write, is worth a wake. */
export const WAKE_QUIET_SECS = 3600

/**
 * Whether a transcript write at `latest` follows a quiet spell. `previous`
 * is the last write this tracker saw, from events only: a file time can be
 * weeks off.
 */
export function activityWake(previous: number | null, latest: number): boolean {
  return previous != null && latest - previous > WAKE_QUIET_SECS
}

/**
 * Wake once when the spend rate sits at the ceiling for two polls in a row.
 * The rate must fall below the ceiling before the next wake, so a long burn
 * does not wake the HUD every poll.
 */
export class BurnWakeTracker {
  private streak = 0
  private armed = true

  /** Feed one poll. Returns true on the poll that should wake the HUD. */
  observe(usdPerMinute: number | null): boolean {
    if (usdPerMinute == null || usdPerMinute < SPEND_CEIL_USD_PER_MIN) {
      this.streak = 0
      this.armed = true
      return false
    }
    this.streak += 1
    if (this.streak < 2 || !this.armed) return false
    this.armed = false
    return true
  }
}
