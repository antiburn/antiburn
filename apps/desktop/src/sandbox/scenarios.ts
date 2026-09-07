/**
 * Named fixture sets for the UI sandbox. `?scenario=<name>` picks one.
 *
 * `default` mirrors the popover test: one session, one provider.
 * `busy` is a full week: a dozen sessions across three agents, one active,
 * one expensive, and live limits for two providers.
 */

import type { ChecksReportPayload, SessionHygienePayload } from "../lib/insightsIpc"
import {
  HEALTHY_STORAGE,
  type ActivityEntryPayload,
  type AppInfo,
  type AppSettings,
  type LiveUsageSummaryPayload,
  type ProviderUsageSummaryPayload,
  type ScanStatus,
  type SessionAnalysisPayload,
  type SessionLimitAllocationSummaryPayload,
  type StorageHealthPayload,
} from "../lib/ipc"
import {
  activityEntry,
  appInfo,
  checksReport,
  cost,
  liveProvider,
  liveUsageSummary,
  liveWindow,
  minutesAgo,
  minutesFromNow,
  providerUsage,
  providerUsageSummary,
  scanStatus,
  sessionAnalysis,
  sessionHygiene,
  sessionMetrics,
  settings,
  subagentMember,
  summary,
  tokens,
  usageWindow,
} from "./fixtures"

export interface Scenario {
  settings: AppSettings
  appInfo: AppInfo
  scanStatus: ScanStatus
  entries: ActivityEntryPayload[]
  /** Analyses by session id. A session absent here gets `sessionAnalysis(entry)`. */
  analyses: Record<string, SessionAnalysisPayload>
  /** Sub-agent analyses by sub-agent id. */
  subagentAnalyses: Record<string, SessionAnalysisPayload>
  /** Hygiene by session id. A session absent here is clean. */
  hygiene: Record<string, SessionHygienePayload>
  providerUsage: ProviderUsageSummaryPayload
  liveUsage: LiveUsageSummaryPayload
  allocations: SessionLimitAllocationSummaryPayload
  checksReport: ChecksReportPayload
  storageHealth: StorageHealthPayload
}

function defaultScenario(): Scenario {
  return {
    settings: settings(),
    appInfo: appInfo(),
    scanStatus: scanStatus(),
    entries: [activityEntry()],
    analyses: {},
    subagentAnalyses: {},
    hygiene: {},
    providerUsage: providerUsageSummary([providerUsage()]),
    liveUsage: liveUsageSummary([
      liveProvider({
        provider: "openai",
        displayName: "Codex",
        sourceLabel: "Asked Codex directly",
        plan: null,
        windows: [
          liveWindow({
            id: "seven-day",
            role: "primaryLong",
            kind: "weekly",
            usedPercent: 40,
            startsAt: null,
            resetsAt: null,
            hasNonzeroUsageInCurrentPeriod: false,
            forecast: {
              unavailableReason: "sparseHistory",
              confidence: null,
              consumptionRate: null,
              paceRatio: null,
              paceTrend: null,
              runwayAt: null,
              usedToday: null,
            },
          }),
        ],
      }),
    ]),
    allocations: { generatedAt: minutesAgo(0), allocations: [] },
    checksReport: checksReport(),
    storageHealth: HEALTHY_STORAGE,
  }
}

const OPUS = "claude-opus-4-6"
const SONNET = "claude-sonnet-5"
const HAIKU = "claude-haiku-4-5"
const CODEX = "gpt-5-codex"

interface BusyRow {
  id: string
  agent: string
  repo: string
  title: string
  minutesAgo: number
  usd: number
  models?: string[]
  overrides?: Partial<ActivityEntryPayload>
}

