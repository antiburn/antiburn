import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type {
  AggregateWinsPayload,
  ApplyPreparedBurnCheckOperationOutcome,
  BurnCheckTargetPayload,
  ChecksReportPayload,
  CopyPromptFixBurnCheckTargetOutcome,
  PrepareAutoFixBurnCheckTargetOutcome,
} from "../../lib/insightsIpc"
import type * as InsightsIpcModule from "../../lib/insightsIpc"
import type * as IpcModule from "../../lib/ipc"
import { BurnChecksSession, type BurnChecksAdapter } from "./BurnChecksSession"
import { BurnChecksView } from "./BurnChecksView"

const commands = vi.hoisted(() => ({
  prepare: vi.fn(),
  apply: vi.fn(),
  copy: vi.fn(),
  copyFallback: vi.fn(),
  copyBatch: vi.fn(),
  openSample: vi.fn(),
  noteInteraction: vi.fn(),
}))

vi.mock("../../lib/insightsIpc", async (importOriginal) => ({
  ...(await importOriginal<typeof InsightsIpcModule>()),
  prepareAutoFixBurnCheckTarget: commands.prepare,
  applyPreparedBurnCheckOperation: commands.apply,
  copyPromptFixBurnCheckTarget: commands.copy,
  copyPromptFixBurnCheck: commands.copyFallback,
  copyPromptFixBurnCheckTargets: commands.copyBatch,
  openBurnCheckSample: commands.openSample,
}))

vi.mock("../../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof IpcModule>()),
  noteInteraction: commands.noteInteraction,
}))

const report: ChecksReportPayload = {
  evidenceSettled: false,
  pendingEvidence: 0,
  estimatedTokenBurnBasisPoints: 800,
  categories: [
    {
      id: "oldModelUsage",
      finding: 1,
      clean: 2,
      unavailable: 0,
      estimatedTokenBurnBasisPoints: 800,
    },
    {
      id: "unusedSkills",
      finding: 0,
      clean: 3,
      unavailable: 0,
      estimatedTokenBurnBasisPoints: 0,
    },
    {
      id: "cacheChurn",
      finding: 0,
      clean: 0,
      unavailable: 3,
      estimatedTokenBurnBasisPoints: null,
    },
  ],
}

const namedTargetReport: ChecksReportPayload = {
  ...report,
  categories: [
    { ...report.categories[0]!, id: "unusedMcpServers" },
    ...report.categories.slice(1),
  ],
}

const target: BurnCheckTargetPayload = {
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
    verificationLimit: "freshEvidenceFromSameSourceAndTarget",
  },
  occurrenceCount: 1,
  autoFix: { status: "available" },
  promptFix: { status: "available" },
  watch: null,
  coverageLimits: ["currentPublishedEvidenceOnly"],
  samples: [
    {
      navigationHandle: "opaque-handle",
      title: "Update model",
      agent: "claude-code",
      surface: "cli",
      observedAtMs: 1,
    },
  ],
  expiresAtEpoch: 100,
}

const aggregate: AggregateWinsPayload = {
  wins: [
    {
      findingId: "win-1",
      detector: "oldModelUsage",
      origin: "action",
      display: target.display,
      savings: {
        tokenSavings: null,
        apiEquivalentCostAvoidedUsd: 1.25,
        improvementCount: 2,
        method: "oldModelPriceDifference",
      },
      startsAtMs: 1,
      endsAtMs: 2,
    },
  ],
}

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((complete) => {
    resolve = complete
  })
  return { promise, resolve }
}

