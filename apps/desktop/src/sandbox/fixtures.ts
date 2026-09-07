/**
 * Fixture factories for the UI sandbox.
 *
 * Each factory returns a full IPC payload, typed against `ipc.ts`. A rename on
 * the shell side then fails `type-check` here instead of blanking the page.
 * Timestamps count back from load time, so "3 minutes ago" stays true.
 */

import type { ChecksReportPayload, SessionHygienePayload } from "../lib/insightsIpc"
import {
  DEFAULT_SETTINGS,
  type ActivityEntryPayload,
  type AppInfo,
  type AppSettings,
  type LiveProviderUsagePayload,
  type LiveUsageSummaryPayload,
  type LiveUsageWindowPayload,
  type ProviderUsagePayload,
  type ProviderUsageSummaryPayload,
  type ProviderUsageWindowPayload,
  type ScanStatus,
  type SessionAnalysisPayload,
  type SubagentMemberPayload,
} from "../lib/ipc"
import type {
  ActiveSessionsSummary,
  BillableTokens,
  SessionBucket,
  SessionCostComponents,
  SessionEfficiency,
  SessionMetrics,
} from "../lib/types/session"

/** The wall clock at page load. Every relative timestamp counts back from here. */
const LOADED_AT = Date.now()

export function minutesAgo(minutes: number): string {
  return new Date(LOADED_AT - minutes * 60_000).toISOString()
}

export function minutesFromNow(minutes: number): string {
  return new Date(LOADED_AT + minutes * 60_000).toISOString()
}

function round(value: number): number {
  return Math.round(value * 100) / 100
}

/* -------------------------------------------------------------------------
 * App state
 * ---------------------------------------------------------------------- */

export function settings(overrides: Partial<AppSettings> = {}): AppSettings {
  return {
    ...DEFAULT_SETTINGS,
    onboardingCompleted: true,
    overviewLimitsExpanded: false,
    ...overrides,
  }
}

export function appInfo(overrides: Partial<AppInfo> = {}): AppInfo {
  return {
    appVersion: "0.4.0",
    debugBuild: true,
    arch: "aarch64",
    pricingCatalogVersion: "2026-09-01",
    schemaVersion: 12,
    dataDir: "/Users/avery/Library/Application Support/antiburn",
    indexedSessions: 412,
    databaseBytes: 48_000_000,
    updatesSupported: false,
    analyticsSupported: false,
    analyticsEnvironmentDisabled: false,
    analyticsOperator: null,
    ...overrides,
  }
}

export function scanStatus(overrides: Partial<ScanStatus> = {}): ScanStatus {
  return {
    running: false,
    completedAgents: 11,
    totalAgents: 11,
    sessions: 4,
    finishedAt: minutesAgo(2),
    cancelled: false,
    error: null,
    agents: [],
    listChanged: false,
    reDescribed: 0,
    ...overrides,
  }
}

/* -------------------------------------------------------------------------
 * Sessions
 * ---------------------------------------------------------------------- */

/** Split a total the way a well-cached session does. */
export function cost(totalUsd: number): SessionCostComponents {
  return {
    totalUsd,
    inputUsd: round(totalUsd * 0.12),
    outputUsd: round(totalUsd * 0.38),
    cacheReadUsd: round(totalUsd * 0.42),
    cacheWriteUsd: round(totalUsd * 0.08),
  }
}

export function tokens(totalUsd: number): BillableTokens {
  return {
    inputTokens: Math.round(totalUsd * 8_000),
    outputTokens: Math.round(totalUsd * 5_000),
    cacheReadTokens: Math.round(totalUsd * 900_000),
    cacheCreationTokens: Math.round(totalUsd * 70_000),
  }
}

function efficiency(totalUsd: number): SessionEfficiency {
  return {
    totalUsd,
    newWorkUsd: round(totalUsd * 0.5),
    carryUsd: round(totalUsd * 0.42),
    rewriteUsd: round(totalUsd * 0.08),
    growthTokens: Math.round(totalUsd * 60_000),
    outputTokens: Math.round(totalUsd * 5_000),
    pricedTurns: Math.max(4, Math.round(totalUsd * 30)),
    unpricedTurns: 0,
  }
}

