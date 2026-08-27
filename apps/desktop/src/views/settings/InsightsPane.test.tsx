import { fireEvent, render, screen } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import type {
  InsightsCategoryPayload,
  InsightsCoveragePayload,
  InsightsReportPayload,
} from "../../lib/insightsIpc"
import { InsightsPane } from "./InsightsPane"

/**
 * The pane's presentation contract. The load-bearing assertions are the
 * FR-12 ones: the coverage denominator is presented separately from the
 * assessed cohort, and no row outside the cohort — pending, processing,
 * failed, unsupported, stale, unknown-start — ever reads as assessed or
 * as clean.
 */

const invoke = vi.hoisted(() => vi.fn())

vi.mock("@tauri-apps/api/core", () => ({ invoke, isTauri: () => true }))

const CATEGORY_IDS = [
  "sessionsOverDepth",
  "modelOverthinking",
  "overpoweredSubagents",
  "unusedMcpServers",
  "unusedBuiltInTools",
  "unusedSkills",
  "oldModelUsage",
  "overuseOfFastMode",
  "cacheChurn",
] as const

function coverage(overrides: Partial<InsightsCoveragePayload> = {}): InsightsCoveragePayload {
  return {
    discovered: 0,
    unknownStart: 0,
    pending: 0,
    processing: 0,
    failed: 0,
    unsupported: 0,
    stale: 0,
    ready: 0,
    activelyGrowing: 0,
    awaitingProviderSupport: 0,
    ...overrides,
  }
}

function notAssessedCategories(): InsightsCategoryPayload[] {
  return CATEGORY_IDS.map((id) => ({
    id,
    eligible: 0,
    assessed: 0,
    status: "notAssessed",
    findingSessions: null,
    notAssessedReason: "noSessionsInWindow",
  }))
}

function report(overrides: Partial<InsightsReportPayload> = {}): InsightsReportPayload {
  return {
    environmentKey: "native",
    windowStartEpoch: 100,
    windowEndEpoch: 200,
    computedAtEpoch: 200,
    coverage: coverage(),
    assessedSessions: 0,
    categories: notAssessedCategories(),
    quotaPressure: { assessed: false, findings: null },
    catalogRevision: 1,
    ...overrides,
  }
}

const STATUS = { calculating: false, pending: 0, processing: 0 }

function mockCommands(overrides: Record<string, unknown> = {}) {
  invoke.mockImplementation((command: string) => {
    if (command in overrides) {
      const override = overrides[command]
      if (override instanceof Error) return Promise.reject(override)
      if (override instanceof Promise) return override
      return Promise.resolve(override)
    }
    switch (command) {
      case "get_insights_report":
        return Promise.resolve(report())
      case "get_insights_status":
        return Promise.resolve(STATUS)
      default:
        return Promise.resolve(null)
    }
  })
}

beforeEach(() => {
  vi.clearAllMocks()
  mockCommands()
})

describe("InsightsPane loading", () => {
  it("renders a loading state while the report is in flight, never a clean one", () => {
    mockCommands({ get_insights_report: new Promise(() => {}) })
    render(<InsightsPane />)

    expect(screen.getByText("Computing the report…")).toBeInTheDocument()
    expect(screen.queryByText(/clean across/i)).not.toBeInTheDocument()
    expect(screen.queryByText(/with findings/i)).not.toBeInTheDocument()
  })

  it("renders the failure as an error with a retry, never as an empty report", async () => {
    mockCommands({ get_insights_report: new Error("synthetic failure") })
    render(<InsightsPane />)

    expect(await screen.findByText("Couldn't check this device")).toBeInTheDocument()
    expect(
      screen.getByText(/could not compute the report just now\. Nothing was assessed/),
    ).toBeInTheDocument()
    expect(screen.queryByText(/clean across/i)).not.toBeInTheDocument()

    mockCommands()
    fireEvent.click(screen.getByRole("button", { name: "Try again" }))
    expect(await screen.findByText("What we checked")).toBeInTheDocument()
  })
})

describe("InsightsPane first open", () => {
  it("names the nothing-processed-yet case explicitly and shows pending progress", async () => {
    mockCommands({
      get_insights_report: report({
        coverage: coverage({ discovered: 6, pending: 5, processing: 1 }),
      }),
      get_insights_status: { calculating: false, pending: 5, processing: 1 },
    })
    render(<InsightsPane />)

    expect(await screen.findByText("Nothing has been processed yet")).toBeInTheDocument()
    // Coverage and pending progress are shown prominently.
    expect(screen.getByText("6 discovered · 0 assessed")).toBeInTheDocument()
    expect(screen.getByText("5 waiting and 1 processing now")).toBeInTheDocument()
    // An incomplete report never renders as clean.
    expect(screen.queryByText(/clean across/i)).not.toBeInTheDocument()
  })

  it("says plainly when there are no sessions at all in the window", async () => {
    render(<InsightsPane />)

    expect(
      await screen.findByText(
        "No sessions were found in the last 30 days, so there is nothing to assess.",
      ),
    ).toBeInTheDocument()
    expect(screen.queryByText("Nothing has been processed yet")).not.toBeInTheDocument()
  })
})

