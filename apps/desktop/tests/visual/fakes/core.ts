import type { HygieneSummary } from "../../../src/lib/insightsIpc"
import type { AllowanceUsageSummaryPayload } from "../../../src/lib/providerUsageIpc"
import { emitFixtureEvent } from "./event"
import { fixtureDetailMap, fixtureIsland, fixtureTokenMap } from "./hud"

declare global {
  interface Window {
    __ANTIBURN_VISUAL_SETTINGS_CALLS__?: Array<{
      command: string
      args: Record<string, unknown> | undefined
    }>
  }
}

type FixtureState = "populated" | "empty" | "loading" | "error" | "long"
type FixtureFault = "session-analysis" | "scale-save" | "onboarding-bootstrap" | "peek-data"

const now = "2026-09-15T00:00:00.000Z"

function fixtureState(): FixtureState {
  const value = new URLSearchParams(window.location.search).get("state")
  return value === "empty" || value === "loading" || value === "error" || value === "long"
    ? value
    : "populated"
}

function fixtureFault(): FixtureFault | null {
  const override = window.__ANTIBURN_VISUAL_FAULT__
  const value =
    override === undefined ? new URLSearchParams(window.location.search).get("fault") : override
  return value === "session-analysis" ||
    value === "scale-save" ||
    value === "onboarding-bootstrap" ||
    value === "peek-data"
    ? value
    : null
}

const usageWindow = {
  tokensIn: 1_280_000,
  tokensOut: 184_000,
  cacheRead: 2_400_000,
  estimatedUsd: 18.42,
  costComplete: true,
  sessionCount: 12,
}

const settings = {
  interfaceScalePercent: 100,
  theme: "system",
  activityWindowDays: 7,
  sessionDataRetentionDays: -1,
  onboardingCompleted: false,
  launchAtLogin: true,
  autoUpdate: true,
  discoveryPaused: false,
  notificationsEnabled: true,
  notifyUpdateAvailable: true,
  notifyScanFailure: true,
  nudgePlacement: "menuBar",
  nudgeAutoDismissSecs: 10,
  notificationSound: true,
  nudgesRespectDnd: false,
  diskSpaceDisplay: "whenLow",
  diskSpaceThresholdGb: 50,
  notifyDiskSpaceLow: true,
  milestones5h: [50, 75, 90],
  milestonesWeekly: [50, 75, 90],
  liveUsageEnabled: true,
  liveUsageHiddenProviders: [],
  disabledAgents: [],
  analyticsEnabled: true,
  overviewLimitsExpanded: true,
  skillsMcpExpanded: false,
  sessionBadgeMetric: "cost",
  sessionFilter: "all",
  workingWeek: "seven",
}

const liveUsage = {
  providers: [
    {
      provider: "openai",
      accountKey: "fixture-account",
      displayName: "Codex",
      sourceLabel: "Codex account",
      freshness: "fresh",
      support: "live",
      windows: [
        {
          id: "five-hour",
          role: "primaryShort",
          kind: "rolling",
          scopeModel: null,
          usedPercent: 72,
          startsAt: "2026-09-14T20:00:00.000Z",
          resetsAt: "2026-09-15T01:00:00.000Z",
          elapsedFraction: 0.8,
          hasNonzeroUsageInCurrentPeriod: true,
          forecast: {
            unavailableReason: null,
            confidence: "high",
            consumptionRate: 14.2,
            paceRatio: 1.1,
            paceTrend: 1.03,
            runwayAt: "2026-09-15T01:35:00.000Z",
            usedToday: 53,
          },
        },
        {
          id: "seven-day",
          role: "primaryLong",
          kind: "weekly",
          scopeModel: null,
          usedPercent: 44,
          startsAt: "2026-09-09T00:00:00.000Z",
          resetsAt: "2026-09-16T00:00:00.000Z",
          elapsedFraction: 6 / 7,
          hasNonzeroUsageInCurrentPeriod: true,
          forecast: {
            unavailableReason: null,
            confidence: "medium",
            consumptionRate: 1.8,
            paceRatio: 0.8,
            paceTrend: 0.94,
            runwayAt: "2026-09-21T05:00:00.000Z",
            usedToday: 8,
          },
        },
      ],
      extraUsage: null,
      resetCredits: { availableCount: 1 },
      plan: { name: "Pro", tier: null },
    },
  ],
  errors: [],
  meters: [{ provider: "openai", displayName: "Codex", shown: true }],
  generatedAt: now,
}

