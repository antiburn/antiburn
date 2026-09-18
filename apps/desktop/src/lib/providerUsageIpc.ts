import { invoke, isTauri } from "@tauri-apps/api/core"

import { traceAsync, traceEvent } from "./perfTrace"

/** How well the app can describe one provider's usage. Mirrors Rust `ProviderUsageState`. */
export type ProviderUsageState = "live" | "estimated" | "observed" | "detected" | "unknown"

/** Whether a provider's newest local evidence still describes now. */
type ProviderUsageStaleness = "fresh" | "stale" | "unknown"

/** One provider's totals over one window. */
export interface ProviderUsageWindowPayload {
  /** Fresh prompt tokens plus prompt-cache writes. */
  tokensIn: number
  tokensOut: number
  /** Prompt-cache reads, billed at their own rate. */
  cacheRead: number
  /** On-device estimate for the models that could be priced. */
  estimatedUsd: number | null
  /** Whether every token-bearing model in this window could be priced. */
  costComplete: boolean
  /** Sessions that contributed. One session can count under two providers. */
  sessionCount: number
}

/** Local usage windows, including both month-to-date and trailing 30 days. */
export interface ProviderUsageWindowsPayload {
  today: ProviderUsageWindowPayload
  week: ProviderUsageWindowPayload
  monthToDate: ProviderUsageWindowPayload
  last30Days: ProviderUsageWindowPayload
}

interface ProviderAgentUsagePayload {
  agent: string
  windows: ProviderUsageWindowsPayload
}

/** Everything the usage surfaces show about one provider. */
export interface ProviderUsagePayload {
  /** Canonical provider ID. */
  provider: string
  /** Installation-scoped opaque account key, or null when unassigned. */
  accountKey: string | null
  displayName: string
  state: ProviderUsageState
  staleness: ProviderUsageStaleness
  windows: ProviderUsageWindowsPayload
  /** Per-agent contributions retained inside this provider account group. */
  agents: ProviderAgentUsagePayload[]
  lastActivityAt: string | null
}

/** One local calendar day's totals across every provider. Mirrors Rust `ProviderUsageDay`. */
export interface ProviderUsageDayPayload extends ProviderUsageWindowPayload {
  /** The reader's calendar date, `YYYY-MM-DD`. */
  localDate: string
}

/** Local provider usage as one snapshot. Mirrors Rust `ProviderUsageSummary`. */
export interface ProviderUsageSummaryPayload {
  providers: ProviderUsagePayload[]
  totals?: ProviderUsageWindowsPayload
  agents?: ProviderAgentUsagePayload[]
  /** The trailing thirty days, oldest first and today last; empty days included. */
  days?: ProviderUsageDayPayload[]
  /** The thirty days before `days`, for a like-for-like comparison. */
  previousDays?: ProviderUsageDayPayload[]
  generatedAt: string
}

type SessionLimitMetricPayload = "weekly" | "fiveHour"

/** One session's estimated share of a provider account's learned
 * dollars-per-percent limit factor. Mirrors Rust `SessionLimitAllocation`. */
export interface SessionLimitAllocationPayload {
  agent: string
  sessionId: string
  wslDistro: string | null
  metric: SessionLimitMetricPayload
  provider: string
  displayName: string
  accountKey: string | null
  /** The lane the factor belongs to (`weekly` or `fiveHour`), not a
   * specific provider window. */
  windowId: string
  percent: number
  /** `learned` from a meter delta, `seeded` from a single first-reading
   * estimate. Either way the percentage is an estimate, never a bill. */
  confidence: "learned" | "seeded"
}

export interface SessionLimitAllocationSummaryPayload {
  allocations: SessionLimitAllocationPayload[]
  generatedAt: string
}

/** One quota window's derived start or end. `reported` came from the
 * provider directly, `derived` was computed from the other boundary and the
 * lane's nominal duration, `cadence` was extrapolated from another observed
 * weekly reset, and `turnGap` was inferred from a gap in local turn
 * activity. Mirrors Rust `QuotaBoundarySource`. */
type QuotaBoundarySourcePayload = "reported" | "derived" | "cadence" | "turnGap"

/** A lane's currently open window, when one exists. Mirrors Rust
 * `QuotaCurrentPeriodPayload`. */
interface QuotaCurrentPeriodPayload {
  startsAtEpoch: number
  resetsAtEpoch: number
}

