import type { ActivityEntryPayload, SessionAnalysisPayload } from "../../lib/ipc"
import type {
  BurnCheckDetectorId,
  BurnCheckTargetPayload,
  ChecksReportPayload,
  SessionHygienePayload,
} from "../../lib/insightsIpc"
import type {
  AllowanceUsageSummaryPayload,
  LiveUsageSummaryPayload,
  ProviderUsageSummaryPayload,
  ProviderUsageWindowPayload,
} from "../../lib/providerUsageIpc"
import type { SessionBucket } from "../../lib/types/session"

const now = new Date()
const epoch = Math.floor(now.getTime() / 1000)
const day = 86400
const iso = (seconds: number) => new Date(seconds * 1000).toISOString()

// Synthetic profiles reflect observed bursts, plateaus, and compaction resets.
function timeline(index: number, model: string): SessionBucket[] {
  const length = [100, 64, 48, 88, 56, 40, 96, 32][index % 8]!
  let context = 12000 + index * 2200
  return Array.from({ length }, (_, turn) => {
    const previous = context
    const pulse = ((turn * 17 + index * 11) % 29) / 29
    const quiet = (turn + index * 3) % 13 < 3
    const compaction =
      index % 2 === 0 && (turn === Math.floor(length * 0.44) || (index === 0 && turn === 78))
    const rehydration =
      index % 3 !== 2 &&
      (turn === Math.floor(length * 0.3) || turn === Math.floor(length * 0.84))
    const growth = quiet ? 0 : Math.round(500 + pulse ** 2 * (index % 2 === 0 ? 13000 : 4200))
    context = compaction ? Math.round(previous * 0.32) : previous + growth
    const routingMiss = index % 3 === 0 && turn === Math.floor(length * 0.65)
    const rewrite =
      rehydration || routingMiss
        ? Math.round(context * 0.72)
        : quiet
          ? 0
          : Math.round(pulse * 160)
    const output = quiet ? 0 : Math.round(180 + (1 - pulse) ** 3 * 3800)
    const fresh = growth + rewrite
    const write = Math.round(fresh * 0.7)
    const read = Math.max(0, previous - rewrite)
    const cost = {
      inputUsd: (fresh - write) * 0.000003,
      outputUsd: output * 0.000015,
      cacheReadUsd: read * 0.0000003,
      cacheWriteUsd: write * 0.00000375,
      totalUsd: 0,
    }
    cost.totalUsd = cost.inputUsd + cost.outputUsd + cost.cacheReadUsd + cost.cacheWriteUsd
    return {
      tokensIn: fresh,
      tokensOut: output,
      subagentTokens: 0,
      contextTokens: context,
      isCompactionBoundary: compaction,
      cacheReadTokens: read,
      cacheWriteTokens: write,
      rewriteTokens: rewrite,
      isCacheRehydration: rehydration,
      isCacheRoutingMiss: routingMiss,
      secsSincePriorTurn: rehydration ? 1800 : quiet ? 0 : Math.round(12 + pulse * 110),
      subagentLaunches: 0,
      userPrompts: turn === 0 || turn % 17 === 0 ? 1 : 0,
      lastTool: quiet ? null : ["Read", "Bash", "Edit", "Grep"][turn % 4]!,
      model,
      thinkingMode: index === 0 ? "high" : "medium",
      speed: "standard",
      hasThinking: !quiet,
      compactionTrigger: compaction ? "auto" : null,
      compactionPreTokens: compaction ? previous : null,
      compactionPostTokens: compaction ? context : null,
      cost,
    }
  })
}

