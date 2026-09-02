/** How well the app can describe one provider's usage. Mirrors Rust `ProviderUsageState`. */
export type ProviderUsageState = "live" | "estimated" | "observed" | "detected" | "unknown"

/** Whether a provider's newest local evidence still describes now. */
export type ProviderUsageStaleness = "fresh" | "stale" | "unknown"

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

export interface ProviderAgentUsagePayload {
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

/** Local provider usage as one snapshot. Mirrors Rust `ProviderUsageSummary`. */
export interface ProviderUsageSummaryPayload {
  providers: ProviderUsagePayload[]
  totals?: ProviderUsageWindowsPayload
  agents?: ProviderAgentUsagePayload[]
  generatedAt: string
}

export type SessionLimitMetricPayload = "weekly" | "fiveHour"

/** One session's estimated share of a provider-reported allowance. */
export interface SessionLimitAllocationPayload {
  agent: string
  sessionId: string
  wslDistro: string | null
  metric: SessionLimitMetricPayload
  provider: string
  displayName: string
  accountKey: string | null
  windowId: string
  resetsAt: string
  percent: number
}

export interface SessionLimitAllocationSummaryPayload {
  allocations: SessionLimitAllocationPayload[]
  generatedAt: string
}

/** Marks figures stated directly by a provider. Mirrors Rust `LiveUsageSupport`. */
export type LiveUsageSupport = "live"

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
export interface LiveExtraUsagePayload {
  /** Whether the account permits this path. */
  enabled: boolean
  usedPercent: number | null
  used: number | null
  remaining: number | null
  limit: number | null
  currency: string | null
}

/** Provider credits that manually reset rate limits. */
export interface LiveUsageResetCreditsPayload {
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
}

/** A source that failed, in terms a reader can act on. */
export interface LiveUsageSourceErrorPayload {
  source: string
  provider: string
  displayName: string
  /** `authentication` | `rateLimited` | `schema` | `unavailable`. */
  category: string
}

/** Live provider usage as one snapshot. Mirrors Rust `LiveUsageSummary`. */
export interface LiveUsageSummaryPayload {
  providers: LiveProviderUsagePayload[]
  errors: LiveUsageSourceErrorPayload[]
  meters: LiveUsageMeterPayload[]
  generatedAt: string
}

/** One provider antiburn can meter. Mirrors Rust `LiveUsageMeter`. */
export interface LiveUsageMeterPayload {
  provider: string
  displayName: string
  /** False when the reader turned this meter off. */
  shown: boolean
}