const BUSY_ROWS: BusyRow[] = [
  {
    id: "s-01",
    agent: "claude-code",
    repo: "antiburn",
    title: "Rebuild the session row as a Settings-style card",
    minutesAgo: 0,
    usd: 3.4,
    overrides: { isActive: true },
  },
  {
    id: "s-02",
    agent: "claude-code",
    repo: "antiburn",
    title: "Plan the UI sandbox: fixture mode and mouse overlay",
    minutesAgo: 14,
    usd: 48.1,
    models: [OPUS, SONNET],
    overrides: { forkChildCount: 1 },
  },
  {
    id: "s-03",
    agent: "codex",
    repo: "ai-barometer",
    title: "Fix the flaky CI job on the macOS runner",
    minutesAgo: 41,
    usd: 2.15,
    models: [CODEX],
  },
  {
    id: "s-04",
    agent: "cursor",
    repo: "cadence-marketing-site",
    title: "Tidy the onboarding copy",
    minutesAgo: 65,
    usd: 0.62,
    models: [SONNET],
    overrides: { surface: "ide_desktop" },
  },
  {
    id: "s-05",
    agent: "claude-code",
    repo: "antiburn",
    title: "Investigate the HUD z-index in fullscreen",
    minutesAgo: 130,
    usd: 5.9,
  },
  {
    id: "s-06",
    agent: "codex",
    repo: "antiburn",
    title: "Write the release notes for 0.4.0",
    minutesAgo: 180,
    usd: 0.88,
    models: [CODEX],
  },
  {
    id: "s-07",
    agent: "claude-code",
    repo: "synthesis",
    title: "Metabase usage report for the ProcurePro deck",
    minutesAgo: 300,
    usd: 12.3,
    overrides: { hasForkParent: true },
  },
  {
    id: "s-08",
    agent: "cursor",
    repo: "antiburn",
    title: "Fix the shimmer in dark mode",
    minutesAgo: 480,
    usd: 1.04,
    models: [SONNET],
    overrides: { surface: "ide_desktop" },
  },
  {
    id: "s-09",
    agent: "claude-code",
    repo: "ai-barometer",
    title: "Refresh the DB snapshot runbook",
    minutesAgo: 26 * 60,
    usd: 0.41,
    models: [HAIKU],
  },
  {
    id: "s-10",
    agent: "codex",
    repo: "antiburn",
    title: "Add the DCO check to CI",
    minutesAgo: 30 * 60,
    usd: 1.77,
    models: [CODEX],
  },
  {
    id: "s-11",
    agent: "claude-code",
    repo: "docs",
    title: "Explore the Fable vs Sol design",
    minutesAgo: 50 * 60,
    usd: 7.25,
  },
  {
    id: "s-12",
    agent: "claude-code",
    repo: "antiburn",
    title: "Nudge window text crop",
    minutesAgo: 74 * 60,
    usd: 2.02,
  },
]

function busyEntry(row: BusyRow): ActivityEntryPayload {
  const models = row.models ?? [OPUS]
  return activityEntry({
    agent: row.agent,
    sessionId: row.id,
    repo: row.repo,
    title: row.title,
    timestamp: minutesAgo(row.minutesAgo),
    cost: cost(row.usd),
    models,
    modelRuns: models.map((model) => ({ model })),
    ...row.overrides,
  })
}