function totals(buckets: SessionBucket[]) {
  const sum = (value: (bucket: SessionBucket) => number) =>
    buckets.reduce((total, bucket) => total + value(bucket), 0)
  const tokens = {
    inputTokens: sum((b) => b.tokensIn - b.cacheWriteTokens),
    outputTokens: sum((b) => b.tokensOut),
    cacheReadTokens: sum((b) => b.cacheReadTokens),
    cacheCreationTokens: sum((b) => b.cacheWriteTokens),
  }
  const cost = {
    inputUsd: sum((b) => b.cost!.inputUsd),
    outputUsd: sum((b) => b.cost!.outputUsd),
    cacheReadUsd: sum((b) => b.cost!.cacheReadUsd),
    cacheWriteUsd: sum((b) => b.cost!.cacheWriteUsd),
    totalUsd: sum((b) => b.cost!.totalUsd),
  }
  const rewriteUsd = sum((b) => (b.rewriteTokens ?? 0) * (0.3 * 0.000003 + 0.7 * 0.00000375))
  return {
    tokens,
    cost,
    efficiency: {
      totalUsd: cost.totalUsd,
      newWorkUsd: cost.totalUsd - cost.cacheReadUsd - rewriteUsd,
      carryUsd: cost.cacheReadUsd,
      rewriteUsd,
      growthTokens: sum((b) => b.tokensIn - (b.rewriteTokens ?? 0)),
      outputTokens: tokens.outputTokens,
      pricedTurns: buckets.length,
      unpricedTurns: 0,
    },
    durationSecs: sum((b) => b.secsSincePriorTurn ?? 0),
    activeSecs: sum((b) => Math.min(b.secsSincePriorTurn ?? 0, 120)),
    peakContextTokens: Math.max(...buckets.map((b) => b.contextTokens)),
    tokensIn: sum((b) => b.tokensIn),
  }
}

const featuredSessions = [
  ["claude-code", "Build the project dashboard", "studio", "claude-sonnet-4-6"],
  ["codex", "Add keyboard navigation", "studio", "gpt-5.4"],
  ["claude-code", "Fix the session export", "workbench", "claude-sonnet-4-6"],
  ["codex", "Review the caching strategy", "workbench", "gpt-5.4"],
  ["claude-code", "Polish the settings controls", "studio", "claude-sonnet-4-6"],
  ["codex", "Update the test fixtures", "notebook", "gpt-5.4"],
  ["claude-code", "Add source discovery", "notebook", "claude-sonnet-4-6"],
  ["codex", "Check the build pipeline", "studio", "gpt-5.4"],
]
const sessionTasks = [
  "Refine",
  "Test",
  "Review",
  "Implement",
  "Debug",
  "Document",
  "Optimize",
  "Validate",
]
const sessionAreas = [
  "search results",
  "API pagination",
  "export controls",
  "cache invalidation",
  "keyboard shortcuts",
  "source discovery",
  "notification rules",
  "accessibility",
  "error recovery",
  "provider meters",
  "session filters",
]
export const sessions: (ActivityEntryPayload & {
  workingFolder: string
  additionalRepos: string[]
})[] = Array.from(
  { length: 96 },
  (_, index) =>
    featuredSessions[index] ?? [
      index % 2 === 0 ? "claude-code" : "codex",
      `${sessionTasks[index % sessionTasks.length]} ${sessionAreas[Math.floor((index - 8) / 8) % sessionAreas.length]}`,
      ["studio", "workbench", "notebook"][index % 3]!,
      index % 2 === 0 ? "claude-sonnet-4-6" : "gpt-5.4",
    ],
).map(([agent, title, repo, model], index) => {
  let summary: ReturnType<typeof totals> | undefined
  const getSummary = () => (summary ??= totals(timeline(index, model!)))
  return {
    agent: agent!,
    sessionId: `prototype-session-${index + 1}`,
    repo: index === 1 || index === 4 ? "" : repo!,
    workingFolder:
      index === 1
        ? "~/Writing/notes"
        : index === 4 || index === 7
          ? "~/Projects"
          : `~/Projects/${repo}`,
    additionalRepos: index === 7 ? ["workbench", "notebook"] : [],
    remoteHostId:
      index === 2 || index === 5
        ? `prototype-remote-${index}`
        : index === 3 || index === 6
          ? `prototype-cloud-${index}`
          : null,
    timestamp: iso(
      epoch - (index === 0 ? 60 : index < 8 ? index * 14000 : 98000 + (index - 7) * 21600),
    ),
    isActive: index === 0,
    surface: "cli",
    wslDistro: null,
    title:
      index === 1
        ? "Draft research notes in a regular folder"
        : index === 4
          ? "Plan work from the Projects parent folder"
          : index === 7
            ? "Update dashboard across three repositories"
            : title!,
    hasForkParent: false,
    forkChildCount: 0,
    get cost() {
      return getSummary().cost
    },
    get totalTokens() {
      return Object.values(getSummary().tokens).reduce((sum, value) => sum + value, 0)
    },
    models: [model!],
    modelRuns: [{ model: model!, thinkingMode: index === 0 ? "high" : "medium" }],
  }
})

