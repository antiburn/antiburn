/**
 * Shaping and wording for provider-reported usage limits.
 *
 * The sibling module `providerUsage.ts` shapes the *estimate* surface, and its
 * whole job is to stop a spend figure reading as a budget. This module has the
 * opposite problem: these numbers really are a budget, so the work is stopping
 * them from reading as more current than they are.
 *
 * Two rules follow from that, and everything below is one of them:
 *
 * 1. **Say when it was true.** Every reading carries the moment the provider
 *    stated it, and a reading past its age budget says so in as many words
 *    rather than quietly ageing on screen.
 * 2. **Never fill a gap.** A window with no percentage renders as unknown, not
 *    as zero; a window with no reset says the reset is unavailable rather than
 *    guessing one from the window's name.
 *
 * Nothing here fetches, and nothing here can invent a number — the payload is
 * the provider's own statement or null.
 */

import type {
  LiveProviderUsagePayload,
  LiveUsageFreshness,
  LiveUsagePlanPayload,
  LiveUsageSourceErrorPayload,
  LiveUsageSummaryPayload,
  LiveUsageWindowPayload,
} from "../ipc"

/** A provider whose live reading failed and left nothing to show. */
export interface UnavailableLiveProvider {
  /** Canonical provider id, from the failed source. */
  provider: string
  displayName: string
  /** `authentication` | `rateLimited` | `schema` | `unavailable`. */
  category: string
}
import { relativeTime } from "./relativeTime"

/** Canonical provider ids, matching the Rust `provider_usage::providers` constants. */
const ANTHROPIC = "anthropic"
const GOOGLE = "google"
const OPENAI = "openai"

/** How long a failed source's last good reading still stands in for a live one. */
export const LIVE_USAGE_GRACE_MS = 10 * 60_000

/** Provider window ids whose product names carry the clearest label. */
const WINDOW_LABEL_BY_ID: Readonly<Record<string, string>> = {
  "five-hour": "5-hour limit",
  "seven-day": "Weekly limit",
  "antigravity-gemini-5h": "Gemini 5-hour limit",
  "antigravity-gemini-weekly": "Gemini weekly limit",
  "antigravity-claude-gpt-5h": "Claude and GPT 5-hour limit",
  "antigravity-claude-gpt-weekly": "Claude and GPT weekly limit",
}

/** Claude plan names that resolve to a fixed label with no tier to read. */
const CLAUDE_PLAN_NAME_LABELS: Readonly<Record<string, string>> = {
  pro: "Pro",
  team: "Team",
  enterprise: "Enterprise",
}

/**
 * Claude's `max` tiers, matched by substring because the raw values carry
 * extra prefixes (for example `default_claude_max_5x`) that this label does
 * not repeat.
 */
const CLAUDE_MAX_TIER_LABELS: ReadonlyArray<{ substring: string; label: string }> = [
  { substring: "max_5x", label: "Max 5x" },
  { substring: "max_20x", label: "Max 20x" },
]

/** Codex plan names, mapped to the words this app shows for them. */
const CODEX_PLAN_NAME_LABELS: Readonly<Record<string, string>> = {
  free: "Free",
  go: "Go",
  plus: "Plus",
  pro: "Pro",
  prolite: "Pro Lite",
  team: "Team",
  business: "Business",
  self_serve_business_prolite: "Business",
  self_serve_business_usage_based: "Business",
  enterprise: "Enterprise",
  ent26: "Enterprise",
  enterprise_cbp_automation: "Enterprise",
  enterprise_cbp_usage_based: "Enterprise",
  edu: "Edu",
  edu_plus: "Edu Plus",
  edu_pro: "Edu Pro",
}

const GOOGLE_PLAN_NAME_LABELS: Readonly<Record<string, string>> = {
  free: "Free",
  pro: "Pro",
  ultra: "Ultra",
  "google ai pro": "Google AI Pro",
  "google ai ultra": "Google AI Ultra",
}