describe("InsightsPane coverage (FR-12)", () => {
  it("presents the denominator separately, and every non-cohort row as not assessed", async () => {
    mockCommands({
      get_insights_report: report({
        coverage: coverage({
          discovered: 10,
          ready: 3,
          pending: 2,
          processing: 1,
          failed: 1,
          unsupported: 1,
          stale: 1,
          unknownStart: 1,
        }),
        assessedSessions: 3,
      }),
    })
    render(<InsightsPane />)

    // Denominator and cohort are two figures, side by side, never merged.
    expect(await screen.findByText("10 discovered · 3 assessed")).toBeInTheDocument()
    expect(
      screen.getByText(/10 sessions discovered in the last 30 days\. 3 in the assessed cohort/),
    ).toBeInTheDocument()

    // Every denominator row outside the cohort names itself as not assessed.
    expect(screen.getByText("2 waiting to be processed — not assessed yet")).toBeInTheDocument()
    expect(screen.getByText("1 being processed now — not assessed yet")).toBeInTheDocument()
    expect(screen.getByText("1 could not be processed — not assessed")).toBeInTheDocument()
    expect(
      screen.getByText("1 do not carry readable evidence — not assessed"),
    ).toBeInTheDocument()
    expect(
      screen.getByText("1 have out-of-date evidence — not assessed until reprocessed"),
    ).toBeInTheDocument()
    expect(
      screen.getByText("1 have no trustworthy start time — not assessed"),
    ).toBeInTheDocument()
  })
})

describe("InsightsPane categories", () => {
  it("renders findings, clean, and not-assessed states with their reasons", async () => {
    const categories = notAssessedCategories()
    categories[0] = {
      ...categories[0]!,
      status: "findings",
      findingSessions: 2,
      notAssessedReason: null,
      eligible: 4,
      assessed: 4,
    }
    categories[1] = {
      ...categories[1]!,
      status: "clean",
      notAssessedReason: null,
      eligible: 4,
      assessed: 4,
    }
    categories[2] = { ...categories[2]!, notAssessedReason: "capabilityMissing" }
    categories[3] = { ...categories[3]!, notAssessedReason: "incompleteEvidence" }
    categories[4] = { ...categories[4]!, notAssessedReason: "evidenceContractIncomplete" }
    mockCommands({
      get_insights_report: report({
        coverage: coverage({ discovered: 4, ready: 4 }),
        assessedSessions: 4,
        categories,
      }),
    })
    render(<InsightsPane />)

    // All nine categories are present by name.
    expect(await screen.findByText("Sessions over depth")).toBeInTheDocument()
    for (const label of [
      "Model overthinking",
      "Overpowered subagents",
      "Unused MCP servers",
      "Unused built-in tools",
      "Unused skills",
      "Old model usage",
      "Overuse of fast mode",
      "Cache churn",
    ]) {
      expect(screen.getByText(label)).toBeInTheDocument()
    }

    expect(screen.getByText("2 sessions with findings")).toBeInTheDocument()
    expect(screen.getByText("Clean across 4 assessed sessions")).toBeInTheDocument()
    expect(
      screen.getByText(
        "Not assessed — these sessions do not record the evidence this check needs",
      ),
    ).toBeInTheDocument()
    expect(
      screen.getByText(
        "Not assessed — evidence is incomplete, so a clean result cannot be claimed",
      ),
    ).toBeInTheDocument()
    expect(
      screen.getByText("Not assessed — stored evidence cannot express this check yet"),
    ).toBeInTheDocument()
    // The remaining categories carry the no-sessions reason.
    expect(
      screen.getAllByText("Not assessed — no processed sessions in this window").length,
    ).toBe(4)
  })
})

describe("InsightsPane quota pressure", () => {
  it("renders the not-assessed case as not assessed, without a clean look", async () => {
    render(<InsightsPane />)

    expect(
      await screen.findByText(
        "Not assessed — the sessions in this window carry no quota evidence.",
      ),
    ).toBeInTheDocument()
  })

  it("renders quota findings with limit kinds, severities, and models", async () => {
    mockCommands({
      get_insights_report: report({
        coverage: coverage({ discovered: 2, ready: 2 }),
        assessedSessions: 2,
        quotaPressure: {
          assessed: true,
          findings: {
            totalHits: 3,
            hardHits: 1,
            warnings: 2,
            affectedSessionCount: 2,
            hitsByLimitKind: [
              { kind: "weekly", hits: 2 },
              { kind: "rateLimit", hits: 1 },
            ],
            affectedModels: ["claude-3-5-haiku-20241022"],
            affectedModelsTruncated: false,
            firstObservedTsMs: 1_000,
            lastObservedTsMs: 2_000,
          },
        },
      }),
    })
    render(<InsightsPane />)

    expect(await screen.findByText("3 limit hits across 2 sessions")).toBeInTheDocument()
    expect(screen.getByText("1 hard hit · 2 warnings")).toBeInTheDocument()
    expect(screen.getByText("Weekly: 2 hits")).toBeInTheDocument()
    expect(screen.getByText("Rate limit: 1 hit")).toBeInTheDocument()
    expect(screen.getByText("Models: claude-3-5-haiku-20241022")).toBeInTheDocument()
  })
})

describe("InsightsPane freshness", () => {
  it("states that the report is computed locally and scoped to this machine", async () => {
    render(<InsightsPane />)

    expect(
      await screen.findByText(/Computed on this device from local session transcripts/),
    ).toBeInTheDocument()
    expect(screen.getByText(/Nothing leaves this device/)).toBeInTheDocument()
    expect(screen.getByText(/native environment/)).toBeInTheDocument()
  })

  it("stamps the report with when it was computed and the window it covers", async () => {
    render(<InsightsPane />)

    expect(
      await screen.findByText(/Updated at .+ · 30 days to .+ · Computed on/),
    ).toBeInTheDocument()
    // The intro and the machine-scope label are always visible.
    expect(screen.getByText(/Nothing is uploaded/)).toBeInTheDocument()
    expect(screen.getByText("This machine, live")).toBeInTheDocument()
  })
})