const detectorIds: BurnCheckDetectorId[] = [
  "unusedSkills",
  "unusedBuiltInTools",
  "unusedMcpServers",
  "modelOverthinking",
  "oldModelUsage",
  "overuseOfFastMode",
  "overpoweredSubagents",
  "sessionsOverDepth",
  "cacheChurn",
]

export const report: ChecksReportPayload = {
  evidenceSettled: true,
  windowSessions: sessions.length,
  pendingEvidence: 0,
  deferredEvidence: 0,
  estimatedTokenBurnBasisPoints: 1600,
  categories: detectorIds.map((id, index) => ({
    id,
    finding: index < 4 ? 1 : 0,
    clean: index < 4 ? sessions.length - 3 : index < 6 ? 0 : sessions.length - 2,
    unavailable: index >= 4 && index < 6 ? sessions.length : 2,
    estimatedTokenBurnBasisPoints: index === 0 ? 1400 : index < 4 ? 60 : index < 6 ? null : 0,
    lifecycle: index < 4 ? "failing" : index < 6 ? null : "passing",
    agents: ["claude-code", "codex"],
  })),
  installedChecks: [],
}

export function hygiene(sessionId: string): SessionHygienePayload {
  const unlinked = sessions.find((session) => session.sessionId === sessionId)?.repo === ""
  const finding = sessionId === sessions[0]!.sessionId
  return {
    evidenceState: "ready",
    badges: (
      [
        "sessionOverdepth",
        "modelOverthinking",
        "overpoweredSubagents",
        "obsoleteModel",
        "fastModeOveruse",
        "excessCacheRehydration",
      ] as const
    ).map((id) => ({
      id,
      status: unlinked
        ? "notAssessed"
        : finding && id === "modelOverthinking"
          ? "finding"
          : "clean",
      notAssessedReason: unlinked ? "incompleteEvidence" : null,
      ...(finding && id === "modelOverthinking"
        ? {
            findingEvidence: {
              kind: "modelOverthinking" as const,
              tiers: [{ tier: "high", mainLoopTurns: 12, delegatedTurns: 0 }],
            },
          }
        : {}),
    })),
    unusedResources: finding
      ? {
          skills: [
            { name: "release-notes", costUsd: 0.17 },
            { name: "database-review", costUsd: 0.08 },
          ],
          mcpServers: [{ name: "design-assets", costUsd: 0.03 }],
          builtInTools: [{ name: "WebSearch", costUsd: 0.01 }],
        }
      : { skills: [], mcpServers: [], builtInTools: [] },
  }
}

