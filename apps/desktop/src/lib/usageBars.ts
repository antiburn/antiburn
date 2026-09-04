import { siClaude } from "simple-icons"

import type { LiveUsageSummaryPayload } from "./ipc"
import {
  liveDisplayableProviders,
  liveWindowElapsed,
  liveWindowLabel,
  liveWindows,
} from "./presentation/liveUsage"

export type UsageBarItem = {
  key: string
  label: string
  percent: number
  resetsAt: Date | null
  color: string
  /** Elapsed share of the window's period, 0-1, or null when unknown. */
  expectedFraction: number | null
}

/**
 * Anthropic keeps its own colour, taken from the hex the `simple-icons`
 * package records for the Claude mark. The brand-mark rule in `design.md`
 * applies: a vendor colour comes from the source package, never from a hex
 * written here. The other providers use antiburn's own palette.
 */
const CLAUDE_BRAND = `#${siClaude.hex}`

const PROVIDER_COLORS: Record<string, string> = {
  anthropic: CLAUDE_BRAND,
  claude: CLAUDE_BRAND,
  openai: "var(--color-label)",
  cursor: "var(--color-system-indigo)",
  google: "var(--color-system-blue)",
  opencode_go: "var(--color-system-green)",
}

export function providerBarColor(provider: string): string {
  return PROVIDER_COLORS[provider.toLowerCase()] ?? "var(--color-burn)"
}

function resetDate(resetsAt: string | null): Date | null {
  if (!resetsAt) return null
  const date = new Date(resetsAt)
  return Number.isNaN(date.getTime()) ? null : date
}

/**
 * True when the reader turned off every meter antiburn can show.
 *
 * The other reason for an empty HUD is that no provider reported anything.
 * The two look alike on screen and are not alike: one is a choice the reader
 * made, and the surfaces say so rather than reading as a failure.
 */
export function noMeterSelected(response: LiveUsageSummaryPayload | null): boolean {
  // Only a roster that arrived and is wholly off proves the choice. No
  // snapshot, or a snapshot from a build that predates the roster, means
  // antiburn cannot say — and it must not claim the reader chose this.
  if (!response || response.meters.length === 0) return false
  return response.meters.every((meter) => !meter.shown)
}

/** Convert a live usage snapshot into the HUD bar order. */
export function deriveUsageBars(response: LiveUsageSummaryPayload | null): UsageBarItem[] {
  const withBars = (response ? liveDisplayableProviders(response) : [])
    .map((provider) => ({
      provider,
      windows: liveWindows(provider).filter((window) => window.usedPercent != null),
    }))
    .filter((group) => group.windows.length > 0)

  const multiProvider = withBars.length > 1
  // The notch measures from the snapshot's own time, not the wall clock. A
  // snapshot that states no usable time gets no notch, because a notch drawn
  // from an assumed time is a claim the provider did not make.
  const generatedAt = response ? Date.parse(response.generatedAt) : Number.NaN

  return withBars.flatMap((group) =>
    group.windows.map((window) => ({
      key: `${group.provider.provider}-${window.id}`,
      label: multiProvider
        ? `${group.provider.displayName} · ${liveWindowLabel(window)}`
        : liveWindowLabel(window),
      percent: window.usedPercent!,
      resetsAt: resetDate(window.resetsAt),
      color: providerBarColor(group.provider.provider),
      expectedFraction: Number.isNaN(generatedAt)
        ? null
        : liveWindowElapsed(window, generatedAt),
    })),
  )
}

const CLOCK_HORIZON_MS = 12 * 3_600_000

function durationUntil(resetsAt: Date, now: number): string {
  const minutes = Math.ceil((resetsAt.getTime() - now) / 60_000)
  if (minutes < 60) return `${minutes}m`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) {
    const remaining = minutes % 60
    return remaining > 0 ? `${hours}h ${remaining}m` : `${hours}h`
  }
  const days = Math.floor(hours / 24)
  const remainingHours = hours % 24
  return remainingHours > 0 ? `${days}d ${remainingHours}h` : `${days}d`
}

export function resetsLabel(resetsAt: Date | null, now: number): string {
  if (!resetsAt) return "reset unknown"
  const milliseconds = resetsAt.getTime() - now
  if (milliseconds <= 0) return "resets soon"
  if (milliseconds > CLOCK_HORIZON_MS) return `resets in ${durationUntil(resetsAt, now)}`
  const clock = resetsAt
    .toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" })
    .replace(/\s+/g, "")
    .toLowerCase()
  return `resets ${clock}`
}

export function resetsIn(resetsAt: Date | null, now: number): string {
  if (!resetsAt) return "reset unknown"
  const milliseconds = resetsAt.getTime() - now
  if (milliseconds <= 0) return "resets soon"
  return `resets in ${durationUntil(resetsAt, now)}`
}