const ANTIGRAVITY_PRIMARY_WINDOW_IDS = new Set([
  "antigravity-gemini-5h",
  "antigravity-gemini-weekly",
  "antigravity-claude-gpt-5h",
  "antigravity-claude-gpt-weekly",
])

/** Claude's plan label, reading the tier only for the `max` name. */
function claudePlanLabel(plan: LiveUsagePlanPayload): string {
  if (plan.name === "max") {
    const tier = plan.tier ?? ""
    const matched = CLAUDE_MAX_TIER_LABELS.find((entry) => tier.includes(entry.substring))
    return matched?.label ?? "Max"
  }
  return CLAUDE_PLAN_NAME_LABELS[plan.name] ?? plan.name
}

/** Codex's plan label. Codex never reports a tier, so only the name matters. */
function codexPlanLabel(plan: LiveUsagePlanPayload): string {
  return CODEX_PLAN_NAME_LABELS[plan.name] ?? plan.name
}

function googlePlanLabel(plan: LiveUsagePlanPayload): string {
  const name = plan.name.trim()
  return GOOGLE_PLAN_NAME_LABELS[name.toLocaleLowerCase()] ?? name
}

/** How to read a plan, keyed by the provider's canonical id. */
const PLAN_LABEL_BY_PROVIDER: Readonly<Record<string, (plan: LiveUsagePlanPayload) => string>> =
  {
    [ANTHROPIC]: claudePlanLabel,
    [GOOGLE]: googlePlanLabel,
    [OPENAI]: codexPlanLabel,
  }

/**
 * The plan a provider reports, in words, or null when nothing was reported.
 *
 * A provider with no entry in the table below falls back to the raw name
 * rather than hiding it: the rule this module lives by is "never fill a
 * gap", and swallowing a real value the app has not learned to word yet
 * would be exactly that, in the other direction.
 */
export function livePlanLabel(provider: LiveProviderUsagePayload): string | null {
  const { plan } = provider
  if (!plan) return null
  const label = PLAN_LABEL_BY_PROVIDER[provider.provider]
  return label ? label(plan) : plan.name
}

/** The plan and account email suffix, with the plan first for truncation. */
export function livePlanAccountLabel(provider: LiveProviderUsagePayload): string | null {
  const plan = livePlanLabel(provider)
  const email = provider.accountEmail?.trim() || null
  if (plan && email) return `${plan} · ${email}`
  return plan ?? email
}

/**
 * The full name of one window.
 *
 * Prefer the provider's stable window identity. Roles only state relative
 * importance, so they must not imply a duration.
 */
export function liveWindowLabel(window: LiveUsageWindowPayload): string {
  const identified = WINDOW_LABEL_BY_ID[window.id]
  if (identified) return identified
  if (window.scopeModel && (window.kind === "weekly" || window.id.startsWith("weekly-"))) {
    return `${window.scopeModel} weekly limit`
  }
  if (window.scopeModel) return `${window.scopeModel} limit`
  if (window.kind === "daily") return "Daily limit"
  if (window.kind === "weekly") return "Weekly limit"
  if (window.kind === "monthly" || window.kind === "billingCycle") return "Monthly limit"
  if (window.role === "primaryShort") return "Short-term limit"
  if (window.role === "primaryLong") return "Long-term limit"
  return "Usage limit"
}

/**
 * `"81%"`, or that we do not know.
 *
 * Deliberately not `"0% used"` when the figure is missing: an empty meter is a
 * claim, and this one would be a claim nobody made.
 */
export function liveWindowValueLabel(window: LiveUsageWindowPayload): string {
  if (window.usedPercent == null) return "Unknown"
  return `${Math.round(window.usedPercent)}%`
}

/**
 * The length of a window whose own *id* states it, in milliseconds, keyed by
 * that id.
 *
 * This is arithmetic, not inference: a window identified as `five-hour` has a
 * five-hour period by definition, so its start is five hours before its reset
 * — that is not a fact we are missing, it is one the id already gave us. The
 * broader case — a period implied by the window's `kind` rather than its
 * specific id — is handled separately below, in `impliedPeriodMs`; this table
 * only covers the recurrence that does not fit that pattern.
 */