export function analysis(sessionId: string): SessionAnalysisPayload {
  const entry = sessions.find((session) => session.sessionId === sessionId) ?? sessions[0]!
  const index = sessions.indexOf(entry)
  const buckets = timeline(index, entry.models[0]!)
  const { cost, tokens, efficiency, durationSecs, activeSecs, peakContextTokens, tokensIn } =
    totals(buckets)
  const metrics = {
    agent: entry.agent,
    sessionId: entry.sessionId,
    durationSecs,
    activeSecs,
    eventCount: buckets.length * 2,
    tokensIn,
    tokensOut: tokens.outputTokens,
    peakContextTokens,
    contextAvailable: true,
    contextWindow: 200000,
    contextWindowSource: "reported" as const,
    compactionCount: buckets.filter((b) => b.isCompactionBoundary).length,
    cacheRehydrationCount: buckets.filter((b) => b.isCacheRehydration).length,
    cacheRoutingMissCount: buckets.filter((b) => b.isCacheRoutingMiss).length,
    model: entry.models[0]!,
    billableInputTokens: tokens.inputTokens,
    billableOutputTokens: tokens.outputTokens,
    billableCacheReadTokens: tokens.cacheReadTokens,
    billableCacheCreationTokens: tokens.cacheCreationTokens,
    cost,
    efficiency,
    buckets,
    initialContext: {
      sources: [
        {
          source: "skill_instructions" as const,
          sourceName: "release-notes",
          tokenCount: 2400,
          useCount: 0,
        },
        {
          source: "mcp_instructions" as const,
          sourceName: "design-assets",
          tokenCount: 1600,
          useCount: 0,
        },
        { source: "builtin_tool" as const, sourceName: "Read", tokenCount: 500, useCount: 16 },
        { source: "builtin_tool" as const, sourceName: "Edit", tokenCount: 400, useCount: 12 },
      ],
      totalTokens: 4900,
    },
  }
  return {
    summary: {
      sessionCount: 1,
      compactionCount: metrics.compactionCount,
      cacheRehydrationCount: metrics.cacheRehydrationCount,
      avgDurationSecs: durationSecs,
      avgActiveSecs: activeSecs,
      tokensInTotal: tokensIn,
      tokensOutTotal: tokens.outputTokens,
      peakContextTokens,
      contextAvailable: true,
      contextWindow: 200000,
      buckets,
      sessions: [metrics],
    },
    supportsAnalysis: true,
    title: entry.title,
    wslDistro: null,
    isActive: entry.isActive,
    cost,
    topLevelCost: cost,
    subagentsCost: null,
    inclusiveTokens: tokens,
    subagentsTokens: null,
    efficiency,
    models: entry.models,
    modelRuns: entry.modelRuns,
    orchestration: null,
    relations: null,
    sourcePath: null,
    projectPath: null,
    startedAtEpoch: Math.floor(Date.parse(entry.timestamp) / 1000) - durationSecs,
    analysisPending: false,
    analysisStale: false,
    calledTools: [
      { name: "Read", kind: "tool", calls: 16 },
      { name: "Edit", kind: "tool", calls: 12 },
      { name: "Bash", kind: "tool", calls: 8 },
    ],
  }
}

export function targets(detector: BurnCheckDetectorId): BurnCheckTargetPayload[] {
  if (!report.categories.find((category) => category.id === detector)?.finding) return []
  const resource =
    {
      unusedSkills: "release-notes",
      unusedMcpServers: "design-assets",
      unusedBuiltInTools: "WebSearch",
      modelOverthinking: "reasoning effort",
    }[detector as "unusedSkills"] ?? "reasoning effort"
  const resourceKind =
    detector === "unusedSkills"
      ? "skill"
      : detector === "unusedMcpServers"
        ? "mcpServer"
        : detector === "unusedBuiltInTools"
          ? "builtInTool"
          : "reasoning"
  const entry = sessions[0]!
  return [
    {
      findingId: `prototype-${detector}`,
      actionId: `prototype-action-${detector}`,
      finding: {
        detector,
        agent: "claude-code",
        sourceFormat: "claudeJsonl",
        observation:
          detector === "modelOverthinking"
            ? "High reasoning was used for 12 routine editing turns."
            : `${resource} was loaded in 40 requests and was never used.`,
        labels: [],
        omitted: 0,
      },
      display: {
        resourceKind,
        resourceIdentity: resource,
        currentValue: detector === "modelOverthinking" ? "high" : "enabled",
        replacementValue: detector === "modelOverthinking" ? "medium" : "disabled",
        scopeKind: "global",
        quantity: 40,
        quantityUnit: "turns",
        observationCount: 40,
        firstObservedAtMs: (epoch - 3600) * 1000,
        lastObservedAtMs: epoch * 1000,
        estimateMethod: null,
        estimatedOpportunity: null,
        estimatedTokenBurnBasisPoints: 1400,
        verificationLimit: "freshEvidenceFromSameSourceAndTarget",
      },
      occurrenceCount: 1,
      affectedSessionCount: 1,
      autoFix: { status: "available" },
      promptFix: { status: "available" },
      watch: null,
      evidenceAvailable: false,
      coverageLimits: ["currentPublishedEvidenceOnly"],
      expiresAtEpoch: epoch + day,
      samples: [
        {
          ...entry,
          title: entry.title!,
          surface: "cli",
          navigationHandle: entry.sessionId,
          observedAtMs: epoch * 1000,
          hygiene: hygiene(entry.sessionId),
        },
      ],
    },
  ]
}

