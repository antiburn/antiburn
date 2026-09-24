import { fireEvent, render, type RenderResult } from "@testing-library/react"
import { vi } from "vitest"

import type {
  AggregateWinPayload,
  AggregateWinsPayload,
  BurnCheckDetectorId,
  BurnCheckTargetPayload,
  ChecksReportPayload,
} from "../../../../lib/insightsIpc"
import { BurnChecksSession, type BurnChecksAdapter } from "../../BurnChecksSession"
import { BurnChecksView } from "../../BurnChecksView"
import { BurnChecksSavings } from "../BurnChecksSavings"

// Shared fixtures and helpers for the Burn Checks view test suite. The suite
// is split across files in this folder by theme; each file imports what it
// needs from here.

// Each test file declares its own `commands` object via `vi.hoisted`, so its
// `vi.mock` factories can reference it. This type states the shared shape.
interface BurnChecksCommandMocks {
  prepare: ReturnType<typeof vi.fn>
  apply: ReturnType<typeof vi.fn>
  copy: ReturnType<typeof vi.fn>
  copyFallback: ReturnType<typeof vi.fn>
  copyBatch: ReturnType<typeof vi.fn>
  writeClipboardText: ReturnType<typeof vi.fn>
  openSample: ReturnType<typeof vi.fn>
  noteInteraction: ReturnType<typeof vi.fn>
}

const innerWidth = Object.getOwnPropertyDescriptor(window, "innerWidth")

export function setWindowWidth(value: number): void {
  Object.defineProperty(window, "innerWidth", { configurable: true, value })
  fireEvent(window, new Event("resize"))
}

export const report: ChecksReportPayload = {
  evidenceSettled: false,
  pendingEvidence: 0,
  estimatedTokenBurnBasisPoints: 800,
  estimatedTokenBurnBasisPointsByDetectorMask: Array<number | null>(512).fill(null),
  categories: [
    {
      id: "oldModelUsage",
      finding: 1,
      clean: 2,
      unavailable: 0,
      estimatedTokenBurnBasisPoints: 800,
      lifecycle: "failing",
    },
    {
      id: "unusedSkills",
      finding: 0,
      clean: 3,
      unavailable: 0,
      estimatedTokenBurnBasisPoints: 0,
      lifecycle: "passing",
    },
    {
      id: "cacheChurn",
      finding: 0,
      clean: 0,
      unavailable: 3,
      estimatedTokenBurnBasisPoints: null,
      lifecycle: null,
    },
  ],
}

export const namedTargetReport: ChecksReportPayload = {
  ...report,
  categories: [
    { ...report.categories[0]!, id: "unusedMcpServers" },
    ...report.categories.slice(1),
  ],
}

export const passedTargetReport: ChecksReportPayload = {
  ...report,
  categories: [
    { ...report.categories[0]!, lifecycle: "passing" },
    ...report.categories.slice(1),
  ],
}

export const target: BurnCheckTargetPayload = {
  findingId: "finding-stable",
  actionId: "action-fresh",
  finding: {
    detector: "oldModelUsage",
    agent: "claude-code",
    sourceFormat: "claudeJsonl",
    observation: "The old model handled 8 eligible requests.",
    labels: [],
    omitted: 0,
  },
  display: {
    resourceKind: "model",
    resourceIdentity: "claude-opus-4-6",
    currentValue: "claude-opus-4-6",
    replacementValue: "claude-sonnet-5",
    scopeKind: "global",
    quantity: 8,
    quantityUnit: "turns",
    observationCount: 8,
    firstObservedAtMs: 1,
    lastObservedAtMs: 2,
    estimateMethod: "oldModelPriceDifference",
    estimatedOpportunity: null,
    estimatedTokenBurnBasisPoints: null,
    verificationLimit: "freshEvidenceFromSameSourceAndTarget",
  },
  occurrenceCount: 1,
  affectedSessionCount: 1,
  autoFix: { status: "available" },
  promptFix: { status: "available" },
  watch: null,
  evidenceAvailable: false,
  coverageLimits: ["currentPublishedEvidenceOnly"],
  samples: [
    {
      navigationHandle: "opaque-handle",
      title: "Update model",
      agent: "claude-code",
      surface: "cli",
      observedAtMs: 1,
      repo: "demo",
      timestamp: "2026-09-14T12:00:00Z",
      isActive: false,
      hasForkParent: false,
      forkChildCount: 0,
      cost: null,
      models: [],
      modelRuns: [],
      hygiene: { evidenceState: "pending", unusedResources: null, badges: [] },
    },
  ],
  expiresAtEpoch: 100,
}

export const aggregate: AggregateWinsPayload = {
  wins: [
    {
      findingId: "win-1",
      remediationCycleId: "cycle-1",
      detector: "oldModelUsage",
      origin: "action",
      display: target.display,
      savings: {
        status: {
          status: "known",
          method: "oldModelPriceDifference",
          methodRevision: 1,
          pricingRevision: "pricing-1",
          apiEquivalentCostAvoidedUsd: 1.25,
          measuredThroughMs: 2,
          recurrenceMs: null,
        },
        tokenSavings: null,
        apiEquivalentCostAvoidedUsd: 1.25,
        improvementCount: 2,
        method: "oldModelPriceDifference",
      },
      verifiedBoundaryMs: 2,
      startsAtMs: 1,
      endsAtMs: 2,
    },
  ],
}