const IMPLIED_PERIOD_MS: Readonly<Record<string, number>> = {
  "five-hour": 5 * 3_600_000,
  "antigravity-gemini-5h": 5 * 3_600_000,
  "antigravity-claude-gpt-5h": 5 * 3_600_000,
}

const DAY_MS = 24 * 3_600_000
const WEEK_MS = 7 * DAY_MS

/**
 * The length of a window's period implied by its own identity, in
 * milliseconds, or null when nothing about the window states one.
 *
 * An id-keyed period (`IMPLIED_PERIOD_MS`) wins when there is one, since it is
 * the more specific fact. Failing that, a window's `kind` still states a
 * recurrence for two of the four kinds the provider sends: `weekly` is seven
 * days and `daily` is twenty-four hours by definition of what those words
 * mean, the same way `five-hour` is five hours — this is reading the window's
 * own name, not assuming a period nobody stated. `monthly` and
 * `billingCycle` are deliberately excluded: a month's length genuinely
 * varies (28 to 31 days, and a billing cycle can be shorter still), so
 * "thirty days before the reset" would be a guess dressed as a measurement,
 * exactly the thing this module exists to avoid.
 */
function impliedPeriodMs(window: LiveUsageWindowPayload): number | null {
  const byId = IMPLIED_PERIOD_MS[window.id]
  if (byId != null) return byId
  if (window.kind === "weekly") return WEEK_MS
  if (window.kind === "daily") return DAY_MS
  return null
}

/**
 * How far into the window's own period the clock has travelled, 0–1, or null
 * when it cannot be known.
 *
 * This is what the marker on each bar means, and it is the single most useful
 * thing on the surface: 60% used at 30% elapsed and 60% used at 90% elapsed
 * are opposite situations, and the percentage alone cannot tell them apart.
 *
 * It needs both ends of the window. The provider states the reset but usually
 * not the start, so the start comes from the window's own stated duration
 * where it has one — see `impliedPeriodMs`. A window with neither a stated
 * start nor an implied duration gets no marker at all, rather than one drawn
 * from an assumed period.
 */
export function liveWindowElapsed(window: LiveUsageWindowPayload, now: number): number | null {
  if (!window.resetsAt) return null
  const end = new Date(window.resetsAt).getTime()
  if (Number.isNaN(end)) return null

  const stated = window.startsAt ? new Date(window.startsAt).getTime() : Number.NaN
  const implied = impliedPeriodMs(window)
  const start = Number.isNaN(stated) ? (implied == null ? Number.NaN : end - implied) : stated
  if (Number.isNaN(start) || end <= start) return null

  return Math.min(1, Math.max(0, (now - start) / (end - start)))
}

/**
 * `"resets 4pm"` for a reset later today, `"resets Tue 4pm"` further out, and
 * an honest admission when the provider did not say.
 *
 * A wall-clock time and not a countdown: a clock time stays true for as long
 * as it is on screen, where "in 2h" is wrong a minute after it renders.
 */
export function liveResetLabel(window: LiveUsageWindowPayload, now: number): string {
  if (!window.resetsAt) return "reset unavailable"
  const at = new Date(window.resetsAt).getTime()
  if (Number.isNaN(at)) return "reset unavailable"
  // Already past, and the provider has not published the next one yet. Not an
  // error: a rolling window resets on the provider's clock, not ours.
  if (at - now <= 0) return "reset pending"
  const date = new Date(at)
  const hours24 = date.getHours()
  const hour = hours24 % 12 === 0 ? 12 : hours24 % 12
  const suffix = hours24 < 12 ? "am" : "pm"
  const minutes = date.getMinutes()
  const time =
    minutes === 0 ? `${hour}${suffix}` : `${hour}:${String(minutes).padStart(2, "0")}${suffix}`
  const from = new Date(now)
  // The weekday only when the reset falls on a different calendar day. "Resets
  // 4pm" is unambiguous today and ambiguous tomorrow.
  const sameDay =
    date.getFullYear() === from.getFullYear() &&
    date.getMonth() === from.getMonth() &&
    date.getDate() === from.getDate()
  if (sameDay) return `resets ${time}`
  const day = date.toLocaleDateString(undefined, { weekday: "short" })
  return `resets ${day} ${time}`
}