/** One lane a quota account carries. Mirrors Rust `QuotaLanePayload`. */
export interface QuotaLanePayload {
  /** `weekly`, `fiveHour`, or `model:<slug>`. */
  lane: string
  /** `"Weekly"`, `"5-hour"`, or the model-scoped window's own label
   * (Anthropic's is currently "Fable"). */
  label: string
  hasFactor: boolean
  /** The lane's open window, derived the same way the period resolver
   * derives a boundary the provider did not state. `null` when every known
   * period for the lane has already reset. */
  currentPeriod: QuotaCurrentPeriodPayload | null
}

/** One `(provider, account)` this app has observed at least one quota
 * period for. Mirrors Rust `QuotaAccountPayload`. */
export interface QuotaAccountPayload {
  provider: string
  displayName: string
  accountKey: string
  lanes: QuotaLanePayload[]
}

/** Response for `get_quota_accounts`. Mirrors Rust `QuotaAccountsPayload`. */
export interface QuotaAccountsPayload {
  accounts: QuotaAccountPayload[]
  generatedAt: string
}

/** Request for `get_quota_usage`. Mirrors Rust `QuotaUsageRequest`. */
export interface QuotaUsageRequest {
  provider: string
  accountKey: string
  lane: string
  rangeStartEpoch: number
  rangeEndEpoch: number
}

/** The lane's factor at the newest point in effect. Mirrors Rust
 * `QuotaFactorPayload`. */
interface QuotaFactorPayload {
  usdPerPercent: number
  /** `learned` from a meter delta, `seeded` from a single first-reading
   * estimate. */
  confidence: "learned" | "seeded"
}

/** One meter reading inside a quota period. Mirrors Rust
 * `QuotaSamplePayload`. */
export interface QuotaSamplePayload {
  observedAtEpoch: number
  usedPercent: number | null
  fresh: boolean
  authoritative: boolean
}

/** One session's estimated dollars inside one 15-minute bucket of a quota
 * period. Mirrors Rust `QuotaContributionPayload`. */
export interface QuotaContributionPayload {
  agent: string
  sessionId: string
  wslDistro: string | null
  bucketStartEpoch: number
  usd: number
  percent: number | null
}

/** One session's estimated total inside a quota period. Mirrors Rust
 * `QuotaSessionTotalPayload`. */
interface QuotaSessionTotalPayload {
  agent: string
  sessionId: string
  wslDistro: string | null
  title: string | null
  usd: number
  percent: number | null
}

/** Spend inside a quota period this app could not credit to any session.
 * Mirrors Rust `QuotaUnattributedPayload`. */
export interface QuotaUnattributedPayload {
  usd: number
  percent: number | null
  sessionCount: number
}

/** Unattributed spend inside one 15-minute bucket of a quota period.
 * Mirrors Rust `QuotaBucketTotalPayload`. */
interface QuotaBucketTotalPayload {
  bucketStartEpoch: number
  usd: number
  percent: number | null
}

/** One quota window, its meter readings, and the sessions estimated to have
 * contributed to it. Mirrors Rust `QuotaPeriodPayload`. */
export interface QuotaPeriodPayload {
  /** `null` for a period this app derived rather than observed directly: a
   * cadence-extrapolated or turn-gap-inferred window. */
  periodId: number | null
  startsAtEpoch: number
  resetsAtEpoch: number
  startSource: QuotaBoundarySourcePayload
  resetSource: QuotaBoundarySourcePayload
  samples: QuotaSamplePayload[]
  /** The highest authoritative meter reading in this period. */
  peakPercent: number | null
  contributions: QuotaContributionPayload[]
  /** Descending by `usd`. */
  sessions: QuotaSessionTotalPayload[]
  unattributed: QuotaUnattributedPayload
  /** Ascending by bucket. Holds one entry for each bucket with an unbound
   * row, so a chart can plot unattributed spend over time instead of a
   * single period total. */
  unattributedBuckets: QuotaBucketTotalPayload[]
  /** The sum of every bound session's estimated percent. */
  estimatedPercent: number | null
}

/** Response for `get_quota_usage`. Mirrors Rust `QuotaUsagePayload`. */
export interface QuotaUsagePayload {
  provider: string
  accountKey: string
  lane: string
  laneLabel: string
  rangeStartEpoch: number
  rangeEndEpoch: number
  factor: QuotaFactorPayload | null
  periods: QuotaPeriodPayload[]
  generatedAt: string
}

/** Request for `get_session_quota`. Mirrors Rust `SessionQuotaRequest`. */
interface SessionQuotaRequest {
  agent: string
  sessionId: string
  wslDistro: string | null
}

