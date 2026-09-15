import { describe, expect, it } from "vitest"

import type { SessionHygieneFindingEvidence, SessionHygienePayload } from "../insightsIpc"
import type { SessionAnalysisPayload } from "../ipc"
import type { SessionMetrics } from "../types/session"
import { sessionDiscussionPrompt, type SessionDiscussionInput } from "./sessionDiscussionPrompt"
import { INITIAL_SESSION_HYGIENE } from "./sessionHygiene"

const metrics: SessionMetrics = {
  agent: "claude-code",
  sessionId: "session-1",
  durationSecs: 3600,
  activeSecs: 1200,
  eventCount: 42,
  tokensIn: 1000,
  tokensOut: 200,
  peakContextTokens: 900,
  contextWindow: 200000,
  buckets: [],
  billableInputTokens: 100,
  billableOutputTokens: 200,
  billableCacheReadTokens: 300,
  billableCacheCreationTokens: 400,
}
const payload: SessionAnalysisPayload = {
  summary: {
    sessionCount: 1,
    avgDurationSecs: 3600,
    avgActiveSecs: 1200,
    tokensInTotal: 1000,
    tokensOutTotal: 200,
    peakContextTokens: 900,
    contextWindow: 200000,
    buckets: [],
    sessions: [metrics],
  },
  title: "Stored title",
  supportsAnalysis: true,
  wslDistro: null,
  isActive: false,
  cost: { totalUsd: 1.25, inputUsd: 0.25, outputUsd: 1, cacheReadUsd: 0, cacheWriteUsd: 0 },
  topLevelCost: null,
  subagentsCost: null,
  inclusiveTokens: {
    inputTokens: 110,
    outputTokens: 220,
    cacheReadTokens: 330,
    cacheCreationTokens: 440,
  },
  subagentsTokens: null,
  efficiency: null,
  models: ["claude-sonnet-4"],
  modelRuns: [{ model: "claude-sonnet-4", thinkingMode: "high" }],
  orchestration: null,
  relations: null,
  sourcePath: "/tmp/synthetic/session.jsonl",
  startedAtEpoch: null,
  analysisPending: false,
  analysisStale: false,
}
const input: SessionDiscussionInput = {
  subject: {
    agent: "claude-code",
    sessionId: "session-1",
    title: "List title",
    repo: "Synthetic",
  },
  payload,
  hygiene: INITIAL_SESSION_HYGIENE,
  loading: false,
  refreshing: false,
  error: false,
}

function prompt(over: Partial<SessionDiscussionInput> = {}) {
  return sessionDiscussionPrompt({ ...input, ...over })
}