const endEpoch = Date.parse(now) / 1000
const daySeconds = 86_400
const allowanceUsage: AllowanceUsageSummaryPayload = {
  accounts: [
    {
      provider: "openai",
      displayName: "Codex",
      accountKey: "fixture-account",
      plan: { name: "Pro", tier: null },
      utilization: {
        utilizationPercent: 58,
        weeklyWindowCount: 1,
        shortWindowCount: 1,
        modelWindowCount: 0,
      },
      chart: {
        shortWindows: [
          {
            startsAtEpoch: endEpoch - 14_400,
            resetsAtEpoch: endEpoch + 3_600,
            peakPercent: 72,
          },
        ],
        weeklyWindows: [
          {
            lane: "weekly",
            startsAtEpoch: endEpoch - 6 * daySeconds,
            resetsAtEpoch: endEpoch + daySeconds,
            points: [
              { atEpoch: endEpoch - 6 * daySeconds, percent: 0 },
              { atEpoch: endEpoch, percent: 44 },
            ],
          },
        ],
        rolling: [
          { atEpoch: endEpoch - 5 * daySeconds, percent: 30 },
          { atEpoch: endEpoch, percent: 58 },
        ],
      },
    },
  ],
  utilizationSpanDays: 30,
  rangeStartEpoch: endEpoch - 30 * daySeconds,
  rangeEndEpoch: endEpoch,
  generatedAt: now,
}

const providerUsage = {
  providers: [
    {
      provider: "openai",
      accountKey: "fixture-account",
      displayName: "Codex",
      state: "live",
      staleness: "fresh",
      windows: {
        today: usageWindow,
        week: { ...usageWindow, estimatedUsd: 64.3, sessionCount: 37 },
        monthToDate: { ...usageWindow, estimatedUsd: 211.8, sessionCount: 128 },
        last30Days: { ...usageWindow, estimatedUsd: 419.2, sessionCount: 261 },
      },
      agents: [],
      lastActivityAt: now,
    },
  ],
  totals: {
    today: usageWindow,
    week: { ...usageWindow, estimatedUsd: 64.3 },
    monthToDate: { ...usageWindow, estimatedUsd: 211.8 },
    last30Days: { ...usageWindow, estimatedUsd: 419.2 },
  },
  agents: [],
  generatedAt: now,
}

const entries = [
  {
    agent: "codex",
    sessionId: "fixture-active-session",
    repo: "antiburn",
    timestamp: "2026-09-15T00:00:00.000Z",
    isActive: true,
    surface: "cli",
    wslDistro: null,
    title: "Make every interface scale reachable without clipping",
    hasForkParent: false,
    forkChildCount: 2,
    cost: { inputUsd: 4.2, outputUsd: 2.1, cacheReadUsd: 0.3, totalUsd: 6.6 },
    models: ["gpt-5.6"],
    modelRuns: [{ model: "gpt-5.6", thinkingMode: "high" }],
  },
  {
    agent: "claude-code",
    sessionId: "fixture-long-session",
    repo: "desktop-interface-scale-quality-assurance-and-regression-prevention",
    timestamp: "2026-09-14T08:00:00.000Z",
    isActive: false,
    surface: "terminal",
    wslDistro: null,
    title:
      "Review constrained window behaviour with intentionally long titles, labels, provider names, and supporting descriptions",
    hasForkParent: true,
    forkChildCount: 0,
    cost: null,
    models: ["claude-sonnet"],
    modelRuns: [{ model: "claude-sonnet" }],
  },
]

function fixtureEntries(state: FixtureState) {
  if (state !== "long") return entries
  return entries.map((entry, index) =>
    index === 1
      ? {
          ...entry,
          repo: "desktop-interface-scale-quality-assurance-and-regression-prevention-with-a-deliberately-long-repository-name",
          title:
            "Review constrained window behaviour with intentionally long titles, labels, provider names, and supporting descriptions that must wrap without hiding a reachable action or spilling outside the resized window.",
        }
      : entry,
  )
}

const hudDetail = {
  reason: "show",
  bars: [
    {
      key: "openai:five-hour",
      label: "Codex 5-hour",
      percent: 72,
      resetsAt: "2026-09-15T01:00:00.000Z",
      color: "var(--color-burn)",
      expectedFraction: 0.8,
    },
    {
      key: "openai:week",
      label: "Codex weekly",
      percent: 44,
      resetsAt: "2026-09-16T00:00:00.000Z",
      color: "var(--color-system-blue)",
      expectedFraction: 0.5,
    },
  ],
  now: Date.parse(now),
  noMeterSelected: false,
}

const nudge = {
  id: "fixture-usage-milestone",
  kind: "usageMilestone",
  tone: "warning",
  title: "Codex 5-hour window is 72% used",
  subtitle: "You are on pace to reach your limit before it resets.",
  description:
    "This deterministic fixture keeps the notification content long enough to exercise wrapping and expanded actions.",
  recommendations: [
    "Save a checkpoint before the next large task.",
    "Switch to a lighter model for routine edits.",
  ],
  actions: [
    { id: "dismiss", label: "Dismiss", primary: false },
    {
      id: "open-usage",
      label: "Open usage",
      primary: true,
      target: { type: "providerUsage", provider: "openai", accountKey: "fixture-account" },
    },
  ],
}