function busyScenario(): Scenario {
  const entries = BUSY_ROWS.map(busyEntry)
  const expensive = entries[1]
  const active = entries[0]
  if (!expensive || !active) throw new Error("The busy scenario needs its first two rows")

  const members = [
    subagentMember({ subagentId: "sub-a" }),
    subagentMember({
      subagentId: "sub-b",
      label: "Draft the fixture factories against ipc.ts",
      cost: cost(1.6),
      tokens: tokens(1.6),
      startedAtEpoch: Math.floor((Date.now() - 2_400_000) / 1000),
    }),
  ]
  const subagentsUsd = 0.8 + 1.6
  const expensiveAnalysis = sessionAnalysis(expensive, {
    topLevelCost: cost(48.1 - subagentsUsd),
    subagentsCost: cost(subagentsUsd),
    subagentsTokens: tokens(subagentsUsd),
    modelRuns: [{ model: OPUS, thinkingMode: "adaptive" }, { model: SONNET }],
    orchestration: {
      orchestrating: true,
      orchestratorAgent: "claude-code",
      orchestratorSessionId: expensive.sessionId,
      subagentCount: members.length,
      members,
    },
    relations: {
      title: expensive.title,
      parent: null,
      children: [
        {
          identity: { agent: "claude-code", sessionId: "s-02-fork", wslDistro: null },
          title: "Plan the UI sandbox (fork)",
          available: false,
        },
      ],
    },
  })

  const subagentAnalyses = Object.fromEntries(
    members.map((member) => {
      const metrics = sessionMetrics({
        sessionId: member.subagentId,
        model: SONNET,
        durationSecs: 600,
        activeSecs: 540,
        eventCount: 40,
        tokensIn: 30_000,
        tokensOut: 4_000,
        peakContextTokens: 60_000,
        compactionCount: 0,
        cost: member.cost,
      })
      return [
        member.subagentId,
        sessionAnalysis(expensive, {
          summary: summary(metrics),
          title: member.label,
          cost: member.cost,
          topLevelCost: member.cost,
          inclusiveTokens: member.tokens,
          models: [SONNET],
          modelRuns: member.modelRuns,
        }),
      ]
    }),
  )

  return {
    settings: settings({ overviewLimitsExpanded: true }),
    appInfo: appInfo({ indexedSessions: 1_284, databaseBytes: 212_000_000 }),
    scanStatus: scanStatus({
      completedAgents: 3,
      totalAgents: 3,
      sessions: entries.length,
      finishedAt: minutesAgo(2),
      agents: [
        { agent: "claude-code", lastCompletedAt: minutesAgo(2), sessionsSeen: 7 },
        { agent: "codex", lastCompletedAt: minutesAgo(2), sessionsSeen: 3 },
        { agent: "cursor", lastCompletedAt: minutesAgo(2), sessionsSeen: 2 },
      ],
    }),
    entries,
    analyses: { [expensive.sessionId]: expensiveAnalysis },
    subagentAnalyses,
    hygiene: {
      [expensive.sessionId]: sessionHygiene({
        badges: [
          {
            id: "sessionOverdepth",
            status: "finding",
            notAssessedReason: null,
            findingEvidence: {
              kind: "sessionOverdepth",
              maxRequestContextTokens: 186_000,
              depthCapTokens: 150_000,
            },
          },
          {
            id: "modelOverthinking",
            status: "finding",
            notAssessedReason: null,
            findingEvidence: {
              kind: "modelOverthinking",
              tiers: [{ tier: "opus", mainLoopTurns: 61, delegatedTurns: 4 }],
            },
          },
          { id: "overpoweredSubagents", status: "clean", notAssessedReason: null },
          { id: "obsoleteModel", status: "clean", notAssessedReason: null },
          { id: "fastModeOveruse", status: "clean", notAssessedReason: null },
          { id: "excessCacheRehydration", status: "clean", notAssessedReason: null },
        ],
      }),
    },
    providerUsage: providerUsageSummary([
      providerUsage({
        windows: {
          today: usageWindow(51.5, 3),
          week: usageWindow(79.4, 8),
          monthToDate: usageWindow(214.7, 33),
          last30Days: usageWindow(268.2, 48),
        },
      }),
      providerUsage({
        provider: "openai",
        displayName: "OpenAI",
        windows: {
          today: usageWindow(2.15, 1),
          week: usageWindow(4.8, 3),
          monthToDate: usageWindow(19.6, 12),
          last30Days: usageWindow(24.1, 15),
        },
        lastActivityAt: minutesAgo(41),
      }),
    ]),
    liveUsage: liveUsageSummary([
      liveProvider(),
      liveProvider({
        provider: "openai",
        displayName: "Codex",
        sourceLabel: "Asked Codex directly",
        plan: { name: "Pro", tier: null },
        windows: [
          liveWindow({
            usedPercent: 22,
            forecast: { ...liveWindow().forecast, paceRatio: 0.7 },
          }),
          liveWindow({
            id: "seven-day",
            role: "primaryLong",
            kind: "weekly",
            usedPercent: 71,
            startsAt: minutesAgo(5 * 24 * 60),
            resetsAt: minutesFromNow(2 * 24 * 60),
          }),
        ],
      }),
    ]),
    allocations: {
      generatedAt: minutesAgo(0),
      allocations: [
        {
          agent: active.agent,
          sessionId: active.sessionId,
          wslDistro: null,
          metric: "fiveHour",
          provider: "anthropic",
          displayName: "Claude",
          accountKey: null,
          windowId: "five-hour",
          resetsAt: minutesFromNow(100),
          percent: 9,
        },
        {
          agent: active.agent,
          sessionId: active.sessionId,
          wslDistro: null,
          metric: "weekly",
          provider: "anthropic",
          displayName: "Claude",
          accountKey: null,
          windowId: "seven-day",
          resetsAt: minutesFromNow(3 * 24 * 60),
          percent: 2,
        },
        {
          agent: expensive.agent,
          sessionId: expensive.sessionId,
          wslDistro: null,
          metric: "weekly",
          provider: "anthropic",
          displayName: "Claude",
          accountKey: null,
          windowId: "seven-day",
          resetsAt: minutesFromNow(3 * 24 * 60),
          percent: 31,
        },
      ],
    },
    checksReport: checksReport({
      estimatedTokenBurnBasisPoints: 2_310,
      categories: [
        {
          id: "cacheChurn",
          finding: 4,
          clean: 8,
          unavailable: 0,
          estimatedTokenBurnBasisPoints: 1_400,
        },
        {
          id: "sessionsOverDepth",
          finding: 1,
          clean: 11,
          unavailable: 0,
          estimatedTokenBurnBasisPoints: 910,
        },
      ],
    }),
    storageHealth: HEALTHY_STORAGE,
  }
}

const SCENARIOS = new Map<string, () => Scenario>([
  ["default", defaultScenario],
  ["busy", busyScenario],
])

/** Build the scenario named by `?scenario=`. An unknown name falls back to `default`. */
export function pickScenario(search: string = window.location.search): Scenario {
  const name = new URLSearchParams(search).get("scenario") ?? "default"
  const build = SCENARIOS.get(name)
  if (!build) console.warn(`[sandbox] unknown scenario "${name}"; using "default"`)
  return (build ?? defaultScenario)()
}