/**
 * The provenance line: these are provider-stated figures, and this is how old
 * the observation is.
 *
 * One line rather than two because provenance and age are one thought: even a
 * figure stated directly by the provider can have moved since we observed it.
 */
export function liveSourceNote(provider: LiveProviderUsagePayload): string {
  if (!provider.observedAt) return "Live"
  return `Live ${relativeTime(provider.observedAt)}`
}

/**
 * Tailwind classes for the provenance line. Orange only once a reading has
 * gone stale — a fresh reading is not news.
 */
export function liveFreshnessToneClass(freshness: LiveUsageFreshness): string {
  return freshness === "stale" ? "text-system-orange" : "text-label-tertiary"
}

/**
 * The one sentence a stale reading needs, or null when it is current.
 *
 * Phrased as what the app can and cannot see, not as a fault: an agent that
 * has not run since Tuesday has a Tuesday answer, and nothing is wrong.
 */
export function liveStalenessNote(provider: LiveProviderUsagePayload): string | null {
  if (provider.freshness !== "stale") return null
  return `These figures are from ${relativeTime(provider.observedAt)} and may have moved since.`
}

/**
 * Whether a window is a supplemental, model-scoped limit — the kind that
 * sits at 0% for most readers because it tracks a model they never touch.
 *
 * The account-wide primary windows (session, weekly-all-models) are never
 * conditional: they describe the account's overall standing and belong on
 * screen unconditionally, whatever they read.
 */
export function isConditionallyVisibleUsageWindow(window: LiveUsageWindowPayload): boolean {
  return window.role === "supplemental" && window.scopeModel != null
}

/**
 * Whether a window belongs on screen at all.
 *
 * A supplemental per-model weekly limit is noise beside the limits a reader
 * actually hits until they touch that model, so it stays hidden until then.
 * Once it shows non-zero usage — this reading or an earlier one in the same
 * allowance period — it stays visible for the rest of that period, even past
 * a reading that comes back with no percentage at all: a window that was
 * genuinely in use must never look like it vanished.
 */
export function isUsageWindowVisible(window: LiveUsageWindowPayload): boolean {
  if (window.id.startsWith("antigravity-")) return true
  return (
    !isConditionallyVisibleUsageWindow(window) ||
    (window.usedPercent ?? 0) > 0 ||
    window.hasNonzeroUsageInCurrentPeriod
  )
}

/** Windows worth rendering, primary ones first. */
export function liveWindows(provider: LiveProviderUsagePayload): LiveUsageWindowPayload[] {
  const rank = (window: LiveUsageWindowPayload) => {
    if (window.role === "primaryShort") return 0
    if (window.role === "primaryLong") return 1
    return 2
  }
  // A stable sort over a copy, so the provider's own order breaks ties and two
  // supplemental windows never swap places between renders.
  const localAntigravity = provider.sourceLabel.startsWith("Read from Antigravity")
  const primaryAntigravity = provider.windows.filter((window) =>
    ANTIGRAVITY_PRIMARY_WINDOW_IDS.has(window.id),
  )
  const candidates = primaryAntigravity.length > 0 ? primaryAntigravity : provider.windows
  return candidates
    .filter(
      (window) =>
        provider.provider === GOOGLE || localAntigravity || isUsageWindowVisible(window),
    )
    .sort((a, b) => rank(a) - rank(b))
}