describe("sessionDiscussionPrompt", () => {
  it("reports neutral session context and leaves the analysis question to the user", () => {
    const result = prompt()
    for (const expected of [
      "Title: Stored title",
      "ID: session-1",
      "Claude Code",
      "claude-sonnet-4/high",
      "/tmp/synthetic/session.jsonl",
      "Merged active time (idle gaps excluded): 20m",
      "Merged elapsed span (including idle gaps): 1h",
      "Inclusive events: 42",
      "API-equivalent, not an invoice): $1.25",
      "input 110; output 220; cache read 330; cache creation 440",
      "locally linked subagents (inclusive)",
      "Fork relatives are not added",
      "does not prove that no delegated work occurred",
      "# antiburn session context",
      "Transcript contents are not included.",
    ])
      expect(result).toContain(expected)
    expect(
      result.startsWith(
        "# antiburn session context\n\nSession metadata and analysis evidence. Transcript contents are not included.\n",
      ),
    ).toBe(true)
    expect(result.split("\n").filter((line) => line.startsWith("## "))).toEqual([
      "## Session",
      "## Scope and activity",
      "## API-equivalent cost and billable tokens",
      "## Freshness and coverage",
      "## Per-session burn-check evidence",
      "## Question / requirement for analysis",
    ])
    expect(result).not.toContain("List title")
    expect(result).not.toContain("Investigate the findings")
    expect(result).not.toContain("Prioritize supported findings")
    expect(result).not.toContain("Do not modify files")
    expect(
      result.endsWith(
        "## Question / requirement for analysis\n\n" +
          "Answer the question or address the requirement below as directly as possible, using the session details where relevant.\n\n" +
          "[Add your question or requirement here.]",
      ),
    ).toBe(true)
  })

  it("labels parent-plus-child counts and merged activity without adding child contexts or active times", () => {
    const child = {
      ...metrics,
      sessionId: "child-1",
      eventCount: 8,
      tokensIn: 500,
      tokensOut: 100,
      peakContextTokens: 1500,
      compactionCount: 3,
      cacheRehydrationCount: 4,
      cacheRoutingMissCount: 5,
    }
    // The payload contains merged times. The formatter does not calculate them from individual durations.
    const merged = {
      ...metrics,
      eventCount: metrics.eventCount + child.eventCount,
      tokensIn: metrics.tokensIn + child.tokensIn,
      tokensOut: metrics.tokensOut + child.tokensOut,
      activeSecs: 1500,
      durationSecs: 4200,
      compactionCount: 1,
      cacheRehydrationCount: 2,
      cacheRoutingMissCount: 0,
    }
    const result = prompt({
      payload: {
        ...payload,
        summary: { ...payload.summary!, sessions: [merged] },
        orchestration: {
          orchestrating: false,
          orchestratorAgent: "claude-code",
          orchestratorSessionId: "session-1",
          subagentCount: 1,
          members: [
            {
              agent: "claude-code",
              subagentId: child.sessionId,
              label: "Synthetic child",
              cost: null,
              tokens: null,
              startedAtEpoch: null,
              modelRuns: [],
            },
          ],
        },
      },
    })
    expect(result).toContain(
      "Event and input / output token scope: Selected session plus locally linked subagents (inclusive)",
    )
    expect(result).toContain("Inclusive events: 50")
    expect(result).toContain("Inclusive input / output tokens: 1,500 / 300")
    expect(result).toContain("Merged active time (idle gaps excluded): 25m")
    expect(result).toContain("Merged elapsed span (including idle gaps): 1h 10m")
    expect(result).toContain("not a sum of individual active times")
    expect(result).toContain("Peak context tokens: 900")
    expect(result).toContain(
      "Compactions / cache rehydrations / provider cache misses: 1 / 2 / 0",
    )
    expect(result).toContain(
      "only the selected transcript's context, not linked subagent contexts",
    )
    expect(result).not.toContain("Selected-transcript events:")
    expect(result).not.toContain("Selected-transcript input / output tokens:")
  })

  it("separates actual findings, confirmed clean, not-assessed reasons, and absent badges", () => {
    const hygiene: SessionHygienePayload = {
      evidenceState: "ready",
      badges: [
        {
          id: "sessionOverdepth",
          status: "finding",
          notAssessedReason: null,
          findingEvidence: {
            kind: "sessionOverdepth",
            maxRequestContextTokens: 250000,
            depthCapTokens: 200000,
          },
        },
        { id: "modelOverthinking", status: "clean", notAssessedReason: null },
        {
          id: "overpoweredSubagents",
          status: "notAssessed",
          notAssessedReason: "capabilityMissing",
        },
        { id: "obsoleteModel", status: "notAssessed", notAssessedReason: null },
        { id: "fastModeOveruse", status: "finding", notAssessedReason: null },
      ],
    }
    const result = prompt({ hygiene })
    expect(result.split("\n").filter((line) => line.startsWith("### "))).toEqual([
      "### Session overdepth — Session went too deep",
      "### Fast mode overuse — Fast mode overused",
      "### Other checks",
    ])
    const [findings, otherChecks] = result.split("### Other checks")
    expect(findings).not.toContain("- Passed —")
    expect(findings).not.toContain("- Not assessed —")
    expect(otherChecks).not.toContain("- Evidence:")
    expect(otherChecks).toContain("- Passed — Model overthinking.")
    expect(result).toContain("deepest request carried 250,000 tokens")
    expect(result).toContain("reviewed limit is 200,000")
    expect(result.split("\n")).toContain("- Passed — Model overthinking.")
    expect(result).toContain("this agent's logs don't record what this check needs")
    expect(result).not.toContain("capabilityMissing")
    expect(result).toContain("Not assessed — Obsolete model (reason unavailable)")
    expect(result).toContain("Evidence: Unavailable in the loaded payload")
    expect(result).toContain("Not assessed — Excess cache rehydration (no result available)")
  })

  it.each<SessionHygieneFindingEvidence>([
    {
      kind: "modelOverthinking",
      tiers: [{ tier: "max", mainLoopTurns: 2, delegatedTurns: 1 }],
    },
    {
      kind: "overpoweredSubagents",
      mainModels: ["claude-opus-4"],
      delegatedModels: ["claude-opus-4"],
    },
    {
      kind: "obsoleteModel",
      models: [{ model: "claude-sonnet-3", replacement: "claude-sonnet-4" }],
    },
    { kind: "fastModeOveruse", delegatedTurns: 4 },
    {
      kind: "excessCacheRehydration",
      repeatedTokens: 8000,
      paidTokens: 10000,
      thresholdMultiple: 3,
    },
  ])("uses the shared UI evidence for $kind", (findingEvidence) => {
    const result = prompt({
      hygiene: {
        evidenceState: "ready",
        badges: [
          {
            id: findingEvidence.kind,
            status: "finding",
            notAssessedReason: null,
            findingEvidence,
          },
        ],
      },
    })
    const evidence = result.split("- Evidence: ")[1]?.split("\n")[0]
    expect(evidence).toBeTruthy()
    expect(evidence).not.toContain("Unavailable")
    const expected = {
      modelOverthinking: "max reasoning for 3 turns",
      overpoweredSubagents: "premium subagents",
      obsoleteModel: "sonnet-4 became available",
      fastModeOveruse: "4 delegated turns",
      excessCacheRehydration: "8,000 of 10,000 paid context tokens repeated",
      sessionOverdepth: "",
    }[findingEvidence.kind]
    expect(evidence).toContain(expected)
  })

  it.each([
    "pending",
    "processing",
    "stale",
    "activelyGrowing",
    "failed",
    "unsupported",
    "ready",
  ] as const)("retains evidence state %s without inventing passes", (evidenceState) => {
    const result = prompt({ hygiene: { evidenceState, badges: [] } })
    expect(result).toContain(`Burn-check evidence: ${evidenceState}`)
    expect(result).not.toContain("- Passed —")
    expect(result.match(/no result available/g)).toHaveLength(6)
  })

  it.each(["stale", "activelyGrowing"] as const)(
    "retains loaded findings and passes with %s coverage limits",
    (evidenceState) => {
      const result = prompt({
        hygiene: {
          evidenceState,
          badges: [
            { id: "modelOverthinking", status: "clean", notAssessedReason: null },
            {
              id: "fastModeOveruse",
              status: "finding",
              notAssessedReason: null,
              findingEvidence: { kind: "fastModeOveruse", delegatedTurns: 7 },
            },
          ],
        },
      })
      expect(result).toContain("### Fast mode overuse — Fast mode overused")
      expect(result).toContain("7 delegated turns")
      expect(result).toContain("- Passed — Model overthinking.")
      expect(result).toContain("Stale or growing evidence can describe an earlier transcript")
      expect(result).toContain("Passed applies only to the assessed check and stored scope")
      expect(result).not.toContain("no current completed assessment")
      expect(result.match(/no result available/g)).toHaveLength(4)
    },
  )

  it.each(["cacheWrite", "uncachedInput"] as const)(
    "reports %s accounting as evidence, not prescribed guidance",
    (accounting) => {
      const result = prompt({
        hygiene: {
          evidenceState: "ready",
          badges: [
            {
              id: "excessCacheRehydration",
              status: "finding",
              notAssessedReason: null,
              accounting,
              findingEvidence: {
                kind: "excessCacheRehydration",
                paidTokens: 12000,
                repeatedTokens: 10000,
                thresholdMultiple: 4,
              },
            },
          ],
        },
      })
      expect(result).toContain("10,000 of 12,000 paid context tokens repeated")
      expect(result).toContain("finding threshold is 4×")
      expect(result).toContain(
        `Repeated paid context accounting: ${accounting === "cacheWrite" ? "cache writes" : "uncached input"}.`,
      )
      expect(result).not.toContain("Accounting guidance")
      expect(result).not.toContain("Reduce ")
    },
  )

  it("omits irrelevant freshness caveats for completed ready evidence", () => {
    const result = prompt({ hygiene: { evidenceState: "ready", badges: [] } })
    expect(result).not.toContain("Pending analysis metrics")
    expect(result).not.toContain("Pending or processing evidence")
    expect(result).not.toContain("Stale or growing evidence")
    expect(result).toContain("exact revision and assessment time are unavailable")
  })

  it("does not present pending placeholders as metrics or clean evidence", () => {
    const result = prompt({ payload: { ...payload, analysisPending: true }, loading: true })
    expect(result).toContain("Analysis pending: true")
    expect(result).toContain("Analysis loading: true")
    expect(result).toContain("Active now: Unavailable")
    expect(result).toContain("Inclusive events: Unavailable")
    expect(result).not.toContain("$1.25")
    expect(result).not.toContain("input 110")
    expect(result).not.toContain("- Passed —")
  })

  it("keeps stored stale results while disclosing independent freshness and failed refresh", () => {
    const result = prompt({
      payload: { ...payload, analysisStale: true, isActive: true },
      refreshing: true,
      error: true,
    })
    expect(result).toContain("analysis stale: true")
    expect(result).toContain("refreshing: true; load error: true")
    expect(result).toContain("Projected cost (API-equivalent, not an invoice): $1.25")
    expect(result).toContain("Analysis and check evidence load independently")
  })

  it("preserves unavailable metrics instead of substituting summary aggregates or zeros", () => {
    const result = prompt({
      payload: {
        ...payload,
        summary: { ...payload.summary!, sessions: [] },
        cost: null,
        inclusiveTokens: null,
      },
    })
    expect(result).toContain("Merged active time (idle gaps excluded): Unavailable")
    expect(result).toContain("Inclusive input / output tokens: Unavailable / Unavailable")
    expect(result).toContain("input Unavailable; output Unavailable")
    expect(result).not.toContain("$0.00")
    const missing = prompt({ payload: null })
    expect(missing).toContain("Source path (JSON string): Unavailable")
    expect(missing).toContain("analysis stale: Unavailable")
    expect(missing).toContain("Title: List title")
  })

  it("uses selected-subagent metrics without parent inclusive tokens", () => {
    const result = prompt({
      subject: {
        ...input.subject,
        subagent: { parentSessionId: "parent-1", subagentId: "session-1" },
      },
    })
    expect(result).toContain("Selected subagent transcript only")
    expect(result).toContain("Selected-transcript events: 42")
    expect(result).toContain("Selected-transcript input / output tokens: 1,000 / 200")
    expect(result).toContain("Selected-transcript active time (idle gaps excluded): 20m")
    expect(result).toContain("Selected-transcript elapsed span (including idle gaps): 1h")
    expect(result).toContain("Activity times describe only the selected subagent transcript")
    expect(result).not.toContain("Merged active time")
    expect(result).not.toContain("Inclusive events")
    expect(result).toContain("input 100; output 200; cache read 300; cache creation 400")
    expect(result).not.toContain("input 110")
    expect(result).toContain("Orchestrator session ID: parent-1")
  })

  it.each([
    String.raw`C:\Synthetic\agent logs\session.jsonl`,
    '/tmp/synthetic/"quoted"/newline\nand\ttab/```/session.jsonl',
  ])("preserves an exact JSON source path in Markdown: %s", (sourcePath) => {
    const result = prompt({ payload: { ...payload, sourcePath } })
    const encoded = result.split("```json\n")[1]!.split("\n```")[0]!
    expect(JSON.parse(encoded)).toBe(sourcePath)
  })

  it("uses the selected identity and loaded metadata rather than fixture values", () => {
    const result = prompt({
      subject: {
        agent: "codex",
        sessionId: "session-2",
        title: "New list title",
        repo: "Another synthetic repo",
        wslDistro: "Synthetic Linux",
      },
      payload: {
        ...payload,
        title: "New stored title",
        relations: { title: "Current relation title", parent: null, children: [] },
        models: ["synthetic-model"],
        modelRuns: [],
        sourcePath: "/tmp/synthetic/second.jsonl",
        summary: {
          ...payload.summary!,
          sessions: [
            metrics,
            { ...metrics, agent: "codex", sessionId: "session-2", eventCount: 73 },
          ],
        },
      },
    })
    expect(result).toContain("Title: Current relation title")
    expect(result).toContain("ID: session-2")
    expect(result).toContain("Agent: Codex (codex)")
    expect(result).toContain("Repository label: Another synthetic repo")
    expect(result).toContain("Origin: WSL (Synthetic Linux)")
    expect(result).toContain("Models / thinking modes: synthetic-model")
    expect(result).toContain("Inclusive events: 73")
    expect(result).not.toContain("Inclusive events: 42")
    expect(result).not.toContain("session-1")
    expect(result).not.toContain("claude-sonnet-4")
  })

  it("retains real zeros and unknown optional metrics separately", () => {
    const result = prompt({
      payload: {
        ...payload,
        summary: {
          ...payload.summary!,
          sessions: [{ ...metrics, activeSecs: 0, tokensIn: 0, compactionCount: 0 }],
        },
      },
    })
    expect(result).toContain("Merged active time (idle gaps excluded): 0s")
    expect(result).toContain("Inclusive input / output tokens: 0 / 200")
    expect(result).toContain(
      "Compactions / cache rehydrations / provider cache misses: 0 / Unavailable / Unavailable",
    )
    expect(result).toContain("Cache read: $0.00")
  })

  it("does not serialize transcript-bearing fields or fabricate database-only facts", () => {
    const result = prompt({
      payload: {
        ...payload,
        orchestration: {
          orchestrating: true,
          orchestratorAgent: "claude-code",
          orchestratorSessionId: "session-1",
          subagentCount: 1,
          members: [
            {
              agent: "claude-code",
              subagentId: "child-1",
              label: "PRIVATE_TRANSCRIPT_SENTINEL",
              cost: null,
              tokens: null,
              startedAtEpoch: null,
              modelRuns: [],
            },
          ],
        },
      },
    })
    expect(result).not.toContain("PRIVATE_TRANSCRIPT_SENTINEL")
    expect(result).toContain("Linked subagents reported: 1")
    expect(result).toContain(
      "Selected transcript only, API-equivalent: Unavailable; linked subagents only, API-equivalent: Unavailable",
    )
    expect(result).toContain("Git branch name and complete delegation lineage: Unavailable")
    expect(result).toContain("exact revision and assessment time are unavailable")
  })
})