export function activityEntry(
  overrides: Partial<ActivityEntryPayload> = {},
): ActivityEntryPayload {
  return {
    agent: "claude-code",
    sessionId: "session-abc-123",
    repo: "widgets",
    timestamp: minutesAgo(1),
    isActive: false,
    surface: "cli",
    wslDistro: null,
    title: "Wire the tray popover",
    hasForkParent: false,
    forkChildCount: 0,
    cost: cost(1.25),
    models: ["claude-opus-4-6"],
    modelRuns: [{ model: "claude-opus-4-6" }],
    ...overrides,
  }
}

function bucket(overrides: Partial<SessionBucket> = {}): SessionBucket {
  return {
    tokensIn: 1_000,
    tokensOut: 200,
    subagentTokens: 0,
    contextTokens: 40_000,
    isCompactionBoundary: false,
    cacheReadTokens: 0,
    cacheWriteTokens: 0,
    rewriteTokens: 0,
    isCacheRehydration: false,
    isCacheRoutingMiss: false,
    secsSincePriorTurn: null,
    subagentLaunches: 0,
    userPrompts: 0,
    lastTool: null,
    model: null,
    thinkingMode: null,
    speed: null,
    hasThinking: false,
    compactionTrigger: null,
    compactionPreTokens: null,
    compactionPostTokens: null,
    ...overrides,
  }
}

const TOOLS = ["Read", "Edit", "Bash", "Grep", "Write"]

/**
 * A session timeline. Context grows to `peak`, then one automatic compaction
 * at bucket `compactionAt` drops it before it grows again. The variation is
 * arithmetic, not random, so a screenshot stays the same between loads.
 */
function timeline(
  count: number,
  peak: number,
  model: string,
  compactionAt: number | null = null,
): SessionBucket[] {
  return Array.from({ length: count }, (_, index) => {
    const compaction = compactionAt !== null && index === compactionAt
    let contextTokens: number
    if (compactionAt === null || index < compactionAt) {
      const span = compactionAt ?? count
      contextTokens = Math.round(peak * ((index + 1) / span))
    } else if (compaction) {
      contextTokens = Math.round(peak * 0.3)
    } else {
      const after = (index - compactionAt) / Math.max(1, count - 1 - compactionAt)
      contextTokens = Math.round(peak * (0.3 + 0.6 * after))
    }
    return bucket({
      tokensIn: 400 + ((index * 37) % 900),
      tokensOut: 120 + ((index * 53) % 400),
      contextTokens,
      cacheReadTokens: Math.round(contextTokens * 0.85),
      cacheWriteTokens: 300 + ((index * 29) % 500),
      isCompactionBoundary: compaction,
      compactionTrigger: compaction ? "auto" : null,
      compactionPreTokens: compaction ? peak : null,
      compactionPostTokens: compaction ? contextTokens : null,
      userPrompts: index % 4 === 0 ? 1 : 0,
      lastTool: TOOLS[index % TOOLS.length] ?? null,
      model,
      hasThinking: index % 2 === 0,
      secsSincePriorTurn: 20 + ((index * 17) % 90),
    })
  })
}

export function sessionMetrics(overrides: Partial<SessionMetrics> = {}): SessionMetrics {
  const model = overrides.model ?? "claude-opus-4-6"
  return {
    agent: "claude-code",
    sessionId: "session-abc-123",
    durationSecs: 5_400,
    activeSecs: 3_900,
    eventCount: 212,
    tokensIn: 180_000,
    tokensOut: 24_000,
    peakContextTokens: 150_000,
    compactionCount: 1,
    cacheRehydrationCount: 0,
    contextAvailable: true,
    contextWindow: 200_000,
    contextWindowSource: "reported",
    model,
    billableInputTokens: 42_000,
    billableOutputTokens: 24_000,
    billableCacheReadTokens: 3_100_000,
    billableCacheCreationTokens: 260_000,
    initialContext: {
      sources: [
        {
          source: "skill_instructions",
          sourceName: "discuss",
          tokenCount: 3_200,
          useCount: 2,
          origin: "user",
        },
        {
          source: "mcp_instructions",
          sourceName: "figma",
          tokenCount: 9_800,
          useCount: 0,
          origin: "plugin",
        },
        { source: "builtin_tool", sourceName: "Bash", tokenCount: 1_100, origin: "bundled" },
      ],
    },
    buckets: timeline(40, 150_000, model, 28),
    ...overrides,
  }
}