function dataFor(command: string, args: Record<string, unknown> | undefined): unknown {
  const state = fixtureState()
  const fault = fixtureFault()
  const empty = state === "empty"
  const error = state === "error"
  const loading = state === "loading"
  if (loading && command.startsWith("get_")) return new Promise(() => undefined)
  if (error && command === "get_live_usage") {
    return {
      providers: [],
      errors: [
        {
          provider: "openai",
          reason: "authentication",
          message: "Fixture authentication error",
        },
      ],
      meters: [],
      generatedAt: now,
    }
  }
  if (fault === "session-analysis" && command === "get_session_analysis")
    return Promise.reject(new Error("Fixture session analysis failure"))
  if (fault === "scale-save" && command === "set_interface_scale")
    return Promise.reject(new Error("Fixture interface scale save failure"))
  if (fault === "onboarding-bootstrap" && command === "get_settings")
    return Promise.reject(new Error("Fixture onboarding bootstrap failure"))
  if (fault === "peek-data" && command === "get_popover_peek_data")
    return Promise.reject(new Error("Fixture preview data failure"))
  switch (command) {
    case "get_settings":
      return {
        ...settings,
        interfaceScalePercent:
          Number(new URLSearchParams(window.location.search).get("scale")) || 100,
      }
    case "get_main_window_visible":
      return true
    case "list_burn_check_snoozes":
      return []
    case "set_settings":
      return args?.settings ?? settings
    case "set_interface_scale": {
      const change = args?.change as { kind?: string; percent?: number } | undefined
      const percent =
        change?.kind === "set" ? change.percent : change?.kind === "increase" ? 125 : 100
      return { ...settings, interfaceScalePercent: percent }
    }
    case "app_info":
      return {
        appVersion: "0.5.2",
        debugBuild: true,
        arch: "arm64",
        pricingCatalogVersion: "fixture",
        schemaVersion: 1,
        dataDir: "/fixture",
        indexedSessions: 128,
        databaseBytes: 58_720_256,
        updatesSupported: false,
        analyticsSupported: false,
        analyticsEnvironmentDisabled: false,
        analyticsOperator: null,
      }
    case "list_recent_sessions":
      return empty ? [] : fixtureEntries(state)
    case "get_provider_usage":
      return empty
        ? { providers: [], totals: providerUsage.totals, agents: [], generatedAt: now }
        : providerUsage
    case "get_allowance_usage":
      return empty ? { ...allowanceUsage, accounts: [] } : allowanceUsage
    case "get_live_usage":
    case "refresh_live_usage":
      if (state === "long" && new URLSearchParams(window.location.search).has("island")) {
        return {
          ...liveUsage,
          providers: Array.from({ length: 6 }, (_, index) => ({
            ...liveUsage.providers[0],
            accountKey: `fixture-account-${index}`,
            displayName: `Codex account ${index + 1} with a long descriptive label`,
            windows: liveUsage.providers[0]!.windows.map((window) => ({
              ...window,
              id: `${window.id}-${index}`,
            })),
          })),
        }
      }
      return empty ? { providers: [], errors: [], meters: [], generatedAt: now } : liveUsage
    case "get_session_limit_allocations":
      return { allocations: [], generatedAt: now }
    case "get_storage_health":
      return { failing: false, message: null }
    case "get_scan_status":
      return {
        running: false,
        completedAgents: 2,
        totalAgents: 2,
        sessions: 128,
        finishedAt: now,
        cancelled: false,
        error: null,
        agents: empty
          ? []
          : [
              { agent: "codex", lastCompletedAt: now, sessionsSeen: 87 },
              { agent: "claude-code", lastCompletedAt: now, sessionsSeen: 41 },
            ],
        listChanged: false,
        reDescribed: 2,
      }
    case "get_folder_permissions":
      return { deferred: [], granted: [], supported: false }
    case "default_scan_roots":
      return ["/Users/fixture/.codex", "/Users/fixture/.claude"]
    case "list_scan_roots":
      return ["/Users/fixture/projects"]
    case "list_repositories":
      return empty
        ? []
        : [
            {
              key: "fixture-repo",
              repoName: "antiburn",
              fullName: "fixture/antiburn",
              status: "ready",
              repoRoot: "/Users/fixture/antiburn",
              suspectedPath: null,
              worktreeCount: 1,
              sessionCount: 48,
              wslDistro: null,
              enabled: true,
            },
          ]
    case "get_hygiene_summary":
      return {
        totalSessions: 128,
        settledSessions: 128,
        analyzedSessions: 82,
        failingSessions: 19,
        mostCommonFinding: "sessionOverdepth",
      } satisfies HygieneSummary
    case "get_checks_report":
      return {
        evidenceSettled: true,
        pendingEvidence: 0,
        estimatedTokenBurnBasisPoints: 1200,
        categories: new URLSearchParams(window.location.search).has("findings")
          ? fixtureCheckCategories()
          : [],
      }
    case "list_burn_check_targets":
      return { targets: [], samples: [], truncated: false }
    case "get_latest_session_activity":
      return Math.floor(Date.parse(now) / 1000)
    case "is_overlay_work_active":
      return true
    case "hud_island_state":
      return fixtureIsland()
    case "get_hud_token_map":
      return fixtureTokenMap()
    case "get_hud_detail_state":
      return {
        ...hudDetail,
        ...(empty ? { bars: [], noMeterSelected: true } : {}),
        map: fixtureDetailMap(),
        spend: fixtureTokenMap() ? "$0.12 per minute" : null,
        target:
          new URLSearchParams(window.location.search).get("detail") === "session"
            ? "fixture-map-0"
            : "usage",
        subagent: "fixture-worker",
      }
    case "get_popover_peek_state":
      return {
        generation: 1,
        target: { kind: "provider", provider: "openai", utcOffsetMinutes: 0 },
        rendererReady: true,
        visible: true,
        awaitingRetargetCommit: false,
        awaitingPresentation: false,
        awaitingConcealment: false,
      }
    case "get_popover_peek_data":
      return {
        kind: "provider",
        summary: providerUsage,
        live: empty ? { providers: [], errors: [], meters: [], generatedAt: now } : liveUsage,
      }
    case "nudge_ready":
      queueMicrotask(() =>
        emitFixtureEvent(
          "nudge:show",
          state === "long"
            ? {
                ...nudge,
                title:
                  "Codex 5-hour window is 72% used while a deliberately long fixture title tests notification wrapping",
              }
            : nudge,
        ),
      )
      return undefined
    default:
      return undefined
  }
}