// Keep the irregular sample stable across reloads so click-through comparisons agree.
const dailySpendCents = [
  [84, 210],
  [1360, 95],
  [420, 680],
  [0, 0],
  [57, 0],
  [2400, 730],
  [310, 1180],
  [780, 64],
  [0, 225],
  [165, 39],
  [920, 1860],
  [345, 260],
  [70, 0],
  [0, 0],
  [1720, 440],
  [650, 135],
  [135, 890],
  [2850, 1060],
  [410, 215],
  [0, 48],
  [95, 0],
  [1240, 390],
  [530, 1510],
  [260, 180],
  [0, 0],
  [1860, 680],
  [395, 2450],
  [120, 96],
  [720, 360],
  [470, 185],
]
const spendWindow = (cents: number): ProviderUsageWindowPayload => ({
  tokensIn: cents * 1800,
  tokensOut: cents * 110,
  cacheRead: cents * 5400,
  estimatedUsd: cents / 100,
  costComplete: true,
  sessionCount: cents === 0 ? 0 : Math.ceil(cents / 400),
})
function sumSpend(windows: ProviderUsageWindowPayload[]): ProviderUsageWindowPayload {
  return {
    tokensIn: windows.reduce((sum, value) => sum + value.tokensIn, 0),
    tokensOut: windows.reduce((sum, value) => sum + value.tokensOut, 0),
    cacheRead: windows.reduce((sum, value) => sum + value.cacheRead, 0),
    estimatedUsd:
      Math.round(windows.reduce((sum, value) => sum + (value.estimatedUsd ?? 0), 0) * 100) /
      100,
    costComplete: true,
    sessionCount: windows.reduce((sum, value) => sum + value.sessionCount, 0),
  }
}
const spendDays = dailySpendCents.map((cents, index) => {
  const agents = cents.map((value, agentIndex) => ({
    ...spendWindow(value),
    agent: agentIndex === 0 ? "claude-code" : "codex",
  }))
  return {
    ...sumSpend(agents),
    localDate: iso(epoch - (29 - index) * day).slice(0, 10),
    agents,
  }
})
function spendWindows(agentIndex?: number) {
  const windows = spendDays.map((value) =>
    agentIndex == null ? value : value.agents[agentIndex]!,
  )
  return {
    today: windows.at(-1)!,
    week: sumSpend(windows.slice(-7)),
    monthToDate: sumSpend(
      windows.filter(
        (_, index) =>
          spendDays[index]!.localDate.slice(0, 7) === spendDays.at(-1)!.localDate.slice(0, 7),
      ),
    ),
    last30Days: sumSpend(windows),
  }
}
export const usage: ProviderUsageSummaryPayload = {
  providers: ["anthropic", "openai"].map((provider, index) => ({
    provider,
    accountKey: `sample-${provider}`,
    displayName: index === 0 ? "Claude" : "Codex",
    state: "estimated",
    staleness: "fresh",
    windows: spendWindows(index),
    agents: [],
    lastActivityAt: iso(epoch),
  })),
  totals: spendWindows(),
  days: spendDays,
  previousDays: [],
  generatedAt: iso(epoch),
}

// Synthetic allowance history follows uneven work bursts and independent quota resets.
const weeklyLevels = [
  [0, 0, 8, 52, 56, 56, 56, 56, 56, 56, 56, 56],
  [0, 1, 1, 1, 2, 4, 4, 9, 10, 10, 14, 17],
  [0, 12, 12, 12, 12, 12, 12, 61, 62, 62, 62, 87],
  [0, 7, 8, 8, 9, 9, 19, 19, 36, 36, 98, 98],
  [0, 6, 15, 15, 24, 24, 51, 74, 76, 76, 88, 94],
]
const weeklyOffsets = [0, 0.08, 0.12, 0.2, 0.27, 0.38, 0.46, 0.53, 0.61, 0.73, 0.89, 1]
const shortPeaks = [
  0, 0, 8, 0, 54, 0, 0, 3, 1, 0, 0, 0, 0, 2, 0, 5, 0, 11, 0, 9, 0, 0, 0, 36, 1, 8, 0, 0, 2, 0,
  0, 4, 0, 12, 0, 38, 0, 80, 0, 0, 0, 1, 0, 0, 3, 0, 46, 2, 0, 24, 38, 0, 1, 0, 0, 0, 0, 20, 0,
  30, 0, 0, 48, 8, 0, 0, 0, 0, 0, 96, 3, 43, 0, 5, 18, 0, 6, 0, 0, 0, 14, 3, 0, 4, 12, 0, 0, 0,
  0, 0,
]