/**
 * The fullest of a provider's live windows, as a percentage, or null when
 * none of them carries one.
 *
 * A compact ring can only show one number. A per-model limit at 95% is as
 * important as an account-wide limit at 95%. The expanded bar names every
 * window in the full breakdown.
 */
export function maxLiveUsedPercent(provider: LiveProviderUsagePayload): number | null {
  return liveWindows(provider).reduce<number | null>((max, window) => {
    if (window.usedPercent == null) return max
    return max == null ? window.usedPercent : Math.max(max, window.usedPercent)
  }, null)
}

export interface LiveAccountEntry {
  reading: LiveProviderUsagePayload
  key: string
}

function liveAccountIdentity(reading: LiveProviderUsagePayload): string {
  if (reading.accountKey) return `account:${reading.provider}:${reading.accountKey}`
  return `fallback:${JSON.stringify([reading.provider, reading.sourceLabel])}`
}

/** Preserve provider first-seen order and derive React identity from opaque keys. */
export function orderedLiveAccounts(
  readings: readonly LiveProviderUsagePayload[],
): LiveAccountEntry[] {
  const occurrences = new Map<string, number>()
  return readings.map((reading) => {
    const identity = liveAccountIdentity(reading)
    const occurrence = occurrences.get(identity) ?? 0
    occurrences.set(identity, occurrence + 1)
    return {
      reading,
      key: occurrence === 0 ? identity : `${identity}:duplicate:${occurrence}`,
    }
  })
}

/** The live reading for one provider id, or null when there is none. */
export function liveForProvider(
  summary: LiveUsageSummaryPayload,
  provider: string,
): LiveProviderUsagePayload | null {
  return summary.providers.find((entry) => entry.provider === provider) ?? null
}

/** Whether a provider's reading is live, standing in during its grace period, or too old to show. */
export type LiveProviderStatus =
  | { kind: "live" }
  | { kind: "grace"; category: string; ageMs: number }
  | { kind: "failed"; category: string }

/**
 * A provider's live status: live, within grace after a failed check, or
 * failed past the grace.
 *
 * Age is measured from `summary.generatedAt`, the snapshot's own moment,
 * never from `Date.now()` — a render must not read the clock. When either
 * timestamp cannot be parsed, the age cannot be proven past the grace, so
 * the reading stays in grace rather than being hidden on a guess.
 */
export function liveProviderStatus(
  summary: { errors: readonly LiveUsageSourceErrorPayload[]; generatedAt: string },
  provider: LiveProviderUsagePayload,
): LiveProviderStatus {
  const error = summary.errors.find((entry) => entry.provider === provider.provider)
  if (!error) return { kind: "live" }
  const ageMs = Date.parse(summary.generatedAt) - Date.parse(provider.observedAt)
  if (Number.isNaN(ageMs) || ageMs <= LIVE_USAGE_GRACE_MS) {
    return { kind: "grace", category: error.category, ageMs: Number.isNaN(ageMs) ? 0 : ageMs }
  }
  return { kind: "failed", category: error.category }
}

/**
 * The live providers worth showing on screen: every reading whose status is
 * not `failed`.
 *
 * Every surface that lists live providers reads this instead of
 * `summary.providers` directly, so a provider past its grace period drops
 * out of all of them at once and appears only through
 * `liveUnavailableProviders`.
 */
export function liveDisplayableProviders(
  summary: LiveUsageSummaryPayload,
): LiveProviderUsagePayload[] {
  return summary.providers.filter(
    (provider) => liveProviderStatus(summary, provider).kind !== "failed",
  )
}

/** `"4 min"` at a minute and above, `"under 1 min"` below. */
function formatGraceAge(ageMs: number): string {
  const minutes = Math.floor(ageMs / 60_000)
  return minutes < 1 ? "under 1 min" : `${minutes} min`
}

/** The first sentence of a grace note, per failure category. */
function graceVerb(category: string): string {
  switch (category) {
    case "rateLimited":
      return "rate limited the last check"
    case "authentication":
      return "rejected the sign-in on the last check"
    case "schema":
      return "sent an unreadable reply"
    default:
      return "didn't answer the last check"
  }
}