export function summary(metrics: SessionMetrics): ActiveSessionsSummary {
  return {
    sessionCount: 1,
    avgDurationSecs: metrics.durationSecs,
    avgActiveSecs: metrics.activeSecs,
    tokensInTotal: metrics.tokensIn,
    tokensOutTotal: metrics.tokensOut,
    peakContextTokens: metrics.peakContextTokens,
    compactionCount: metrics.compactionCount ?? 0,
    cacheRehydrationCount: metrics.cacheRehydrationCount ?? 0,
    contextAvailable: true,
    contextWindow: metrics.contextWindow,
    costTotalUsd: metrics.cost?.totalUsd ?? null,
    buckets: metrics.buckets,
    sessions: [metrics],
  }
}

export function subagentMember(
  overrides: Partial<SubagentMemberPayload> = {},
): SubagentMemberPayload {
  return {
    agent: "claude-code",
    subagentId: "subagent-1",
    label: "Explore the IPC surface for every popover command",
    cost: cost(0.8),
    tokens: tokens(0.8),
    startedAtEpoch: Math.floor((LOADED_AT - 4_000_000) / 1000),
    modelRuns: [{ model: "claude-sonnet-5" }],
    ...overrides,
  }
}

/** The analysis for `entry`, with a full timeline behind the chart. */
export function sessionAnalysis(
  entry: ActivityEntryPayload,
  overrides: Partial<SessionAnalysisPayload> = {},
): SessionAnalysisPayload {
  const totalUsd = entry.cost?.totalUsd ?? 0
  const model = entry.models[0] ?? "claude-opus-4-6"
  const metrics = sessionMetrics({
    agent: entry.agent,
    sessionId: entry.sessionId,
    model,
    cost: entry.cost,
    efficiency: efficiency(totalUsd),
  })
  return {
    summary: summary(metrics),
    supportsAnalysis: true,
    title: entry.title,
    wslDistro: entry.wslDistro,
    isActive: entry.isActive,
    cost: entry.cost,
    topLevelCost: entry.cost,
    subagentsCost: null,
    inclusiveTokens: tokens(totalUsd),
    subagentsTokens: null,
    efficiency: efficiency(totalUsd),
    models: entry.models,
    modelRuns: entry.modelRuns,
    orchestration: null,
    relations: null,
    sourcePath: `/Users/avery/.claude/projects/${entry.repo}/${entry.sessionId}.jsonl`,
    startedAtEpoch: Math.floor((LOADED_AT - metrics.durationSecs * 1000) / 1000),
    analysisPending: false,
    analysisStale: false,
    ...overrides,
  }
}

export function sessionHygiene(
  overrides: Partial<SessionHygienePayload> = {},
): SessionHygienePayload {
  return {
    evidenceState: "ready",
    badges: [
      { id: "sessionOverdepth", status: "clean", notAssessedReason: null },
      { id: "modelOverthinking", status: "clean", notAssessedReason: null },
      { id: "overpoweredSubagents", status: "clean", notAssessedReason: null },
      { id: "obsoleteModel", status: "clean", notAssessedReason: null },
      { id: "fastModeOveruse", status: "clean", notAssessedReason: null },
      { id: "excessCacheRehydration", status: "clean", notAssessedReason: null },
    ],
    ...overrides,
  }
}

/* -------------------------------------------------------------------------
 * Usage
 * ---------------------------------------------------------------------- */

export function usageWindow(
  estimatedUsd: number,
  sessionCount: number,
): ProviderUsageWindowPayload {
  return {
    tokensIn: Math.round(estimatedUsd * 9_000),
    tokensOut: Math.round(estimatedUsd * 1_500),
    cacheRead: Math.round(estimatedUsd * 120_000),
    estimatedUsd,
    costComplete: true,
    sessionCount,
  }
}