export function isTauri(): boolean {
  return true
}

export async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (["open_settings_window", "set_settings", "set_interface_scale"].includes(command)) {
    window.__ANTIBURN_VISUAL_SETTINGS_CALLS__ ??= []
    window.__ANTIBURN_VISUAL_SETTINGS_CALLS__.push({ command, args })
  }
  if (command === "tear_off_overlay" || command === "hud_drag_ended") {
    window.__ANTIBURN_VISUAL_HUD_DRAGS__ ??= []
    window.__ANTIBURN_VISUAL_HUD_DRAGS__.push(command)
  }
  if (command === "resize_overlay_window") {
    window.__ANTIBURN_VISUAL_HUD_RESIZES__ ??= []
    window.__ANTIBURN_VISUAL_HUD_RESIZES__.push(args ?? {})
  }
  return dataFor(command, args) as T
}

/** Checks in every state, for the Enhance wizard. Add `findings` to the URL. */
function fixtureCheckCategories() {
  return [
    {
      id: "sessionsOverDepth",
      lifecycle: "failing",
      finding: 14,
      clean: 22,
      unavailable: 3,
      estimatedTokenBurnBasisPoints: 640,
    },
    {
      id: "unusedMcpServers",
      lifecycle: "failing",
      finding: 31,
      clean: 9,
      unavailable: 0,
      estimatedTokenBurnBasisPoints: 310,
    },
    {
      id: "modelOverthinking",
      lifecycle: "failing",
      finding: 6,
      clean: 40,
      unavailable: 2,
      estimatedTokenBurnBasisPoints: 120,
    },
    {
      id: "cacheChurn",
      lifecycle: "awaitingVerification",
      finding: 0,
      clean: 12,
      unavailable: 0,
      estimatedTokenBurnBasisPoints: null,
    },
    {
      id: "oldModelUsage",
      lifecycle: "passing",
      finding: 0,
      clean: 48,
      unavailable: 0,
      estimatedTokenBurnBasisPoints: 0,
    },
    {
      id: "overuseOfFastMode",
      lifecycle: "passing",
      finding: 0,
      clean: 48,
      unavailable: 0,
      estimatedTokenBurnBasisPoints: 0,
    },
    {
      id: "unusedSkills",
      lifecycle: "passing",
      finding: 0,
      clean: 30,
      unavailable: 0,
      estimatedTokenBurnBasisPoints: 0,
    },
    {
      id: "unusedBuiltInTools",
      lifecycle: null,
      finding: 0,
      clean: 0,
      unavailable: 48,
      estimatedTokenBurnBasisPoints: null,
    },
    {
      id: "overpoweredSubagents",
      lifecycle: null,
      finding: 0,
      clean: 0,
      unavailable: 48,
      estimatedTokenBurnBasisPoints: null,
    },
  ]
}