/**
 * The one sentence a grace-period reading needs: still shown, and why.
 *
 * Reuses `liveErrorNote`'s provider naming, so the two notes name a provider
 * the same way.
 */
export function liveGraceNote(
  category: string,
  provider: string | undefined,
  ageMs: number,
): string {
  const name = liveProviderDisplayName(provider) ?? "Your provider"
  return `${name} ${graceVerb(category)}; reading from ${formatGraceAge(ageMs)} ago.`
}

/**
 * The banner one failed source deserves, or null.
 *
 * Only `authentication` earns a banner: it is the one category with an action
 * behind it. A rate limit passes on its own, an unreadable file is usually a
 * missing agent, and neither is worth interrupting someone over.
 */
export function liveAuthNote(summary: LiveUsageSummaryPayload): string | null {
  const failed = summary.errors.some((error) => error.category === "authentication")
  if (!failed) return null
  return "Sign in again with your coding tool, then retry."
}

/**
 * The providers whose live reading failed and left no windows to draw —
 * exactly the ones the limits surfaces would otherwise drop without a word.
 *
 * A failed source with a cached last-good reading still appears in
 * `providers`, carrying its stale figures; while that reading is within its
 * grace period, the staleness treatment covers it and no entry appears
 * here. This list covers two cases instead: the cold-start failure — first
 * fetch rejected, nothing cached — and a reading that has aged past its
 * grace period, which is treated the same way. An error without a provider
 * id (a snapshot cached before the field existed) cannot name a section and
 * is skipped.
 */
export function liveUnavailableProviders(
  summary: LiveUsageSummaryPayload,
): UnavailableLiveProvider[] {
  const showing = new Set(
    liveDisplayableProviders(summary)
      .filter((provider) => liveWindows(provider).length > 0)
      .map((provider) => provider.provider),
  )
  const seen = new Set<string>()
  const unavailable: UnavailableLiveProvider[] = []
  for (const error of summary.errors) {
    if (!error.provider || showing.has(error.provider) || seen.has(error.provider)) continue
    seen.add(error.provider)
    unavailable.push({
      provider: error.provider,
      displayName: error.displayName || error.provider,
      category: error.category,
    })
  }
  return unavailable
}

/** A failure category as two or three words, for a row with no room. */
export function liveUnavailableReason(category: string): string {
  switch (category) {
    case "authentication":
      return "sign-in needed"
    case "rateLimited":
      return "rate limited"
    case "schema":
      return "unreadable reply"
    default:
      return "unreachable"
  }
}

/** The provider's product name, or null when the id is not one this app names. */
function liveProviderDisplayName(provider?: string): string | null {
  return provider === GOOGLE
    ? "Google"
    : provider === ANTHROPIC
      ? "Claude"
      : provider === OPENAI
        ? "Codex"
        : null
}

/** One action for a failed source, with the provider name when it is known. */
export function liveErrorNote(category: string, provider?: string): string {
  const providerName = liveProviderDisplayName(provider)
  switch (category) {
    case "authentication":
      return providerName
        ? `${providerName} sign-in expired. Sign in again, then retry.`
        : "Sign in again with your coding tool, then retry."
    case "rateLimited":
      return `${providerName ?? "Your provider"} rate limited usage checks. Wait, then retry.`
    case "schema":
      return `${providerName ?? "Provider"} usage changed. Update antiburn, then retry.`
    default:
      return `${providerName ?? "Provider"} usage is unavailable. Check your connection, then retry.`
  }
}

/**
 * Metered spend beyond the allowance, as a row — or null when the provider
 * reports none, or reports it switched off.
 *
 * Off is deliberately silent rather than a row saying zero: a control the
 * reader has already turned off does not need reporting back to them.
 */