/** The quota period one [[SessionQuotaEntryPayload]] falls in. Mirrors Rust
 * `SessionQuotaPeriodPayload`. */
interface SessionQuotaPeriodPayload {
  periodId: number | null
  startsAtEpoch: number
  resetsAtEpoch: number
  startSource: QuotaBoundarySourcePayload
  resetSource: QuotaBoundarySourcePayload
  peakPercent: number | null
}

/** One `(provider, lane, period)` a session's turns fell in. Mirrors Rust
 * `SessionQuotaEntryPayload`. */
export interface SessionQuotaEntryPayload {
  provider: string
  displayName: string
  /** `null` when the session has no resolved account for this provider. */
  accountKey: string | null
  /** `null` only when `confidence` is `"unbound"`: an entry with no
   * resolved account has no lane to name either. */
  lane: string | null
  /** `null` only when `confidence` is `"unbound"`. */
  laneLabel: string | null
  /** `null` only when `confidence` is `"unbound"`. */
  period: SessionQuotaPeriodPayload | null
  usd: number
  percent: number | null
  /** `"learned"`, `"seeded"`, or `"unbound"` when the session has no
   * resolved account for the provider its usage attributes to. */
  confidence: "learned" | "seeded" | "unbound"
}

/** Response for `get_session_quota`. Mirrors Rust `SessionQuotaPayload`. */
export interface SessionQuotaPayload {
  entries: SessionQuotaEntryPayload[]
  generatedAt: string
}

/** Every `(provider, account)` this app has observed at least one quota
 * period for, and each account's lanes. */
export async function getQuotaAccounts(): Promise<QuotaAccountsPayload> {
  return traceAsync("ipc.getQuotaAccounts", {}, async () => {
    if (!isTauri()) return EMPTY_QUOTA_ACCOUNTS
    return invoke<QuotaAccountsPayload>("get_quota_accounts")
  })
}

/** One lane's quota windows over a range, its meter readings, and the
 * sessions estimated to have contributed to each window. */
export async function getQuotaUsage(request: QuotaUsageRequest): Promise<QuotaUsagePayload> {
  const { provider, accountKey, lane, rangeStartEpoch, rangeEndEpoch } = request
  const usage = await traceAsync(
    "ipc.getQuotaUsage",
    {
      provider,
      accountKey,
      lane,
      rangeStartEpoch,
      rangeEndEpoch,
      spanDays: (rangeEndEpoch - rangeStartEpoch) / 86400,
    },
    async () => {
      if (!isTauri()) return EMPTY_QUOTA_USAGE
      return invoke<QuotaUsagePayload>("get_quota_usage", { request })
    },
  )
  traceEvent("ipc.getQuotaUsage.payload", {
    periods: usage.periods.length,
    samples: usage.periods.reduce((total, period) => total + period.samples.length, 0),
    contributionBuckets: usage.periods.reduce(
      (total, period) => total + period.contributions.length,
      0,
    ),
    sessions: usage.periods.reduce((total, period) => total + period.sessions.length, 0),
    factorNull: usage.factor == null,
  })
  return usage
}

/** One session's estimated quota contributions, by provider and lane. */
export async function getSessionQuota(
  request: SessionQuotaRequest,
): Promise<SessionQuotaPayload> {
  if (!isTauri()) return EMPTY_SESSION_QUOTA
  return invoke<SessionQuotaPayload>("get_session_quota", { request })
}

const EMPTY_QUOTA_ACCOUNTS: QuotaAccountsPayload = {
  accounts: [],
  generatedAt: "",
}

const EMPTY_QUOTA_USAGE: QuotaUsagePayload = {
  provider: "",
  accountKey: "",
  lane: "",
  laneLabel: "",
  rangeStartEpoch: 0,
  rangeEndEpoch: 0,
  factor: null,
  periods: [],
  generatedAt: "",
}

const EMPTY_SESSION_QUOTA: SessionQuotaPayload = {
  entries: [],
  generatedAt: "",
}

/** Marks figures stated directly by a provider. Mirrors Rust `LiveUsageSupport`. */
type LiveUsageSupport = "live"

/** Whether a live reading still describes now. */
export type LiveUsageFreshness = "fresh" | "stale"