const allowance: AllowanceUsageSummaryPayload = {
  utilizationSpanDays: 28,
  rangeStartEpoch: epoch - 30 * day,
  rangeEndEpoch: epoch,
  generatedAt: iso(epoch),
  accounts: ["anthropic", "openai"].map((provider, index) => ({
    provider,
    displayName: index === 0 ? "Claude" : "Codex",
    accountKey: `sample-${provider}`,
    plan: { name: index === 0 ? "max" : "pro", tier: index === 0 ? "5x" : null },
    utilization: {
      utilizationPercent: index === 0 ? 9 : 20,
      weeklyWindowCount: 4,
      shortWindowCount: shortPeaks.filter((value) => value > 0).length,
      modelWindowCount: 0,
    },
    chart: {
      shortWindows: shortPeaks.flatMap((peak, i) => {
        if (peak === 0) return []
        const startsAtEpoch = epoch - (30 - (i + 0.3) / 3) * day
        return [
          {
            startsAtEpoch,
            resetsAtEpoch: startsAtEpoch + 18000,
            peakPercent:
              index === 0 ? peak : Math.min(100, Math.round(peak * 0.68 + (i % 7) * 3)),
          },
        ]
      }),
      weeklyWindows: weeklyLevels.map((levels, week) => {
        const startsAtEpoch = epoch - (30 - week * 7) * day
        const resetsAtEpoch = startsAtEpoch + 7 * day
        const visibleSpan = Math.min(epoch, resetsAtEpoch) - startsAtEpoch
        return {
          lane: "weekly",
          startsAtEpoch,
          resetsAtEpoch,
          points: levels.flatMap((percent, i) => {
            const atEpoch = Math.round(startsAtEpoch + weeklyOffsets[i]! * visibleSpan)
            const scale = index === 0 ? 1 : week === 4 ? 53 / 94 : 0.58 + week * 0.07
            const point = { atEpoch, percent: Math.round(percent * scale) }
            if (i === 0) return [point]
            return [
              { atEpoch: atEpoch - 300, percent: Math.round(levels[i - 1]! * scale) },
              point,
            ]
          }),
        }
      }),
      rolling: Array.from({ length: 31 }, (_, i) => ({
        atEpoch: epoch - (30 - i) * day,
        percent:
          index === 0
            ? [
                2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 3, 3, 3, 4, 4, 4, 4, 5, 6, 7,
                7, 7, 8, 9,
              ][i]!
            : [
                4, 4, 4, 4, 5, 5, 5, 5, 6, 6, 6, 6, 7, 7, 7, 8, 8, 9, 9, 10, 10, 11, 12, 12, 13,
                14, 15, 16, 18, 19, 20,
              ][i]!,
      })),
    },
  })),
}

export const liveUsage: LiveUsageSummaryPayload = {
  providers: allowance.accounts.map((account, index) => ({
    provider: account.provider,
    accountKey: account.accountKey,
    displayName: account.displayName,
    support: "live",
    freshness: "fresh",
    sourceLabel: "Prototype sample",
    observedAt: iso(epoch),
    plan: account.plan,
    accountUuid: null,
    accountEmail: null,
    extraUsage: null,
    resetCredits: null,
    windows: ["five-hour", "seven-day"].map((id, i) => ({
      id,
      role: i === 0 ? "primaryShort" : "primaryLong",
      kind: i === 0 ? "rolling" : "weekly",
      scopeModel: null,
      usedPercent: i === 0 ? (index === 0 ? 0 : 7) : index === 0 ? 94 : 53,
      startsAt: iso(epoch - (i === 0 ? 9000 : day * 2)),
      resetsAt: iso(epoch + (i === 0 ? 9000 : day * 5)),
      elapsedFraction: 0.58,
      hasNonzeroUsageInCurrentPeriod: true,
      forecast: {
        unavailableReason: "sparseHistory",
        confidence: null,
        consumptionRate: null,
        paceRatio: null,
        paceTrend: null,
        runwayAt: null,
        usedToday: null,
      },
    })),
  })),
  errors: [],
  meters: allowance.accounts.map((account) => ({
    provider: account.provider,
    displayName: account.displayName,
    shown: true,
    detection: "signedIn",
  })),
  generatedAt: iso(epoch),
}