export function liveExtraUsageLabel(provider: LiveProviderUsagePayload): string | null {
  const extra = provider.extraUsage
  if (!extra) return null
  if (provider.provider === GOOGLE) {
    const amount = (value: number) =>
      extra.currency
        ? `${value.toFixed(2)} ${extra.currency}`
        : new Intl.NumberFormat().format(value)
    if (extra.remaining != null) return `AI credits: ${amount(extra.remaining)} remaining`
    if (extra.used != null && extra.limit != null) {
      return `AI credits: ${amount(extra.used)} of ${amount(extra.limit)} used`
    }
    if (extra.used != null) return `AI credits: ${amount(extra.used)} used`
    if (extra.limit != null) return `AI credits: ${amount(extra.limit)} total`
    if (extra.usedPercent != null) return `AI credits: ${Math.round(extra.usedPercent)}% used`
    return null
  }
  if (!extra.enabled) return null
  if (extra.usedPercent != null) return `${Math.round(extra.usedPercent)}% of extra usage`
  if (extra.used != null && extra.currency) {
    return `${extra.used.toFixed(2)} ${extra.currency} of extra usage`
  }
  return "Extra usage is on"
}

/* -------------------------------------------------------------------------
 * Derived metrics: what a window's history says about it.
 *
 * Every function below has an explicit "we cannot say" branch, and it fires
 * far more often than the others. A source that only updates when an agent
 * runs produces a sparse series by construction, so "not enough history" is
 * the resting state rather than an error — and it must never be phrased, or
 * coloured, as one.
 * ---------------------------------------------------------------------- */

/** How a window's pace reads against the pace its allowance can afford. */
export type PaceState = "comfortable" | "onPace" | "runningHot" | "atRisk"

/** Where each band starts. Below 0.8 is comfortable; above 1.5 is at risk. */
const PACE_BANDS: ReadonlyArray<{ below: number; state: PaceState }> = [
  { below: 0.8, state: "comfortable" },
  { below: 1.1, state: "onPace" },
  { below: 1.5, state: "runningHot" },
]

/**
 * Which band a pace ratio falls in.
 *
 * The bands are asymmetric on purpose. 1.0 is exactly on track, so the "on
 * pace" band reaches further above it than below: a reader slightly ahead of
 * their allowance is fine, and telling them otherwise every time they have a
 * busy half hour is how a useful signal becomes one people stop reading.
 */
export function paceState(ratio: number): PaceState {
  return PACE_BANDS.find((band) => ratio < band.below)?.state ?? "atRisk"
}

/** The word for a band. */
export function paceStateLabel(state: PaceState): string {
  switch (state) {
    case "comfortable":
      return "Comfortable"
    case "onPace":
      return "On pace"
    case "runningHot":
      return "Running hot"
    default:
      return "At risk"
  }
}

/** Tailwind colour for a band. Green through red, in that order. */
function paceStateToneClass(state: PaceState): string {
  switch (state) {
    case "comfortable":
      return "text-system-green"
    case "onPace":
      return "text-label-secondary"
    case "runningHot":
      return "text-system-orange"
    default:
      return "text-system-red"
  }
}

/** Below this a trend reads as easing; above its mirror, as picking up. */
const TREND_EASING = 0.85
const TREND_PICKING_UP = 1.15

/** `"Picking up"` / `"Steady"` / `"Easing"`. */
export function trendLabel(ratio: number): string {
  if (ratio < TREND_EASING) return "Easing"
  if (ratio > TREND_PICKING_UP) return "Picking up"
  return "Steady"
}

/**
 * Why a window has no derived figures, in a sentence — or null when it has
 * them.
 *
 * Each reason gets its own wording because each has a different implication
 * for the reader. "Not enough history" means come back later; "just reset"
 * means the numbers are fine and simply too new; "out of date" means go and
 * use the agent.
 */
export function forecastUnavailableNote(window: LiveUsageWindowPayload): string | null {
  switch (window.forecast.unavailableReason) {
    case "sparseHistory":
      return "Not enough history"
    case "transition":
      return "Just reset"
    case "stale":
      return "Reading is out of date"
    default:
      return null
  }
}