function recurredTarget(actionId: string): BurnCheckTargetPayload {
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

function setup(
  targetPayload:
    | BurnCheckTargetPayload
    | BurnCheckTargetPayload[]
    | null
    | Promise<BurnCheckTargetPayload | BurnCheckTargetPayload[] | null> = target,
  truncated = false,
  aggregatePayload: AggregateWinsPayload = aggregate,
  reportPayload: ChecksReportPayload | Promise<ChecksReportPayload> = namedTargetReport,
) {
  let visible: ((value: boolean) => void) | null = null
  const adapter: BurnChecksAdapter = {
    getReport: vi.fn(() => Promise.resolve(reportPayload)),
    getAggregateWins: vi.fn().mockResolvedValue(aggregatePayload),
    getTargets: vi.fn(async () => {
      const resolvedTarget = await targetPayload
      return {
        targets: Array.isArray(resolvedTarget)
          ? resolvedTarget
          : resolvedTarget
            ? [resolvedTarget]
            : [],
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

beforeEach(() => {
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
      currentValue: "claude-opus-4-6",
      proposedValue: "claude-sonnet-5",
      effect: "futureModelSelection",
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
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText: vi.fn().mockResolvedValue(undefined) },
  })
})

afterEach(() => {
  vi.useRealTimers()
})

describe("BurnChecksView", () => {
  it("uses one check detail and one batch prompt for a non-named check", async () => {
    setup(target, false, aggregate, report)

    expect(
      await screen.findByText(
        "Some sessions used an older model when a newer one was available.",
      ),
    ).toBeVisible()
    expect(screen.queryByRole("heading", { name: "claude-opus-4-6" })).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Copy fix prompt" }))

    await waitFor(() =>
      expect(navigator.clipboard.writeText).toHaveBeenCalledWith("Batch backend prompt"),
    )
    expect(commands.copyBatch).toHaveBeenCalledWith(["action-fresh"])
    expect(commands.copy).not.toHaveBeenCalled()
  })

  it("includes all listed targets in a large check prompt", async () => {
    const targets = Array.from({ length: 13 }, (_, index) => ({
      ...target,
      findingId: `finding-${index}`,
      actionId: `action-${index}`,
    }))
    setup(targets, false, aggregate, report)

    fireEvent.click(await screen.findByRole("button", { name: "Copy fix prompt" }))

    await waitFor(() =>
      expect(commands.copyBatch).toHaveBeenCalledWith(
        Array.from({ length: 13 }, (_, index) => `action-${index}`),
      ),
    )
  })

  it.each([0, 1, 50, 100, 200, 10_000, null])(
    "keeps exact labels and a visible positive arc for %s basis points",
    async (basisPoints) => {
      setup(target, false, aggregate, { ...report, estimatedTokenBurnBasisPoints: basisPoints })
      const dial = await screen.findByRole("img", {
        name:
          basisPoints == null
            ? "Burn estimate unavailable"
            : `Estimated burn: ${basisPoints / 100}%`,
      })
      const burn = dial.querySelector('[data-segment-id="burn"]')
      const remainder = dial.querySelector('[data-segment-id="remainder"]')
      if (remainder) expect(remainder).toHaveClass("text-measure")
      if (burn) expect(burn).toHaveClass("text-brand-tint")
      if (basisPoints === null) {
        expect(dial.querySelector('[data-segment-id="unknown"]')).toBeTruthy()
        expect(burn).toBeNull()
      } else if (basisPoints === 0) {
        expect(burn).toBeNull()
        expect(dial.querySelector('[data-segment-id="remainder"]')).toHaveAttribute(
          "data-arc-angle",
          "360",
        )
      } else {
        const actualAngle = (360 * basisPoints) / 10_000
        const minimumAngle = (4 / (Math.PI * 80)) * 360
        expect(Number(burn?.getAttribute("data-arc-angle"))).toBeCloseTo(
          Math.max(actualAngle, minimumAngle),
          5,
        )
        if (basisPoints < 10_000) expect(burn).toHaveAttribute("stroke-linecap", "butt")
      }
    },
  )

  it("renders assessed checks and concise failed details", async () => {
    setup(target, false, aggregate, report)
    expect(
      await screen.findByRole("button", { name: /Old model usage.*8% burn/ }),
    ).toBeVisible()
    const dial = screen.getByRole("img", { name: "Estimated burn: 8%" })
    const arcs = Array.from(dial.querySelectorAll("circle"))
    expect(arcs.map((arc) => arc.dataset.segmentId)).toEqual(["burn", "remainder"])
    expect(Number(arcs[0]!.dataset.arcAngle)).toBeCloseTo(28.8)
    expect(Number(arcs[1]!.dataset.arcAngle)).toBeCloseTo(331.2)
    expect(screen.getByText(/1 check failed/)).toBeVisible()
    expect(screen.queryByText(/More evidence is needed/)).not.toBeInTheDocument()
    expect(screen.getByRole("heading", { name: "Failed checks" })).toBeVisible()
    expect(screen.getByRole("heading", { name: "Passed checks" })).toBeVisible()
    expect(screen.queryByRole("heading", { name: "Not assessed" })).not.toBeInTheDocument()
    expect(screen.queryByText("Excess cache rehydration")).not.toBeInTheDocument()
    expect(screen.queryByText(/check results are assessed/)).not.toBeInTheDocument()
    expect(screen.queryByText(/Evidence work is still in progress/)).not.toBeInTheDocument()
    expect(
      within(screen.getByRole("region", { name: "Your savings" })).getByText(
        "2 improvements across 1 check",
      ),
    ).toBeVisible()
    expect(
      await screen.findByText(
        "Some sessions used an older model when a newer one was available.",
      ),
    ).toBeVisible()
    expect(screen.queryByRole("heading", { name: "claude-opus-4-6" })).not.toBeInTheDocument()
    expect(
      screen.queryByText("The old model handled 8 eligible requests."),
    ).not.toBeInTheDocument()
    expect(screen.queryByText("Exact facts")).not.toBeInTheDocument()
    expect(screen.queryByText("What we found")).not.toBeInTheDocument()
    expect(screen.queryByText("Why it matters")).not.toBeInTheDocument()
    expect(screen.queryByText("Suggested change")).not.toBeInTheDocument()
    expect(screen.queryByText(/Verification requires fresh evidence/)).not.toBeInTheDocument()
  })

  it("renders every shared per-check metric in the main Burn Checks view", async () => {
    const allFailures: ChecksReportPayload = {
      ...report,
      estimatedTokenBurnBasisPoints: 880,
      categories: [
        {
          id: "sessionsOverDepth",
          finding: 1,
          clean: 0,
          unavailable: 0,
          estimatedTokenBurnBasisPoints: 800,
        },
        {
          id: "modelOverthinking",
          finding: 1,
          clean: 0,
          unavailable: 0,
          estimatedTokenBurnBasisPoints: 350,
        },
        {
          id: "overpoweredSubagents",
          finding: 1,
          clean: 0,
          unavailable: 0,
          estimatedTokenBurnBasisPoints: 880,
        },
        {
          id: "unusedMcpServers",
          finding: 1,
          clean: 0,
          unavailable: 0,
          estimatedTokenBurnBasisPoints: 100,
        },
        {
          id: "unusedBuiltInTools",
          finding: 1,
          clean: 0,
          unavailable: 0,
          estimatedTokenBurnBasisPoints: 1,
        },
        {
          id: "unusedSkills",
          finding: 1,
          clean: 0,
          unavailable: 0,
          estimatedTokenBurnBasisPoints: 100,
        },
        {
          id: "oldModelUsage",
          finding: 1,
          clean: 0,
          unavailable: 0,
          estimatedTokenBurnBasisPoints: 400,
        },
        {
          id: "overuseOfFastMode",
          finding: 1,
          clean: 0,
          unavailable: 0,
          estimatedTokenBurnBasisPoints: 333,
        },
        {
          id: "cacheChurn",
          finding: 1,
          clean: 0,
          unavailable: 0,
          estimatedTokenBurnBasisPoints: 700,
        },
      ],
    }
    setup(target, false, aggregate, allFailures)

    for (const metric of [
      "8% burn",
      "3% burn",
      "8% burn",
      "1% burn",
      "Under 1% burn",
      "1% burn",
      "4% burn",
      "3% burn",
      "7% burn",
    ]) {
      expect(
        (await screen.findAllByRole("button", { name: new RegExp(metric) })).length,
      ).toBeGreaterThan(0)
    }
  })

  it("shows the accessible agent icon without visible agent or scope metadata", async () => {
    setup()

    expect(await screen.findByRole("img", { name: "Claude Code" })).toBeVisible()
    expect(screen.queryByText("Claude Code")).not.toBeInTheDocument()
    expect(
      screen.queryByText(/^(Session|Worker|Project|Global) scope$/),
    ).not.toBeInTheDocument()
  })

  it("uses one busy region and one announcement for the shaped loading skeleton", () => {
    const pending = deferred<ChecksReportPayload>()
    const { view } = setup(target, false, aggregate, pending.promise)

    const loading = screen.getByRole("region", { name: "Loading Burn checks" })
    expect(loading).toHaveAttribute("aria-busy", "true")
    expect(within(loading).getAllByRole("status")).toHaveLength(1)
    expect(view.container.querySelectorAll('[aria-busy="true"]')).toHaveLength(1)
    expect(loading.querySelectorAll('[data-skeleton="hero"]')).toHaveLength(1)
    expect(loading.querySelectorAll('[data-skeleton="group-label"]')).toHaveLength(1)
    const rows = loading.querySelectorAll('[data-skeleton="check-row"]')
    expect(rows).toHaveLength(3)
    for (const row of rows) {
      expect(
        Array.from(row.querySelectorAll("[data-skeleton-slot]"), (slot) =>
          slot.getAttribute("data-skeleton-slot"),
        ),
      ).toEqual(["icon", "title", "summary", "metric", "disclosure"])
    }
    expect(loading.querySelectorAll('[data-skeleton-slot="metric"]')).toHaveLength(3)
    expect(
      loading.querySelectorAll('[data-placeholder]:not([aria-hidden="true"])'),
    ).toHaveLength(0)
    expect(loading).toHaveTextContent("Loading Burn checks.")
  })

  it("uses one compact skeleton for an expanded check while its details load", async () => {
    const pending = deferred<BurnCheckTargetPayload | null>()
    setup(pending.promise)

    const loading = await screen.findByRole("region", { name: "Loading finding details" })
    expect(loading).toHaveAttribute("aria-busy", "true")
    expect(within(loading).getAllByRole("status")).toHaveLength(1)
    expect(loading.querySelectorAll("[data-placeholder]")).toHaveLength(4)
    expect(loading).not.toHaveTextContent("Loading finding details…")

    await act(async () => pending.resolve(target))
    expect(await screen.findByRole("button", { name: "Copy fix prompt" })).toBeVisible()
  })

  it("shows a target load error and retries the expanded check", async () => {
    const unavailable = Promise.reject(new Error("Unavailable"))
    const { adapter } = setup(unavailable)

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Could not load this check's details.",
    )
    vi.mocked(adapter.getTargets).mockResolvedValueOnce({ targets: [target], truncated: false })
    fireEvent.click(screen.getByRole("button", { name: "Retry" }))

    expect(await screen.findByRole("button", { name: "Copy fix prompt" })).toBeVisible()
  })

  it("uses backend prepare, apply, and prompt commands without duplicate actions", async () => {
    const { adapter, session } = setup()
    const fix = await screen.findByRole("button", { name: "Fix" })
    fireEvent.click(fix)
    fireEvent.click(fix)
    expect(commands.prepare).toHaveBeenCalledOnce()
    const dialog = await screen.findByRole("dialog", { name: "Fix claude-opus-4-6" })
    expect(dialog).toHaveTextContent("AgentClaude Code")
    expect(dialog).toHaveTextContent("SettingModel")
    expect(dialog).toHaveTextContent("Config file~/.claude/settings.json")
    expect(dialog).toHaveTextContent("Config changeclaude-opus-4-6 → claude-sonnet-5")
    expect(dialog).toHaveTextContent("This plan changes future model selection")
    fireEvent.click(within(dialog).getByRole("button", { name: "Apply change" }))
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Change applied" })).toBeDisabled(),
    )
    expect(commands.apply).toHaveBeenCalledWith("prepared-1")
    await waitFor(() => expect(fix).toHaveFocus())

    fireEvent.click(screen.getByRole("button", { name: "Copy fix prompt" }))
    await waitFor(() =>
      expect(navigator.clipboard.writeText).toHaveBeenCalledWith("Backend prompt"),
    )
    expect(commands.copy).toHaveBeenCalledWith("action-fresh")
    const copied = screen.getByRole("button", { name: "Copied" })
    const applied = screen.getByRole("button", { name: "Change applied" })
    expect(copied).toBeDisabled()
    expect(copied).not.toHaveClass("text-token-in")
    expect(applied).not.toHaveClass("text-token-in")
    expect(copied.querySelector(".lucide-check")).toHaveClass("text-token-in")
    expect(applied.querySelector(".lucide-check")).toHaveClass("text-token-in")
    expect(commands.noteInteraction.mock.calls).toEqual(
      expect.arrayContaining([
        [{ kind: "burnCheckAutoFixReviewed", outcome: "ready" }],
        [{ kind: "burnCheckAutoFixConfirmed" }],
        [
          {
            kind: "burnCheckAutoFixCompleted",
            outcome: "applied_awaiting_verification",
          },
        ],
        [{ kind: "burnCheckPromptPrepared", outcome: "ready" }],
        [{ kind: "burnCheckPromptCopied" }],
      ]),
    )

    vi.mocked(adapter.getTargets).mockResolvedValueOnce({
      targets: [{ ...target, actionId: "action-new" }],
      truncated: false,
    })
    const refreshCount = vi.mocked(adapter.getTargets).mock.calls.length
    session.loadTargets("unusedMcpServers", true)
    await waitFor(() =>
      expect(vi.mocked(adapter.getTargets).mock.calls.length).toBeGreaterThan(refreshCount),
    )
    await waitFor(() => expect(screen.getByRole("button", { name: "Copied" })).toBeDisabled())
    expect(screen.getByRole("button", { name: "Change applied" })).toBeDisabled()

    vi.mocked(adapter.getTargets).mockResolvedValueOnce({
      targets: [
        {
          ...target,
          actionId: "action-next-attempt",
          watch: {
            watchId: "watch-2",
            origin: "action",
            lifecycle: "watching",
            verification: { status: "watching" },
            savings: { status: "pending" },
          },
        },
      ],
      truncated: false,
    })
    session.loadTargets("unusedMcpServers", true)
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Copy fix prompt" })).toBeEnabled(),
    )
    expect(screen.getByRole("button", { name: "Fix" })).toBeEnabled()
  })

  it("returns to the chooser after the selected automatic fix changes", async () => {
    const targets = Array.from({ length: 3 }, (_, index) => ({
      ...target,
      findingId: `finding-${index}`,
      actionId: `action-${index}`,
      display: {
        ...target.display,
        resourceIdentity: `model-${index}`,
        currentValue: `model-${index}`,
      },
    }))
    const { adapter } = setup(targets, false, aggregate, report)

    fireEvent.click(await screen.findByRole("button", { name: "Fix" }))
    fireEvent.click(screen.getByRole("button", { name: "model-0" }))
    fireEvent.click(screen.getByRole("button", { name: "Fix" }))
    const dialog = await screen.findByRole("dialog", { name: "Fix model-0" })
    vi.mocked(adapter.getTargets).mockResolvedValueOnce({
      targets: targets.slice(1),
      truncated: false,
    })
    fireEvent.click(within(dialog).getByRole("button", { name: "Apply change" }))

    await screen.findByRole("button", { name: "Fix" })
    fireEvent.click(screen.getByRole("button", { name: "Fix" }))
    expect(screen.getByRole("button", { name: "model-1" })).toBeVisible()
    expect(screen.getByRole("button", { name: "model-2" })).toBeVisible()
  })

  it("lets the user choose a different automatic fix before applying", async () => {
    const targets = Array.from({ length: 2 }, (_, index) => ({
      ...target,
      findingId: `finding-${index}`,
      actionId: `action-${index}`,
      display: {
        ...target.display,
        resourceIdentity: `model-${index}`,
        currentValue: `model-${index}`,
      },
    }))
    setup(targets, false, aggregate, report)

    fireEvent.click(await screen.findByRole("button", { name: "Fix" }))
    fireEvent.click(screen.getByRole("button", { name: "model-0" }))
    fireEvent.click(screen.getByRole("button", { name: "Choose another change" }))
    fireEvent.click(screen.getByRole("button", { name: "model-1" }))
    fireEvent.click(screen.getByRole("button", { name: "Fix" }))

    await waitFor(() => expect(commands.prepare).toHaveBeenCalledWith("action-1"))
  })

  it("restores named target actions after their brief success state", async () => {
    setup()

    fireEvent.click(await screen.findByRole("button", { name: "Copy fix prompt" }))
    await screen.findByRole("button", { name: "Copied" })
    fireEvent.click(screen.getByRole("button", { name: "Fix" }))
    const dialog = await screen.findByRole("dialog", { name: "Fix claude-opus-4-6" })
    fireEvent.click(within(dialog).getByRole("button", { name: "Apply change" }))
    await screen.findByRole("button", { name: "Change applied" })

    await waitFor(
      () => {
        expect(screen.getByRole("button", { name: "Copy fix prompt" })).toBeEnabled()
        expect(screen.getByRole("button", { name: "Fix" })).toBeEnabled()
      },
      { timeout: 4_000 },
    )
    expect(commands.copy).toHaveBeenCalledOnce()
    expect(commands.apply).toHaveBeenCalledOnce()
  })

  it("restores a check-level prompt action after its brief success state", async () => {
    setup(target, false, aggregate, report)

    fireEvent.click(await screen.findByRole("button", { name: "Copy fix prompt" }))
    await screen.findByRole("button", { name: "Copied" })
    await waitFor(
      () => expect(screen.getByRole("button", { name: "Copy fix prompt" })).toBeEnabled(),
      { timeout: 4_000 },
    )
    expect(commands.copyBatch).toHaveBeenCalledOnce()
  })

  it("keeps an open review when the expiring action handle rotates", async () => {
    const { adapter, session } = setup()
    fireEvent.click(await screen.findByRole("button", { name: "Fix" }))
    const dialog = await screen.findByRole("dialog", { name: "Fix claude-opus-4-6" })
    vi.mocked(adapter.getTargets).mockResolvedValueOnce({
      targets: [{ ...target, actionId: "action-rotated" }],
      truncated: false,
    })

    session.loadTargets("unusedMcpServers", true)

    await waitFor(() => expect(adapter.getTargets).toHaveBeenCalledTimes(2))
    expect(dialog).toBeVisible()
    fireEvent.click(within(dialog).getByRole("button", { name: "Apply change" }))
    expect(commands.apply).toHaveBeenCalledWith("prepared-1")
  })

  it("accepts a deferred prepare after the action handle rotates for the same attempt", async () => {
    const pending = deferred<PrepareAutoFixBurnCheckTargetOutcome | null>()
    commands.prepare.mockReturnValueOnce(pending.promise)
    const { adapter, session } = setup()
    fireEvent.click(await screen.findByRole("button", { name: "Fix" }))
    vi.mocked(adapter.getTargets).mockResolvedValueOnce({
      targets: [{ ...target, actionId: "action-rotated" }],
      truncated: false,
    })

    session.loadTargets("unusedMcpServers", true)
    await waitFor(() => expect(adapter.getTargets).toHaveBeenCalledTimes(2))
    await act(async () => {
      pending.resolve({
        outcome: "reviewReady",
        review: {
          preparedOperationId: "prepared-rotated",
          expiresAtEpoch: 100,
          agent: "claude-code",
          scope: "global",
          setting: "model",
          configFile: "~/.claude/settings.json",
          currentValue: "claude-opus-4-6",
          proposedValue: "claude-sonnet-5",
          effect: "futureModelSelection",
          sideEffect: "modelBehaviorMayChange",
        },
      })
    })

    expect(screen.getByRole("dialog", { name: "Fix claude-opus-4-6" })).toBeVisible()
  })

  it("ignores a deferred prepare from an attempt that recurred", async () => {
    const pending = deferred<PrepareAutoFixBurnCheckTargetOutcome | null>()
    commands.prepare.mockReturnValueOnce(pending.promise)
    const { adapter, session } = setup()
    fireEvent.click(await screen.findByRole("button", { name: "Fix" }))
    vi.mocked(adapter.getTargets).mockResolvedValueOnce({
      targets: [recurredTarget("action-after-recurrence")],
      truncated: false,
    })

    session.loadTargets("unusedMcpServers", true)
    await waitFor(() => expect(screen.getByRole("button", { name: "Fix" })).toBeEnabled())
    await act(async () => {
      pending.resolve({
        outcome: "reviewReady",
        review: {
          preparedOperationId: "prepared-stale",
          expiresAtEpoch: 100,
          agent: "claude-code",
          scope: "global",
          setting: "model",
          configFile: "~/.claude/settings.json",
          currentValue: "claude-opus-4-6",
          proposedValue: "claude-sonnet-5",
          effect: "futureModelSelection",
          sideEffect: "modelBehaviorMayChange",
        },
      })
    })

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
    expect(commands.noteInteraction).toHaveBeenCalledWith({
      kind: "burnCheckAutoFixReviewed",
      outcome: "ready",
    })
    expect(
      commands.noteInteraction.mock.calls.filter(
        ([interaction]) => interaction.kind === "burnCheckAutoFixReviewed",
      ),
    ).toHaveLength(1)
  })

  it("ignores a deferred prompt from an attempt that recurred", async () => {
    const pending = deferred<CopyPromptFixBurnCheckTargetOutcome | null>()
    commands.copy.mockReturnValueOnce(pending.promise)
    const { adapter, session } = setup()
    fireEvent.click(await screen.findByRole("button", { name: "Copy fix prompt" }))
    vi.mocked(adapter.getTargets).mockResolvedValueOnce({
      targets: [recurredTarget("action-after-recurrence")],
      truncated: false,
    })

    session.loadTargets("unusedMcpServers", true)
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Copy fix prompt" })).toBeEnabled(),
    )
    await act(async () => {
      pending.resolve({
        outcome: "promptReady",
        prompt: "Stale backend prompt",
        watch: {
          watchId: "watch-1",
          origin: "action",
          lifecycle: "watching",
          verification: { status: "watching" },
          savings: { status: "pending" },
        },
      })
    })

    expect(navigator.clipboard.writeText).not.toHaveBeenCalled()
    expect(screen.getByRole("button", { name: "Copy fix prompt" })).toBeEnabled()
    expect(commands.noteInteraction).toHaveBeenCalledWith({
      kind: "burnCheckPromptPrepared",
      outcome: "ready",
    })
    expect(
      commands.noteInteraction.mock.calls.filter(
        ([interaction]) => interaction.kind === "burnCheckPromptPrepared",
      ),
    ).toHaveLength(1)
  })

  it("keeps a stale apply pending, then ignores its completion after recurrence", async () => {
    const pending = deferred<ApplyPreparedBurnCheckOperationOutcome | null>()
    commands.apply.mockReturnValueOnce(pending.promise)
    const { adapter, session } = setup()
    fireEvent.click(await screen.findByRole("button", { name: "Fix" }))
    const dialog = await screen.findByRole("dialog", { name: "Fix claude-opus-4-6" })
    fireEvent.click(within(dialog).getByRole("button", { name: "Apply change" }))
    vi.mocked(adapter.getTargets).mockResolvedValueOnce({
      targets: [recurredTarget("action-after-recurrence")],
      truncated: false,
    })

    session.loadTargets("unusedMcpServers", true)
    await waitFor(() => expect(within(dialog).getByText("Applying…")).toBeVisible())
    expect(dialog).toHaveAttribute("aria-busy", "true")
    await act(async () => {
      pending.resolve({ outcome: "appliedAwaitingVerification", watchId: "watch-old" })
    })

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
    const currentFix = screen.getByRole("button", { name: "Fix" })
    expect(currentFix).toBeEnabled()
    expect(currentFix).toHaveFocus()
    expect(screen.queryByRole("button", { name: "Change applied" })).not.toBeInTheDocument()
    expect(commands.noteInteraction).toHaveBeenCalledWith({
      kind: "burnCheckAutoFixCompleted",
      outcome: "applied_awaiting_verification",
    })
    expect(
      commands.noteInteraction.mock.calls.filter(
        ([interaction]) => interaction.kind === "burnCheckAutoFixCompleted",
      ),
    ).toHaveLength(1)
  })

  it("resets copied state when the same watch records a recurrence", async () => {
    const { adapter, session } = setup()
    fireEvent.click(await screen.findByRole("button", { name: "Copy fix prompt" }))
    await screen.findByRole("button", { name: "Copied" })
    vi.mocked(adapter.getTargets).mockResolvedValueOnce({
      targets: [
        {
          ...target,
          actionId: "action-after-recurrence",
          watch: {
            watchId: "watch-1",
            origin: "action",
            lifecycle: "recurred",
            verification: { status: "recurred", methodRevision: 1, evidenceRevision: "e2" },
            savings: { status: "pending" },
          },
        },
      ],
      truncated: false,
    })

    session.loadTargets("unusedMcpServers", true)

    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Copy fix prompt" })).toBeEnabled(),
    )
  })

  it.each([
    ["conflict", { outcome: "conflict" }, "Another prepared change conflicts"],
    [
      "unavailable",
      { outcome: "unavailable", reason: "unsupportedOrUnprovenTarget" },
      "can no longer prove a safe write target",
    ],
  ] as const)("distinguishes the typed %s review outcome", async (_name, outcome, text) => {
    commands.prepare.mockResolvedValueOnce(outcome)
    setup()

    fireEvent.click(await screen.findByRole("button", { name: "Fix" }))

    expect(await screen.findByRole("alert")).toHaveTextContent(text)
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
  })

  it("renders reasoning operation effects", async () => {
    commands.prepare.mockResolvedValueOnce({
      outcome: "reviewReady",
      review: {
        preparedOperationId: "prepared-reasoning",
        expiresAtEpoch: 100,
        agent: "claude-code",
        scope: "project",
        setting: "reasoning",
        configFile: "~/.claude/settings.json",
        currentValue: "max",
        proposedValue: "medium",
        effect: "futureReasoningEffort",
        sideEffect: "responsesMayUseLessReasoning",
      },
    })
    setup()
    fireEvent.click(await screen.findByRole("button", { name: "Fix" }))
    const dialog = await screen.findByRole("dialog")
    expect(dialog).toHaveTextContent("This plan lowers reasoning effort for future requests")
  })

  it("retries clipboard failure without creating a second backend action", async () => {
    vi.mocked(navigator.clipboard.writeText)
      .mockRejectedValueOnce(new Error("Denied"))
      .mockResolvedValueOnce(undefined)
    setup(target, false, aggregate, report)
    const copy = await screen.findByRole("button", { name: "Copy fix prompt" })
    fireEvent.click(copy)
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not copy")
    fireEvent.click(copy)
    await screen.findByRole("button", { name: "Copied" })
    expect(commands.copyBatch).toHaveBeenCalledOnce()
    expect(navigator.clipboard.writeText).toHaveBeenCalledTimes(2)
    expect(commands.noteInteraction.mock.calls).toEqual(
      expect.arrayContaining([
        [{ kind: "burnCheckPromptPrepared", outcome: "ready" }],
        [{ kind: "burnCheckPromptCopied" }],
      ]),
    )
    expect(
      commands.noteInteraction.mock.calls.filter(
        ([interaction]) => interaction.kind === "burnCheckPromptPrepared",
      ),
    ).toHaveLength(1)
  })

  it("records a failed Auto Fix result without claiming success", async () => {
    commands.apply.mockRejectedValueOnce(new Error("Private backend error"))
    setup(target, false, aggregate, report)
    fireEvent.click(await screen.findByRole("button", { name: "Fix" }))
    const dialog = await screen.findByRole("dialog", { name: "Fix claude-opus-4-6" })
    fireEvent.click(within(dialog).getByRole("button", { name: "Apply change" }))

    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      "Could not confirm the result. Check the setting before you try again.",
    )
    expect(screen.getByRole("dialog", { name: "Fix claude-opus-4-6" })).toBeVisible()
    expect(commands.noteInteraction).toHaveBeenCalledWith({
      kind: "burnCheckAutoFixCompleted",
      outcome: "failed",
    })
    expect(commands.noteInteraction).not.toHaveBeenCalledWith({
      kind: "burnCheckAutoFixCompleted",
      outcome: "applied_awaiting_verification",
    })
    expect(JSON.stringify(commands.noteInteraction.mock.calls)).not.toContain(
      "Private backend error",
    )
  })

  it("routes samples by opaque handle and shows typed unavailable states", async () => {
    commands.openSample.mockResolvedValueOnce({ outcome: "deleted" })
    setup()
    const samples = await screen.findByRole("button", { name: /Sample sessions/ })
    fireEvent.click(samples)
    fireEvent.click(screen.getByRole("button", { name: "Open sample session Update model" }))
    expect(await screen.findByText("This sample session was deleted.")).toHaveAttribute(
      "role",
      "status",
    )
    expect(commands.openSample).toHaveBeenCalledWith("opaque-handle")
  })

  it("keeps a sample card busy while its opaque route opens", async () => {
    const opening = deferred<{ outcome: "opened" }>()
    commands.openSample.mockReturnValueOnce(opening.promise)
    setup()
    fireEvent.click(await screen.findByRole("button", { name: /Sample sessions/ }))
    const sample = screen.getByRole("button", { name: "Open sample session Update model" })

    fireEvent.click(sample)

    expect(sample).toHaveAttribute("aria-busy", "true")
    expect(sample).toBeDisabled()
    await act(async () => opening.resolve({ outcome: "opened" }))
    await waitFor(() => expect(sample).toBeEnabled())
  })

  it.each([
    [
      "recurred",
      { status: "recurred", methodRevision: 1, evidenceRevision: "e2" },
      "This finding returned after it was verified.",
    ],
    [
      "recovery",
      { status: "recoveryNeeded", reason: "writeOutcomeUnknown" },
      "The write result is uncertain. Review the setting before another change.",
    ],
  ] as const)("renders the typed %s state", async (_name, verification, expected) => {
    setup(
      {
        ...target,
        autoFix: { status: "unavailable", reason: "activeWatch" },
        watch: {
          watchId: "watch-1",
          origin: "action",
          lifecycle: verification.status === "recurred" ? "recurred" : "recoveryNeeded",
          verification,
          savings: { status: "pending" },
        },
      },
      false,
      aggregate,
      report,
    )

    expect(await screen.findByText(expected)).toBeVisible()
  })

  it("hides passive verification status", async () => {
    setup(
      {
        ...target,
        watch: {
          watchId: "watch-1",
          origin: "action",
          lifecycle: "fixed",
          verification: { status: "fixed", methodRevision: 1, evidenceRevision: "e2" },
          savings: { status: "pending" },
        },
      },
      false,
      aggregate,
      report,
    )

    await screen.findByText("Some sessions used an older model when a newer one was available.")
    expect(
      screen.queryByText("Fresh evidence verified this improvement."),
    ).not.toBeInTheDocument()
  })

  it("hides bounded target and retained-detail notices", async () => {
    setup(target, true, aggregate, report)

    await screen.findByText("Some sessions used an older model when a newer one was available.")
    expect(screen.queryByText(/bounded view can show/)).not.toBeInTheDocument()
    expect(screen.queryByText(/Previous details remain visible/)).not.toBeInTheDocument()
  })

  it("copies a fallback prompt for an empty failed check without showing Auto Fix", async () => {
    setup(null)

    expect(await screen.findByText("Some MCP servers were loaded but not used.")).toBeVisible()
    expect(
      screen.queryByText(/bounded view|exact target|nothing safe/i),
    ).not.toBeInTheDocument()
    expect(screen.queryByText(/Auto Fix unavailable/i)).not.toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "Fix" })).not.toBeInTheDocument()

    fireEvent.click(screen.getByRole("button", { name: "Copy fix prompt" }))

    await waitFor(() =>
      expect(navigator.clipboard.writeText).toHaveBeenCalledWith(
        "Inspect representative evidence.",
      ),
    )
    expect(commands.copyFallback).toHaveBeenCalledWith("unusedMcpServers")
    const copied = screen.getByRole("button", { name: "Copied" })
    expect(copied).toBeDisabled()
    expect(copied).not.toHaveClass("text-token-in")
    expect(copied.querySelector(".lucide-check")).toHaveClass("text-token-in")
  })

  it("shows a retryable fallback prompt error", async () => {
    commands.copyFallback.mockRejectedValueOnce(new Error("Private backend error"))
    setup(null)

    fireEvent.click(await screen.findByRole("button", { name: "Copy fix prompt" }))

    expect(await screen.findByRole("alert")).toHaveTextContent("Could not copy the prompt")
    expect(screen.getByRole("button", { name: "Copy fix prompt" })).toBeEnabled()
    expect(JSON.stringify(commands.noteInteraction.mock.calls)).not.toContain(
      "Private backend error",
    )
  })

  it("ignores a fallback prompt that completes after an exact target appears", async () => {
    const pending = deferred<{ outcome: "promptReady"; prompt: string } | null>()
    commands.copyFallback.mockReturnValueOnce(pending.promise)
    const { adapter, session } = setup(null)
    fireEvent.click(await screen.findByRole("button", { name: "Copy fix prompt" }))
    vi.mocked(adapter.getTargets).mockResolvedValueOnce({ targets: [target], truncated: false })

    session.loadTargets("unusedMcpServers", true)
    await screen.findByRole("heading", { name: "claude-opus-4-6" })
    await act(async () => pending.resolve({ outcome: "promptReady", prompt: "Stale prompt" }))

    expect(navigator.clipboard.writeText).not.toHaveBeenCalled()
    expect(screen.queryByRole("button", { name: "Copied" })).not.toBeInTheDocument()
    expect(commands.noteInteraction).toHaveBeenCalledWith({
      kind: "burnCheckPromptPrepared",
      outcome: "ready",
    })
  })

  it("keeps a pending apply modal locked until the write completes", async () => {
    let resolveApply!: (value: Awaited<ReturnType<typeof commands.apply>>) => void
    commands.apply.mockReturnValueOnce(
      new Promise((resolve) => {
        resolveApply = resolve
      }),
    )
    setup()
    fireEvent.click(await screen.findByRole("button", { name: "Fix" }))
    const dialog = await screen.findByRole("dialog", { name: "Fix claude-opus-4-6" })
    fireEvent.click(within(dialog).getByRole("button", { name: "Apply change" }))

    expect(within(dialog).getByRole("button", { name: "Cancel" })).toBeDisabled()
    fireEvent.keyDown(dialog, { key: "Escape" })
    fireEvent.mouseDown(dialog.parentElement!)
    expect(screen.getByRole("dialog", { name: "Fix claude-opus-4-6" })).toHaveAttribute(
      "aria-busy",
      "true",
    )
    expect(commands.apply).toHaveBeenCalledOnce()

    resolveApply({ outcome: "appliedAwaitingVerification", watchId: "watch-1" })
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument())
  })

  it("hides the implied auto-fix reason when a prompt action is available", async () => {
    setup({
      ...target,
      autoFix: { status: "unavailable", reason: "safetyCheckFailed" },
    })

    expect(await screen.findByRole("button", { name: "Copy fix prompt" })).toBeVisible()
    expect(screen.queryByText(/write safety check/)).not.toBeInTheDocument()
  })

  it("shows one direct reason when neither action is available", async () => {
    setup({
      ...target,
      autoFix: { status: "unavailable", reason: "safetyCheckFailed" },
      promptFix: { status: "unavailable", reason: "unsupportedSourceFormat" },
    })

    expect(
      await screen.findByText("The current setting did not pass the write safety check."),
    ).toBeVisible()
    expect(screen.queryByText(/^Automatic fix:/)).not.toBeInTheDocument()
    expect(screen.queryByText(/^Prompt fix:/)).not.toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "Fix" })).not.toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "Copy fix prompt" })).not.toBeInTheDocument()
  })

  it.each([
    ["conflict", { outcome: "conflict" }, "Another change now conflicts"],
    [
      "unavailable",
      { outcome: "unavailable", reason: "safetyCheckFailed" },
      "no longer passes the write safety check",
    ],
    [
      "recovery",
      { outcome: "recoveryNeeded", watchId: "watch-recovery" },
      "The write result is uncertain",
    ],
  ] as const)(
    "keeps the typed %s apply outcome in the review",
    async (_name, outcome, text) => {
      commands.apply.mockResolvedValueOnce(outcome)
      setup()
      fireEvent.click(await screen.findByRole("button", { name: "Fix" }))
      const dialog = await screen.findByRole("dialog", { name: "Fix claude-opus-4-6" })
      fireEvent.click(within(dialog).getByRole("button", { name: "Apply change" }))

      expect(await within(dialog).findByRole("alert")).toHaveTextContent(text)
      expect(within(dialog).getByRole("button", { name: "Apply change" })).toBeDisabled()
      expect(within(dialog).getByRole("button", { name: "Close" })).toBeEnabled()
    },
  )

  it("shows one check-level finding before its actions", async () => {
    setup(target, false, aggregate, report)
    const finding = await screen.findByText(
      "Some sessions used an older model when a newer one was available.",
    )
    const fix = screen.getByRole("button", { name: "Fix" })

    expect(finding.compareDocumentPosition(fix) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0)
    expect(screen.queryByRole("heading", { name: "claude-opus-4-6" })).not.toBeInTheDocument()
    expect(screen.queryByText(target.finding.observation)).not.toBeInTheDocument()
  })

  it("keeps expanded check content visually light", async () => {
    setup(target, false, aggregate, report)
    const finding = await screen.findByText(
      "Some sessions used an older model when a newer one was available.",
    )
    const detail = finding.closest("article")!

    expect(detail).not.toHaveClass("border-t", "border-separator")
    expect(within(detail).getByRole("button", { name: /Sample sessions/ })).toHaveClass(
      "type-callout",
      "text-label-secondary",
    )
    expect(
      within(detail).queryByText(/API-equivalent cost opportunity/),
    ).not.toBeInTheDocument()
  })

  it("does not repeat target opportunities in a check-level detail", async () => {
    setup(
      {
        ...target,
        display: {
          ...target.display,
          estimatedOpportunity: { value: 12.5, unit: "apiEquivalentUsd" },
        },
      },
      false,
      aggregate,
      report,
    )

    await screen.findByText("Some sessions used an older model when a newer one was available.")
    expect(screen.queryByText(/API-equivalent cost opportunity/)).not.toBeInTheDocument()
    expect(screen.queryByText("Estimated opportunity:")).not.toBeInTheDocument()
    expect(screen.queryByText("Estimate method:")).not.toBeInTheDocument()
  })

  it("does not invent an estimated opportunity when it is unavailable", async () => {
    setup(target, false, aggregate, report)

    await screen.findByText("Some sessions used an older model when a newer one was available.")
    expect(screen.queryByText("Estimated opportunity:")).not.toBeInTheDocument()
    expect(screen.queryByText(/ opportunity$/)).not.toBeInTheDocument()
  })

  it("groups aggregate wins and states partial metric coverage", async () => {
    setup(target, false, {
      wins: [
        aggregate.wins[0]!,
        {
          ...aggregate.wins[0]!,
          findingId: "win-2",
          savings: {
            tokenSavings: 1200,
            apiEquivalentCostAvoidedUsd: null,
            improvementCount: null,
            method: null,
          },
        },
      ],
    })

    const savings = await screen.findByRole("region", { name: "Your savings" })
    expect(within(savings).getByText("2 verified wins")).toBeVisible()
    expect(within(savings).getAllByText("~1,200 saved from 1 of 2 wins")).toHaveLength(2)
    expect(within(savings).getAllByText("~$1.25 saved from 1 of 2 wins")).toHaveLength(2)
    expect(within(savings).getByText("Count known for 1 of 2 wins")).toBeVisible()
  })

  it("pairs authoritative token and cost totals that cover the same wins", async () => {
    setup(target, false, {
      wins: [
        {
          ...aggregate.wins[0]!,
          savings: {
            tokenSavings: 1200,
            apiEquivalentCostAvoidedUsd: 1.25,
            improvementCount: 2,
            method: "oldModelPriceDifference",
          },
        },
      ],
    })

    const savings = await screen.findByRole("region", { name: "Your savings" })
    const total = within(savings).getByText("~1,200 · ~$1.25 saved")
    expect(total).toBeVisible()
    expect(total).toHaveClass("text-label-secondary")
    expect(within(savings).getByText("Old model usage")).toHaveClass("text-label")
    expect(within(savings).getByText("2 improvements across 1 check")).toBeVisible()
  })
})