export function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason: unknown) => void
  const promise = new Promise<T>((complete, fail) => {
    resolve = complete
    reject = fail
  })
  return { promise, resolve, reject }
}

export function recurredTarget(actionId: string): BurnCheckTargetPayload {
  return {
    ...target,
    actionId,
    watch: {
      watchId: "watch-1",
      origin: "action",
      lifecycle: "recurred",
      verification: { status: "recurred", methodRevision: 1, evidenceRevision: "e2" },
      savings: { status: "pending" },
    },
  }
}

export function setup(
  targetPayload:
    | BurnCheckTargetPayload
    | BurnCheckTargetPayload[]
    | null
    | Promise<BurnCheckTargetPayload | BurnCheckTargetPayload[] | null> = target,
  truncated = false,
  aggregatePayload: AggregateWinsPayload | Promise<AggregateWinsPayload> = aggregate,
  reportPayload: ChecksReportPayload | Promise<ChecksReportPayload> = namedTargetReport,
  checkSamples?: BurnCheckTargetPayload["samples"],
): {
  adapter: BurnChecksAdapter
  session: BurnChecksSession
  view: RenderResult
  hide: () => void
} {
  let visible: ((value: boolean) => void) | null = null
  const adapter: BurnChecksAdapter = {
    getReport: vi.fn(() => Promise.resolve(reportPayload)),
    getAggregateWins: vi.fn(() => Promise.resolve(aggregatePayload)),
    getTargets: vi.fn(async () => {
      const resolvedTarget = await targetPayload
      return {
        targets: Array.isArray(resolvedTarget)
          ? resolvedTarget
          : resolvedTarget
            ? [resolvedTarget]
            : [],
        samples:
          checkSamples ??
          (Array.isArray(resolvedTarget)
            ? resolvedTarget.flatMap((target) => target.samples).slice(0, 3)
            : (resolvedTarget?.samples ?? [])),
        truncated,
      }
    }),
    cancelReport: vi.fn().mockResolvedValue(undefined),
    getVisible: vi.fn().mockResolvedValue(true),
    onVisible: vi.fn(async (handler) => {
      visible = handler
      return () => undefined
    }),
    onChanged: vi.fn(async () => () => undefined),
  }
  const session = new BurnChecksSession(adapter)
  const view = render(<BurnChecksView active session={session} />)
  return { adapter, session, view, hide: () => visible?.(false) }
}

// Renders the Savings section on its own, with the fixture target's check as
// the sole passed detector unless a test needs a different set.
export function renderSavings(
  wins: readonly AggregateWinPayload[],
  passedDetectors: ReadonlySet<BurnCheckDetectorId> = new Set(["oldModelUsage"]),
): RenderResult {
  return render(<BurnChecksSavings wins={wins} passedDetectors={passedDetectors} />)
}

// Call from each test file's own beforeEach, passing that file's own
// `commands` object. Resets the command mocks to their default resolved
// values.
export function installBurnChecksCommandMocks(commands: BurnChecksCommandMocks): void {
  vi.clearAllMocks()
  commands.prepare.mockResolvedValue({
    outcome: "reviewReady",
    review: {
      preparedOperationId: "prepared-1",
      expiresAtEpoch: 100,
      agent: "claude-code",
      scope: "global",
      setting: "model",
      configFile: "~/.claude/settings.json",
      selectorLabel: "model",
      currentValue: "claude-opus-4-6",
      proposedValue: "claude-sonnet-5",
      effect: "modelSelection",
      sideEffect: "modelBehaviorMayChange",
    },
  })
  commands.apply.mockResolvedValue({
    outcome: "appliedAwaitingVerification",
    watchId: "watch-1",
  })
  commands.copy.mockResolvedValue({
    outcome: "promptReady",
    prompt: "Backend prompt",
    watch: {
      watchId: "watch-1",
      origin: "action",
      lifecycle: "watching",
      verification: { status: "watching" },
      savings: { status: "pending" },
    },
  })
  commands.copyFallback.mockResolvedValue({
    outcome: "promptReady",
    prompt: "Inspect representative evidence.",
  })
  commands.copyBatch.mockResolvedValue({
    outcome: "promptReady",
    prompt: "Batch backend prompt",
  })
  commands.openSample.mockResolvedValue({ outcome: "opened" })
  commands.writeClipboardText.mockResolvedValue(undefined)
}

// Call from each test file's own afterEach. Restores real timers and the
// window width descriptor.
export function restoreBurnChecksTestWindow(): void {
  vi.useRealTimers()
  if (innerWidth) Object.defineProperty(window, "innerWidth", innerWidth)
}