/** One derived row: a label, a value, and how alarming the value is. */
export interface LiveMetricRow {
  key: string
  label: string
  value: string
  toneClass: string
}

/**
 * The derived rows for one window, in a fixed order.
 *
 * Fixed because they answer a sequence of questions — how fast, will it last,
 * how much was today — and a reader who learns where the runway sits should
 * find it in the same place next time. A row whose
 * figure is unavailable still appears, carrying the reason: a row that
 * vanishes takes the question with it.
 */
export function liveMetricRows(window: LiveUsageWindowPayload, now: number): LiveMetricRow[] {
  const { forecast } = window
  const unavailable = forecastUnavailableNote(window)
  const muted = "text-label-tertiary"

  const pace: LiveMetricRow = {
    key: "pace",
    label: "Pace",
    value: unavailable ?? "Not enough history",
    toneClass: muted,
  }
  if (forecast.paceRatio != null && forecast.consumptionRate != null) {
    const state = paceState(forecast.paceRatio)
    pace.value = `${paceStateLabel(state)} · ${forecast.paceRatio.toFixed(1)}× · ${forecast.consumptionRate.toFixed(
      1,
    )}%/hour`
    pace.toneClass = paceStateToneClass(state)
  } else if (forecast.consumptionRate != null) {
    // A rate with no reset to measure it against: still worth showing, but it
    // is not a verdict, so it stays muted.
    pace.value = `${forecast.consumptionRate.toFixed(1)}%/hour`
  }

  const runway: LiveMetricRow = {
    key: "runway",
    label: "Runway",
    value: unavailable ?? "Not enough history",
    toneClass: muted,
  }
  const runwayNote = runwayLabel(window, now)
  if (runwayNote) {
    runway.value = runwayNote
    runway.toneClass =
      forecast.paceRatio != null && forecast.paceRatio > 1 ? "text-system-orange" : muted
  }

  const rows = [pace, runway]
  if (forecast.usedToday != null) {
    // The shell reports percentage points of this window's allowance, and only
    // for a window longer than a day. "Points" is the unit, not a word a
    // reader knows, so the row gives the share of the limit instead.
    rows.push({
      key: "today",
      label: "Today's usage %",
      value: `${formatAllowanceShare(forecast.usedToday)} of this limit`,
      toneClass: muted,
    })
  }
  return rows
}

/**
 * A share of a limit's allowance, as a percentage.
 *
 * A decimal below ten, so a small but real share does not round away to `0%`;
 * a whole number above it, where the decimal adds nothing.
 */
function formatAllowanceShare(points: number): string {
  return points < 10 ? `${points.toFixed(1)}%` : `${Math.round(points)}%`
}

/**
 * `"Hits the limit Thu 15:00"`, or that it lasts — or null when there is no
 * forecast to say either from.
 *
 * A runway past the reset is reported as lasting rather than as a date,
 * because a limit that refills before you reach it is not a deadline and
 * printing one would invent an anxiety.
 */
export function runwayLabel(window: LiveUsageWindowPayload, now: number): string | null {
  const at = window.forecast.runwayAt ? Date.parse(window.forecast.runwayAt) : Number.NaN
  if (Number.isNaN(at)) return null

  const reset = window.resetsAt ? Date.parse(window.resetsAt) : Number.NaN
  if (!Number.isNaN(reset) && at >= reset) return "Lasts past the reset"
  if (at <= now) return "At the limit"

  const date = new Date(at)
  const remaining = at - now
  if (remaining < 86_400_000) {
    const hours = Math.floor(remaining / 3_600_000)
    const minutes = Math.floor((remaining % 3_600_000) / 60_000)
    return hours > 0 ? `Runs out in ${hours}h ${minutes}m` : `Runs out in ${minutes}m`
  }
  const day = date.toLocaleDateString(undefined, { weekday: "short" })
  const time = date.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" })
  return `Runs out ${day} ${time}`
}