/** One provider-reported allowance. */
export interface LiveUsageWindowPayload {
  /** `five-hour`, `seven-day`, `weekly-<model>`, or the provider's own name. */
  id: string
  /** `primaryShort` | `primaryLong` | `supplemental` | the provider's word. */
  role: string
  /** `rolling` | `weekly` | `daily` | `monthly` | `billingCycle` | provider's. */
  kind: string
  /** The model a scoped window covers, when it covers one. */
  scopeModel: string | null
  /** Consumed capacity, 0-100. Never remaining. */
  usedPercent: number | null
  startsAt: string | null
  resetsAt: string | null
  /** Whether history shows non-zero usage in this allowance period. */
  hasNonzeroUsageInCurrentPeriod: boolean
  forecast: LiveUsageForecastPayload
}

/** The derived half of a provider allowance window. */
export interface LiveUsageForecastPayload {
  /** `stale` | `transition` | `sparseHistory`, or null when available. */
  unavailableReason: string | null
  /** `low` | `medium` | `high`. */
  confidence: string | null
  /** Percentage points of the allowance consumed per hour. */
  consumptionRate: number | null
  /** Current rate divided by the rate that reaches the allowance at reset. */
  paceRatio: number | null
  /** Last half-hour rate divided by the last two-hour rate. */
  paceTrend: number | null
  /** When the allowance runs out at the current rate. */
  runwayAt: string | null
  /** Points of this window consumed since the reader's local midnight. */
  usedToday: number | null
}

/** Metered spend alongside the allowance. */
interface LiveExtraUsagePayload {
  /** Whether the account permits this path. */
  enabled: boolean
  usedPercent: number | null
  used: number | null
  remaining: number | null
  limit: number | null
  currency: string | null
}

/** Provider credits that manually reset rate limits. */
interface LiveUsageResetCreditsPayload {
  availableCount: number
}

/** The account's subscription plan, in the provider's own raw strings. */
export interface LiveUsagePlanPayload {
  name: string
  tier: string | null
}

/** One provider account's live usage. Mirrors Rust `LiveProviderUsage`. */
export interface LiveProviderUsagePayload {
  provider: string
  /** Stable opaque account key. Null when the source does not identify an account. */
  accountKey: string | null
  displayName: string
  support: LiveUsageSupport
  freshness: LiveUsageFreshness
  /** Where the figures came from. Safe to display; carries no account ID. */
  sourceLabel: string
  /** When the provider fact was observed. */
  observedAt: string
  windows: LiveUsageWindowPayload[]
  extraUsage: LiveExtraUsagePayload | null
  resetCredits: LiveUsageResetCreditsPayload | null
  plan: LiveUsagePlanPayload | null
  /** The provider's account UUID for local display, when known. */
  accountUuid: string | null
  /** The provider's account email for local display, when known. */
  accountEmail: string | null
}

/** A source that failed, in terms a reader can act on. */
export interface LiveUsageSourceErrorPayload {
  source: string
  provider: string
  displayName: string
  /** `authentication` | `rateLimited` | `schema` | `unavailable`. */
  category: string
  /** Which failure inside the category, when the source can say. Mirrors Rust `SourceErrorDetail`. */
  detail?: LiveUsageSourceErrorDetail
}

export type LiveUsageSourceErrorDetail =
  | "keychainUnreadable"
  | "refreshUnsupported"
  | "cliMissing"
  | "signInRequired"
  | "refreshPending"

/** Live provider usage as one snapshot. Mirrors Rust `LiveUsageSummary`. */
export interface LiveUsageSummaryPayload {
  providers: LiveProviderUsagePayload[]
  errors: LiveUsageSourceErrorPayload[]
  meters: LiveUsageMeterPayload[]
  generatedAt: string
}

export type LiveUsageDetection =
  "notInstalled" | "installedNotSignedIn" | "signedIn" | "unknown"

/** One provider antiburn can meter. Mirrors Rust `LiveUsageMeter`. */
export interface LiveUsageMeterPayload {
  provider: string
  displayName: string
  /** False when the reader turned this meter off. */
  shown: boolean
  detection?: LiveUsageDetection
  /** Where the login was found, when a carrier was. Mirrors Rust `LoginCarrier`. */
  carrier?: LiveLoginCarrier
  /** `carrier` as the reader would name it, e.g. "the Claude Code CLI (Keychain)". */
  carrierLabel?: string
}

type LiveLoginCarrier =
  | "claudeCredentialsFile"
  | "claudeKeychain"
  | "pi"
  | "codexAuthFile"
  | "agyToken"
  | "antigravityIde"
  | "antigravityKeyring"