export function providerUsage(
  overrides: Partial<ProviderUsagePayload> = {},
): ProviderUsagePayload {
  return {
    provider: "anthropic",
    accountKey: null,
    displayName: "Anthropic",
    state: "estimated",
    staleness: "fresh",
    windows: {
      today: usageWindow(4.8, 3),
      week: usageWindow(31.2, 14),
      monthToDate: usageWindow(96.4, 41),
      last30Days: usageWindow(128.9, 55),
    },
    agents: [],
    lastActivityAt: minutesAgo(1),
    ...overrides,
  }
}

function sumWindows(windows: ProviderUsageWindowPayload[]): ProviderUsageWindowPayload {
  return windows.reduce(
    (total, window) => ({
      tokensIn: total.tokensIn + window.tokensIn,
      tokensOut: total.tokensOut + window.tokensOut,
      cacheRead: total.cacheRead + window.cacheRead,
      estimatedUsd: round((total.estimatedUsd ?? 0) + (window.estimatedUsd ?? 0)),
      costComplete: total.costComplete && window.costComplete,
      sessionCount: total.sessionCount + window.sessionCount,
    }),
    usageWindow(0, 0),
  )
}

/** The summary for `providers`, with `totals` summed across them. */
export function providerUsageSummary(
  providers: ProviderUsagePayload[],
): ProviderUsageSummaryPayload {
  return {
    providers,
    totals: {
      today: sumWindows(providers.map((provider) => provider.windows.today)),
      week: sumWindows(providers.map((provider) => provider.windows.week)),
      monthToDate: sumWindows(providers.map((provider) => provider.windows.monthToDate)),
      last30Days: sumWindows(providers.map((provider) => provider.windows.last30Days)),
    },
    generatedAt: minutesAgo(0),
  }
}

export function liveWindow(
  overrides: Partial<LiveUsageWindowPayload> = {},
): LiveUsageWindowPayload {
  return {
    id: "five-hour",
    role: "primaryShort",
    kind: "fiveHour",
    scopeModel: null,
    usedPercent: 40,
    startsAt: minutesAgo(200),
    resetsAt: minutesFromNow(100),
    hasNonzeroUsageInCurrentPeriod: true,
    forecast: {
      unavailableReason: null,
      confidence: "medium",
      consumptionRate: 0.2,
      paceRatio: 1.1,
      paceTrend: 0.05,
      runwayAt: minutesFromNow(240),
      usedToday: 62,
    },
    ...overrides,
  }
}

export function liveProvider(
  overrides: Partial<LiveProviderUsagePayload> = {},
): LiveProviderUsagePayload {
  return {
    provider: "anthropic",
    accountKey: null,
    displayName: "Claude",
    support: "live",
    freshness: "fresh",
    sourceLabel: "Asked Claude directly",
    observedAt: minutesAgo(2),
    windows: [
      liveWindow(),
      liveWindow({
        id: "seven-day",
        role: "primaryLong",
        kind: "weekly",
        usedPercent: 63,
        startsAt: minutesAgo(4 * 24 * 60),
        resetsAt: minutesFromNow(3 * 24 * 60),
      }),
    ],
    extraUsage: null,
    resetCredits: null,
    plan: { name: "Max", tier: "20x" },
    accountUuid: null,
    accountEmail: null,
    ...overrides,
  }
}

export function liveUsageSummary(
  providers: LiveProviderUsagePayload[],
): LiveUsageSummaryPayload {
  return {
    providers,
    errors: [],
    meters: providers.map((provider) => ({
      provider: provider.provider,
      displayName: provider.displayName,
      shown: true,
    })),
    generatedAt: minutesAgo(0),
  }
}

/* -------------------------------------------------------------------------
 * Checks
 * ---------------------------------------------------------------------- */

export function checksReport(
  overrides: Partial<ChecksReportPayload> = {},
): ChecksReportPayload {
  return {
    evidenceSettled: true,
    estimatedTokenBurnBasisPoints: 1_625,
    categories: [
      {
        id: "cacheChurn",
        finding: 7,
        clean: 7,
        unavailable: 0,
        estimatedTokenBurnBasisPoints: 1_250,
      },
      {
        id: "sessionsOverDepth",
        finding: 0,
        clean: 14,
        unavailable: 0,
        estimatedTokenBurnBasisPoints: 0,
      },
    ],
    ...overrides,
  }
}
