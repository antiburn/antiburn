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
import type * as ClipboardModule from "../../lib/clipboard"
import type * as InsightsIpcModule from "../../lib/insightsIpc"
import type * as IpcModule from "../../lib/ipc"
import { BurnChecksSession, type BurnChecksAdapter } from "./BurnChecksSession"
import { BurnChecksView } from "./BurnChecksView"
import { BurnCheckDetail, CheckPromptAction } from "./burn-checks/BurnCheckDetail"

const commands = vi.hoisted(() => ({
  prepare: vi.fn(),
  apply: vi.fn(),
  copy: vi.fn(),
  copyFallback: vi.fn(),
  copyBatch: vi.fn(),
  writeClipboardText: vi.fn(),
  openSample: vi.fn(),
  openSettings: vi.fn(),
  noteInteraction: vi.fn(),
}))

const innerWidth = Object.getOwnPropertyDescriptor(window, "innerWidth")

function setWindowWidth(value: number): void {
  Object.defineProperty(window, "innerWidth", { configurable: true, value })
  fireEvent(window, new Event("resize"))
}

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
  openSettingsWindow: commands.openSettings,
  noteInteraction: commands.noteInteraction,
}))

vi.mock("../../lib/clipboard", async (importOriginal) => ({
  ...(await importOriginal<typeof ClipboardModule>()),
  writeClipboardText: commands.writeClipboardText,
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
  commands.openSettings.mockResolvedValue(undefined)
  commands.writeClipboardText.mockResolvedValue(undefined)
})

afterEach(() => {
  vi.useRealTimers()
  if (innerWidth) Object.defineProperty(window, "innerWidth", innerWidth)
})

describe("BurnChecksView", () => {
  it("keeps anchored selection and sample disclosure state across window resizes", async () => {
    setWindowWidth(1400)
    const { view } = setup(target, false, aggregate, {
      ...report,
      categories: [
        report.categories[0]!,
        {
          id: "modelOverthinking",
          finding: 2,
          clean: 1,
          unavailable: 0,
          estimatedTokenBurnBasisPoints: 400,
        },
        report.categories[1]!,
      ],
    })

    const oldModel = await screen.findByRole("button", { name: /Old model usage/ })
    const overthinking = screen.getByRole("button", { name: /Model overthinking/ })
    expect(screen.getByRole("heading", { name: "Failed checks 2" })).toBeVisible()
    expect(oldModel).toHaveAttribute("aria-pressed", "true")

    fireEvent.keyDown(oldModel, { key: "ArrowDown" })
    await waitFor(() => expect(overthinking).toHaveAttribute("aria-pressed", "true"))
    expect(overthinking).toHaveFocus()
    fireEvent.keyDown(overthinking, { key: "Enter" })
    await waitFor(() =>
      expect(document.getElementById("burn-check-modelOverthinking-detail")).toHaveFocus(),
    )

    const samples = await screen.findByRole("button", { name: "Sample session 1" })
    fireEvent.click(samples)
    expect(screen.getByRole("button", { name: "Sample session 1" })).toHaveAttribute(
      "aria-expanded",
      "false",
    )

    setWindowWidth(1000)
    const resizedOverthinking = await screen.findByRole("button", {
      name: /Model overthinking/,
    })
    expect(resizedOverthinking).toHaveAttribute("aria-pressed", "true")
    expect(resizedOverthinking).not.toHaveAttribute("aria-expanded")
    expect(
      within(document.getElementById("burn-check-modelOverthinking-detail")!).getByRole(
        "button",
        { name: "Sample session 1" },
      ),
    ).toBeVisible()
    const resizedOldModel = screen.getByRole("button", { name: /Old model usage/ })
    expect(resizedOldModel).toHaveAttribute("aria-pressed", "false")
    expect(resizedOldModel).not.toHaveAttribute("aria-expanded")
    expect(view.container.querySelector(".burn-checks-collection")).toHaveClass(
      "main-window-collection",
    )
    expect(view.container.querySelector(".burn-checks-detail-pane")).toHaveClass(
      "main-window-detail",
    )
  })

  it("keeps the complete collection before the detail at minimum desktop width", async () => {
    setWindowWidth(1000)
    setup(target, false, aggregate, {
      ...report,
      categories: [
        report.categories[0]!,
        {
          id: "modelOverthinking",
          finding: 2,
          clean: 1,
          unavailable: 0,
          estimatedTokenBurnBasisPoints: 400,
        },
      ],
    })

    const first = await screen.findByRole("button", { name: /Old model usage/ })
    const detail = document.getElementById("burn-check-oldModelUsage-detail")!
    const action = await within(detail).findByRole("button", { name: "Copy fix prompt" })
    const second = screen.getByRole("button", { name: /Model overthinking/ })

    expect(first.compareDocumentPosition(second) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0)
    expect(second.compareDocumentPosition(action) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(
      0,
    )
    expect(first).toHaveAttribute("aria-pressed", "true")
    expect(first).not.toHaveAttribute("aria-expanded")
  })

  it("opens passed checks when a live report changes from failed to pass-only", async () => {
    setWindowWidth(1400)
    const { adapter, session } = setup(target, false, aggregate, {
      ...report,
      categories: [report.categories[0]!],
    })

    expect(
      await screen.findByRole("button", { name: /Old model usage.*1 failed/ }),
    ).toHaveAttribute("aria-pressed", "true")
    vi.mocked(adapter.getReport).mockResolvedValue({
      ...report,
      categories: [{ ...report.categories[0]!, finding: 0, clean: 4 }],
    })
    act(() => session.refresh())

    expect(await screen.findByRole("button", { name: /Passed checks/ })).toHaveAttribute(
      "aria-expanded",
      "true",
    )
    expect(screen.getByRole("button", { name: /Passed checks/ })).toHaveAttribute(
      "aria-controls",
      "burn-checks-passed-body",
    )
    expect(document.getElementById("burn-checks-passed-body")).toBeVisible()
    expect(screen.getByRole("button", { name: /Old model usage.*0 failed/ })).toHaveAttribute(
      "aria-pressed",
      "true",
    )
  })

  it("selects the first assessed check when unavailable evidence becomes assessed", async () => {
    setWindowWidth(1400)
    const { adapter, session } = setup(null, false, aggregate, {
      ...report,
      categories: [report.categories[2]!],
    })

    expect(screen.queryByText("1 check not assessed.")).not.toBeInTheDocument()
    vi.mocked(adapter.getReport).mockResolvedValue({
      ...report,
      categories: [{ ...report.categories[1]!, finding: 0, clean: 3 }],
    })
    act(() => session.refresh())

    expect(
      await screen.findByRole("button", { name: /Unused skills.*0 failed/ }),
    ).toHaveAttribute("aria-pressed", "true")
  })

  it("does not resurrect a removed selection when its category returns", async () => {
    setWindowWidth(1400)
    const twoFailures: ChecksReportPayload = {
      ...report,
      categories: [
        report.categories[0]!,
        {
          id: "modelOverthinking",
          finding: 2,
          clean: 1,
          unavailable: 0,
          estimatedTokenBurnBasisPoints: 400,
        },
      ],
    }
    const { adapter, session } = setup(target, false, aggregate, twoFailures)
    const overthinking = await screen.findByRole("button", { name: /Model overthinking/ })
    fireEvent.click(overthinking)
    expect(overthinking).toHaveAttribute("aria-pressed", "true")

    vi.mocked(adapter.getReport).mockResolvedValue({
      ...report,
      categories: [report.categories[0]!],
    })
    act(() => session.refresh())
    const oldModel = await screen.findByRole("button", { name: /Old model usage/ })
    await waitFor(() => expect(oldModel).toHaveAttribute("aria-pressed", "true"))

    vi.mocked(adapter.getReport).mockResolvedValue(twoFailures)
    act(() => session.refresh())
    await screen.findByRole("button", { name: /Model overthinking/ })
    expect(screen.getByRole("button", { name: /Old model usage/ })).toHaveAttribute(
      "aria-pressed",
      "true",
    )
  })

  it("keeps the explanation and disabled finding actions together in the category header", async () => {
    setup(target, false, aggregate, report)
    const action = await screen.findByRole("button", { name: "Copy fix prompt" })
    expect(action).toHaveAttribute("aria-disabled", "true")
    expect(action.closest("header")).toContainElement(
      screen.getByRole("heading", { name: "Old model usage", level: 2 }),
    )
    expect(action.closest("header")).toHaveTextContent(
      "Some sessions used an older model when a newer one was available.",
    )
    expect(
      screen.getAllByText("Some sessions used an older model when a newer one was available."),
    ).toHaveLength(1)
    expect(action.parentElement).toContainElement(
      screen.getByRole("button", { name: "Snooze" }),
    )
    fireEvent.click(action)
    expect(commands.copyBatch).not.toHaveBeenCalled()
    expect(commands.writeClipboardText).not.toHaveBeenCalled()
    act(() => action.focus())
    expect(await screen.findByRole("tooltip")).toHaveTextContent("Coming soon")
    expect(screen.getByRole("region", { name: "Burn check details" })).toHaveTextContent(
      "1 failed · 2 passed",
    )
    expect(
      screen.queryByRole("heading", { name: "Burn checks", level: 2 }),
    ).not.toBeInTheDocument()
  })

  it("shows Snoozed as an empty group", async () => {
    setup(target, false, aggregate, report)
    const snoozed = await screen.findByRole("button", { name: "Snoozed 0" })
    expect(snoozed).toHaveAttribute("aria-expanded", "false")
    fireEvent.click(snoozed)
    expect(snoozed).toHaveAttribute("aria-expanded", "true")
    expect(screen.queryByText(/Reminders are coming soon/)).not.toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Snooze" })).not.toHaveAttribute("aria-disabled")
    fireEvent.click(snoozed)
    expect(snoozed).toHaveAttribute("aria-expanded", "false")
  })

  it("uses only the category agent inventory for neutral vendor watermarks", async () => {
    setup(target, false, aggregate, {
      ...report,
      categories: report.categories.map((check, index) => ({
        ...check,
        agents: index === 0 ? ["codex", "codex", "cursor", "unknown"] : [],
      })),
    })
    const row = await screen.findByRole("button", { name: /Old model usage, 1 failed/ })
    const watermarks = row.querySelector("[data-check-vendor-watermarks]")
    expect(watermarks).toHaveAttribute("aria-hidden", "true")
    expect(watermarks?.querySelectorAll("[data-agent-icon]")).toHaveLength(2)
    expect(watermarks?.querySelector('[data-agent-icon="codex"]')).toBeInTheDocument()
    expect(watermarks?.querySelector('[data-agent-icon="cursor"]')).toBeInTheDocument()
    expect(watermarks?.querySelector('[data-agent-icon="claude"]')).not.toBeInTheDocument()
  })

  it("normalizes Claude evidence aliases and retains both vendor marks", async () => {
    setup(target, false, aggregate, {
      ...report,
      categories: report.categories.map((check) => ({
        ...check,
        agents: ["claude", "claude-code", "codex", "unknown"],
      })),
    })
    const row = await screen.findByRole("button", { name: /Old model usage, 1 failed/ })
    const watermarks = row.querySelector("[data-check-vendor-watermarks]")
    expect(watermarks?.querySelectorAll("[data-agent-icon]")).toHaveLength(2)
    expect(watermarks?.querySelector('[data-agent-icon="claude"]')).toBeInTheDocument()
    expect(watermarks?.querySelector('[data-agent-icon="codex"]')).toBeInTheDocument()
  })

  it("includes all listed targets in a large check prompt", async () => {
    const targets = Array.from({ length: 13 }, (_, index) => ({
      ...target,
      findingId: `finding-${index}`,
      actionId: `action-${index}`,
    }))
    render(<CheckPromptAction detector="oldModelUsage" targets={targets} refresh={vi.fn()} />)

    fireEvent.click(await screen.findByRole("button", { name: "Copy fix prompt" }))

    await waitFor(() =>
      expect(commands.copyBatch).toHaveBeenCalledWith(
        Array.from({ length: 13 }, (_, index) => `action-${index}`),
      ),
    )
  })

  it("keeps aggregate burn in dismissible assessment details and counts beside groups", async () => {
    setup(target, false, aggregate, report)
    const trigger = await screen.findByRole("button", { name: "Assessment details" })
    expect(screen.getByRole("heading", { name: "Failed checks 1" })).toBeVisible()
    expect(screen.getByRole("button", { name: "Passed checks 1" })).toBeVisible()
    expect(screen.queryByText("1 check not assessed.")).not.toBeInTheDocument()
    expect(screen.queryByText("Estimated token burn")).not.toBeInTheDocument()
    expect(trigger.closest("header")).toContainElement(
      screen.getByRole("heading", { name: "Failed checks 1" }),
    )
    fireEvent.click(trigger)
    const details = screen.getByRole("region", { name: "Assessment details" })
    expect(within(details).getByText("8%")).toBeVisible()
    expect(within(details).getByText(/Estimate includes only checks/)).toBeVisible()
    fireEvent.keyDown(document, { key: "Escape" })
    expect(screen.queryByRole("region", { name: "Assessment details" })).not.toBeInTheDocument()
    expect(trigger).toHaveFocus()
    fireEvent.click(trigger)
    fireEvent.pointerDown(document.body)
    expect(trigger).toHaveAttribute("aria-expanded", "false")
  })

  it("opens Insights coverage details from the assessment summary", async () => {
    setup(target, false, aggregate, { ...report, evidenceSettled: true })

    fireEvent.click(await screen.findByRole("button", { name: "Assessment details" }))
    const summary = screen.getByRole("region", { name: "Assessment details" })
    expect(
      within(summary).getByText("Assessment complete for available evidence."),
    ).toBeVisible()
    fireEvent.click(within(summary).getByRole("button", { name: "Coverage details" }))

    expect(commands.openSettings).toHaveBeenCalledExactlyOnceWith("insights")
  })

  it("keeps processing count out of the collection header", async () => {
    setup(target, false, aggregate, { ...report, pendingEvidence: 2 })

    expect(screen.queryByText("2 sessions processing.")).not.toBeInTheDocument()
  })

  it("shows a pass-only outcome and opens its counted disclosure", async () => {
    setup(
      null,
      false,
      { wins: [] },
      {
        ...report,
        estimatedTokenBurnBasisPoints: 0,
        categories: [
          {
            id: "unusedSkills",
            finding: 0,
            clean: 4,
            unavailable: 0,
            estimatedTokenBurnBasisPoints: 0,
          },
        ],
      },
    )

    expect(await screen.findByRole("button", { name: "Passed checks 1" })).toBeVisible()
    expect(screen.queryByRole("heading", { name: /Failed checks/ })).not.toBeInTheDocument()
    expect(screen.getByRole("button", { name: /Passed checks/ })).toHaveAttribute(
      "aria-expanded",
      "true",
    )
  })

  it("renders assessed checks and concise failed details", async () => {
    setup(target, false, aggregate, report)
    const row = await screen.findByRole("button", { name: /Old model usage.*8% burn/ })
    expect(row).toBeVisible()
    expect(within(row).getByText("8% burn")).toBeVisible()
    expect(row.querySelector(".lucide-flame")).toBeInTheDocument()
    expect(row.querySelector('[role="meter"]')).not.toBeInTheDocument()
    expect(within(row).getByText("2 passed")).toHaveClass("text-label-secondary")
    expect(screen.getByRole("heading", { name: "Failed checks 1" })).toBeVisible()
    expect(screen.queryByText(/More evidence is needed/)).not.toBeInTheDocument()
    expect(screen.getByRole("heading", { name: /Passed checks/ })).toBeVisible()
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

  it("groups wide check rows and exposes the selected outcome state", async () => {
    setWindowWidth(1400)
    setup()

    const selected = await screen.findByRole("button", { name: /Unused MCP servers/ })
    expect(selected).toHaveAttribute("aria-pressed", "true")
    expect(selected).toHaveAttribute("data-outcome", "failed")
    expect(selected.querySelector(".lucide-chevron-right")).not.toBeInTheDocument()
    expect(selected.querySelector(".rounded-full")).toHaveClass("bg-surface-card")
    expect(selected.closest(".burn-checks-group-body")).toBeInTheDocument()
  })

  it("shows named target counts and truncated result wording", async () => {
    setWindowWidth(1400)
    const first = setup(target, true)

    expect(await screen.findByText(/1 affected resource shown$/)).toBeVisible()
    first.view.unmount()

    setup(target)
    expect(await screen.findByText(/1 affected resource$/)).toBeVisible()
  })

  it("uses report session totals when named targets share bounded samples", async () => {
    setup(
      [target, { ...target, findingId: "second-target", actionId: "second-action" }],
      true,
      aggregate,
      {
        ...namedTargetReport,
        categories: [{ ...namedTargetReport.categories[0]!, finding: 23 }],
      },
    )

    const resources = await screen.findByText(/2 affected resources shown$/)
    expect(resources.parentElement).toHaveTextContent(
      "23 sessions affected · 2 affected resources shown",
    )
    expect(screen.getByRole("region", { name: "Burn check details" })).toHaveTextContent(
      "23 failed",
    )
  })

  it("expands one-sample resources and shows authoritative impact and project identity", async () => {
    setup([
      { ...target, projectName: "antiburn", affectedSessionCount: 12 },
      {
        ...target,
        findingId: "second",
        actionId: "second",
        projectName: "browser-tests",
        affectedSessionCount: 3,
      },
    ])
    const disclosures = await screen.findAllByRole("button", {
      name: "Sample session 1",
    })
    expect(disclosures).toHaveLength(2)
    for (const disclosure of disclosures)
      expect(disclosure).toHaveAttribute("aria-expanded", "true")
    expect(screen.getByText("12 sessions affected")).toBeVisible()
    expect(screen.getByText("3 sessions affected")).toBeVisible()
    expect(screen.getByText(/· antiburn/)).toBeVisible()
    expect(screen.getByText(/· browser-tests/)).toBeVisible()
    expect(
      screen.getAllByRole("button", { name: "Open sample session Update model" }),
    ).toHaveLength(2)
  })

  it("keeps multiple sample sessions collapsed until requested", async () => {
    setWindowWidth(1400)
    setup(
      {
        ...target,
        samples: [
          target.samples[0]!,
          {
            ...target.samples[0]!,
            navigationHandle: "opaque-handle-2",
            title: "Review model",
          },
        ],
      },
      false,
      aggregate,
      report,
    )

    expect(await screen.findByRole("button", { name: "Sample sessions 2" })).toHaveAttribute(
      "aria-expanded",
      "false",
    )
    expect(
      screen.queryByRole("button", { name: "Open sample session Update model" }),
    ).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Sample sessions 2" }))
    expect(
      screen.getByRole("button", { name: "Open sample session Review model" }),
    ).toBeVisible()
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

  it("shows the exact agent mark and configuration scope", async () => {
    setup()

    expect(await screen.findAllByRole("img", { name: "Claude Code" })).toHaveLength(2)
    expect(screen.getByText("Global configuration")).toBeVisible()
  })

  it("uses the pane headers as macOS deep drag regions while loading and after load", async () => {
    const userAgent = vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Macintosh")
    try {
      const pending = deferred<ChecksReportPayload>()
      const { view } = setup(target, false, aggregate, pending.promise)
      const loading = screen.getByRole("region", { name: "Loading Burn checks" })
      const loadingSummary = loading.querySelector(".burn-checks-collection-header")!
      expect(loadingSummary).toHaveAttribute("data-tauri-drag-region", "deep")
      expect(loadingSummary).not.toHaveAttribute("aria-hidden")
      await act(async () => pending.resolve(report))
      await screen.findByRole("button", { name: /Old model usage.*8% burn/ })
      const header = view.container.querySelector(".burn-checks-collection-header")!
      expect(header).toHaveAttribute("data-tauri-drag-region", "deep")
      const detailHeader = view.container.querySelector(".burn-check-detail-heading")!
      expect(detailHeader).toHaveAttribute("data-tauri-drag-region", "deep")
      expect(screen.getByRole("button", { name: "Assessment details" })).not.toHaveAttribute(
        "data-tauri-drag-region",
      )
    } finally {
      userAgent.mockRestore()
    }
  })

  it("uses native title bars without adding a drag strip on Windows", () => {
    const userAgent = vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Windows")
    try {
      const { view } = setup()
      expect(view.container.querySelector("[data-tauri-drag-region]")).toBeNull()
    } finally {
      userAgent.mockRestore()
    }
  })

  it("keeps an overlay drag region in the macOS empty error state", async () => {
    const userAgent = vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Macintosh")
    try {
      const { view } = setup(target, false, aggregate, Promise.reject(new Error("Unavailable")))
      expect(await screen.findByRole("alert")).toHaveTextContent("Burn checks are unavailable.")
      const dragRegion = view.container.querySelector("[data-tauri-drag-region]")!
      expect(dragRegion).toHaveClass("main-window-empty-titlebar")
      expect(dragRegion).toHaveAttribute("aria-hidden", "true")
    } finally {
      userAgent.mockRestore()
    }
  })

  it("uses one busy region and one loading announcement", () => {
    const pending = deferred<ChecksReportPayload>()
    const { view } = setup(target, false, aggregate, pending.promise)

    const loading = screen.getByRole("region", { name: "Loading Burn checks" })
    expect(loading).toHaveAttribute("aria-busy", "true")
    expect(within(loading).getAllByRole("status")).toHaveLength(1)
    expect(view.container.querySelectorAll('[aria-busy="true"]')).toHaveLength(1)
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
    const dialog = await screen.findByRole("dialog", { name: "Review change" })
    expect(dialog).toHaveTextContent("Claude Code · Model · Global configuration")
    expect(dialog).toHaveTextContent("~/.claude/settings.json · model")
    expect(dialog).toHaveTextContent("claude-opus-4-6 → claude-sonnet-5")
    expect(dialog).toHaveTextContent("Responses can change")
    fireEvent.click(within(dialog).getByRole("button", { name: "Apply change" }))
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Change applied" })).toBeDisabled(),
    )
    expect(commands.apply).toHaveBeenCalledWith("prepared-1")
    await waitFor(() => expect(fix).toHaveFocus())

    fireEvent.click(screen.getByRole("button", { name: "Copy fix prompt" }))
    await waitFor(() =>
      expect(commands.writeClipboardText).toHaveBeenCalledWith("Backend prompt"),
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

  it("reviews and applies all selected automatic fixes", async () => {
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
    setup(targets, false, aggregate, report)

    const fix = await screen.findByRole("button", { name: "Fix" })
    const prompt = screen.getByRole("button", { name: "Copy fix prompt" })
    expect(fix.parentElement).not.toHaveClass("mt-3")
    expect(fix.parentElement?.parentElement).toBe(prompt.parentElement)
    fireEvent.click(fix)
    let dialog = await screen.findByRole("dialog", { name: "Choose changes" })
    expect(within(dialog).getByRole("button", { name: "Review 0 changes" })).toBeDisabled()
    fireEvent.click(within(dialog).getByRole("button", { name: "Select all" }))
    fireEvent.click(within(dialog).getByRole("button", { name: "Review 3 changes" }))

    dialog = await screen.findByRole("dialog", { name: "Review 3 changes" })
    expect(within(dialog).getByText("model-0")).toBeVisible()
    expect(within(dialog).getByText("model-1")).toBeVisible()
    expect(within(dialog).getByText("model-2")).toBeVisible()
    fireEvent.click(within(dialog).getByRole("button", { name: "Apply 3 changes" }))

    dialog = await screen.findByRole("dialog", { name: "Changes applied" })
    expect(within(dialog).getByRole("status")).toHaveTextContent("3 changes applied")
    expect(commands.apply).toHaveBeenCalledTimes(3)
  })

  it("applies only the selected automatic fixes", async () => {
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
    let dialog = await screen.findByRole("dialog", { name: "Choose changes" })
    fireEvent.click(within(dialog).getByRole("checkbox", { name: /model-1/ }))
    fireEvent.click(within(dialog).getByRole("button", { name: "Review 1 change" }))
    dialog = await screen.findByRole("dialog", { name: "Review 1 change" })
    expect(within(dialog).getByText("model-1")).toBeVisible()
    expect(within(dialog).queryByText("model-0")).not.toBeInTheDocument()
    fireEvent.click(within(dialog).getByRole("button", { name: "Apply 1 change" }))

    await screen.findByRole("dialog", { name: "Changes applied" })
    expect(commands.prepare).toHaveBeenCalledTimes(2)
    expect(commands.prepare).toHaveBeenNthCalledWith(1, "action-1")
    expect(commands.prepare).toHaveBeenNthCalledWith(2, "action-1")
    expect(commands.apply).toHaveBeenCalledOnce()
  })

  it("names resource targets in the automatic fix chooser", async () => {
    const targets = ["ReportFindings", "Workflow"].map((name, index) => ({
      ...target,
      findingId: `finding-${index}`,
      actionId: `action-${index}`,
      display: {
        ...target.display,
        resourceKind: "builtInTool" as const,
        resourceIdentity: name,
        currentValue: null,
        replacementValue: null,
        scopeKind: "project" as const,
      },
    }))
    setup(targets, false, aggregate, report)

    fireEvent.click(await screen.findByRole("button", { name: "Fix" }))
    const chooser = await screen.findByRole("dialog", { name: "Choose changes" })

    expect(
      within(chooser).getByText("Select optional built-in tools to disable."),
    ).toBeVisible()
    expect(within(chooser).getByText("Claude Code · Project configuration")).toBeVisible()
    expect(within(chooser).getByRole("checkbox", { name: "ReportFindings" })).toBeVisible()
    expect(within(chooser).getByRole("checkbox", { name: "Workflow" })).toBeVisible()
    expect(within(chooser).getAllByText(/built-in tools to disable/i)).toHaveLength(1)
  })

  it("restores named target actions after their brief success state", async () => {
    setup()

    const copy = await screen.findByRole("button", { name: "Copy fix prompt" })
    vi.useFakeTimers()
    fireEvent.click(copy)
    await act(async () => undefined)
    expect(screen.getByRole("button", { name: "Copied" })).toBeDisabled()
    fireEvent.click(screen.getByRole("button", { name: "Fix" }))
    await act(async () => undefined)
    const dialog = screen.getByRole("dialog", { name: "Review change" })
    fireEvent.click(within(dialog).getByRole("button", { name: "Apply change" }))
    await act(async () => undefined)
    expect(screen.getByRole("button", { name: "Change applied" })).toBeDisabled()

    await act(async () => vi.advanceTimersByTime(3_000))
    expect(screen.getByRole("button", { name: "Copy fix prompt" })).toBeEnabled()
    expect(screen.getByRole("button", { name: "Fix" })).toBeEnabled()
    expect(commands.copy).toHaveBeenCalledOnce()
    expect(commands.apply).toHaveBeenCalledOnce()
  })

  it("restores a check-level prompt action after its brief success state", async () => {
    render(<CheckPromptAction detector="oldModelUsage" targets={[target]} refresh={vi.fn()} />)

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
    const dialog = await screen.findByRole("dialog", { name: "Review change" })
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
          selectorLabel: "model",
          currentValue: "claude-opus-4-6",
          proposedValue: "claude-sonnet-5",
          effect: "modelSelection",
          sideEffect: "modelBehaviorMayChange",
          behaviorOverrideWarning: false,
        },
      })
    })

    expect(screen.getByRole("dialog", { name: "Review change" })).toBeVisible()
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
          selectorLabel: "model",
          currentValue: "claude-opus-4-6",
          proposedValue: "claude-sonnet-5",
          effect: "modelSelection",
          sideEffect: "modelBehaviorMayChange",
          behaviorOverrideWarning: false,
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

    expect(commands.writeClipboardText).not.toHaveBeenCalled()
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
    const dialog = await screen.findByRole("dialog", { name: "Review change" })
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
        effect: "reasoningEffort",
        sideEffect: "responsesMayUseLessReasoning",
      },
    })
    setup()
    fireEvent.click(await screen.findByRole("button", { name: "Fix" }))
    const dialog = await screen.findByRole("dialog")
    expect(dialog).toHaveTextContent("Future responses can use less reasoning")
  })

  it.each([
    ["model", "modelSelection", "modelBehaviorMayChange", "Model", "Responses can change"],
    [
      "reasoning",
      "reasoningEffort",
      "responsesMayUseLessReasoning",
      "Reasoning effort",
      "Future responses can use less reasoning",
    ],
    [
      "compaction",
      "sessionCompaction",
      "earlierSessionSummarization",
      "Compaction",
      "Future sessions can summarize earlier",
    ],
    [
      "subagentModel",
      "workerModelSelection",
      "workerBehaviorMayChange",
      "Subagent model",
      "Future worker responses can change",
    ],
    [
      "mcpServer",
      "mcpAvailability",
      "serverWillNotBeAvailable",
      "MCP server",
      "This server will not be available",
    ],
    [
      "builtInTool",
      "toolAvailability",
      "toolWillNotBeAvailable",
      "Built-in tool",
      "This built-in tool will not be available",
    ],
    [
      "skill",
      "skillAvailability",
      "skillWillNotBeAvailable",
      "Skill",
      "This skill will not be available",
    ],
    [
      "fastMode",
      "serviceTierSelection",
      "responsesMayTakeLonger",
      "Fast mode",
      "Future responses can take longer",
    ],
  ] as const)(
    "renders the typed %s review without a presentation fallback",
    async (setting, effect, sideEffect, settingLabel, sideEffectText) => {
      commands.prepare.mockResolvedValueOnce({
        outcome: "reviewReady",
        review: {
          preparedOperationId: `prepared-${setting}`,
          expiresAtEpoch: 100,
          agent: "claude-code",
          scope: "project",
          setting,
          configFile: "~/.agent/config",
          selectorLabel: `${setting}.reviewed`,
          currentValue: "reviewed=true",
          proposedValue: "reviewed=false",
          behaviorOverrideWarning: false,
          effect,
          sideEffect,
        },
      })
      setup()

      fireEvent.click(await screen.findByRole("button", { name: "Fix" }))

      const dialog = await screen.findByRole("dialog")
      expect(dialog).toHaveTextContent(`Claude Code · ${settingLabel} · Project configuration`)
      expect(dialog).toHaveTextContent(`${setting}.reviewed`)
      expect(dialog).toHaveTextContent(sideEffectText)
    },
  )

  it("shows a named resource only when the backend supplies one", async () => {
    commands.prepare.mockResolvedValueOnce({
      outcome: "reviewReady",
      review: {
        preparedOperationId: "prepared-mcp",
        expiresAtEpoch: 100,
        agent: "claude-code",
        scope: "global",
        setting: "mcpServer",
        configFile: "~/.claude/settings.json",
        selectorLabel: "mcp_servers.reviewed.enabled",
        currentValue: "reviewed=true",
        proposedValue: "reviewed=false",
        behaviorOverrideWarning: false,
        effect: "mcpAvailability",
        sideEffect: "serverWillNotBeAvailable",
      },
    })
    setup({ ...target, display: { ...target.display, resourceIdentity: null } })

    fireEvent.click(await screen.findByRole("button", { name: "Fix" }))

    expect(await screen.findByRole("dialog", { name: "Review change" })).toHaveTextContent(
      "mcp_servers.reviewed.enabled",
    )
  })

  it("retries clipboard failure without creating a second backend action", async () => {
    commands.writeClipboardText
      .mockRejectedValueOnce(new Error("Denied"))
      .mockResolvedValueOnce(undefined)
    render(<CheckPromptAction detector="oldModelUsage" targets={[target]} refresh={vi.fn()} />)
    const copy = await screen.findByRole("button", { name: "Copy fix prompt" })
    fireEvent.click(copy)
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not copy")
    fireEvent.click(copy)
    await screen.findByRole("button", { name: "Copied" })
    expect(commands.copyBatch).toHaveBeenCalledOnce()
    expect(commands.writeClipboardText).toHaveBeenCalledTimes(2)
    expect(screen.queryByText(/clipboard access/i)).not.toBeInTheDocument()
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
    const dialog = await screen.findByRole("dialog", { name: "Review change" })
    fireEvent.click(within(dialog).getByRole("button", { name: "Apply change" }))

    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      "Could not confirm the result. Check the setting before you try again.",
    )
    expect(screen.getByRole("dialog", { name: "Review change" })).toBeVisible()
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
    fireEvent.click(
      await screen.findByRole("button", { name: "Open sample session Update model" }),
    )
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
    const sample = await screen.findByRole("button", {
      name: "Open sample session Update model",
    })

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
    render(
      <BurnCheckDetail detector="unusedMcpServers" targets={[]} refresh={vi.fn()} contained />,
    )

    const emptyState = await screen.findByText("Some MCP servers were loaded but not used.")
    expect(emptyState).toBeVisible()
    expect(emptyState.closest("article")).toHaveClass(
      "rounded-control",
      "bg-surface-card/75",
      "p-4",
    )
    expect(
      screen.queryByText(/bounded view|exact target|nothing safe/i),
    ).not.toBeInTheDocument()
    expect(screen.queryByText(/Auto Fix unavailable/i)).not.toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "Fix" })).not.toBeInTheDocument()

    fireEvent.click(screen.getByRole("button", { name: "Copy fix prompt" }))

    await waitFor(() =>
      expect(commands.writeClipboardText).toHaveBeenCalledWith(
        "Inspect representative evidence.",
      ),
    )
    expect(commands.copyFallback).toHaveBeenCalledWith("unusedMcpServers")
    const copied = screen.getByRole("button", { name: "Copied" })
    expect(copied).toBeDisabled()
    expect(copied).not.toHaveClass("text-token-in")
    expect(copied.querySelector(".lucide-check")).toHaveClass("text-token-in")
  })

  it("does not use a fallback prompt when exact targets are unavailable", async () => {
    setup(
      {
        ...target,
        promptFix: { status: "unavailable", reason: "unsupportedSourceFormat" },
      },
      false,
      aggregate,
      report,
    )

    await screen.findByText("Some sessions used an older model when a newer one was available.")
    expect(screen.queryByRole("button", { name: "Copy fix prompt" })).not.toBeInTheDocument()
    expect(commands.copyFallback).not.toHaveBeenCalled()
    expect(commands.copyBatch).not.toHaveBeenCalled()
  })

  it("shows a retryable fallback prompt error", async () => {
    commands.copyFallback.mockRejectedValueOnce(new Error("Private backend error"))
    render(
      <BurnCheckDetail detector="unusedMcpServers" targets={[]} refresh={vi.fn()} contained />,
    )

    fireEvent.click(await screen.findByRole("button", { name: "Copy fix prompt" }))

    expect(await screen.findByRole("alert")).toHaveTextContent("Could not prepare the prompt")
    expect(screen.getByRole("button", { name: "Copy fix prompt" })).toBeEnabled()
    expect(commands.writeClipboardText).not.toHaveBeenCalled()
    expect(JSON.stringify(commands.noteInteraction.mock.calls)).not.toContain(
      "Private backend error",
    )
  })

  it("ignores a fallback prompt that completes after an exact target appears", async () => {
    const pending = deferred<{ outcome: "promptReady"; prompt: string } | null>()
    commands.copyFallback.mockReturnValueOnce(pending.promise)
    const view = render(
      <CheckPromptAction
        key="fallback"
        detector="unusedMcpServers"
        targets={[]}
        refresh={vi.fn()}
      />,
    )
    fireEvent.click(await screen.findByRole("button", { name: "Copy fix prompt" }))
    view.rerender(
      <CheckPromptAction
        key="exact"
        detector="unusedMcpServers"
        targets={[target]}
        refresh={vi.fn()}
      />,
    )
    await act(async () => pending.resolve({ outcome: "promptReady", prompt: "Stale prompt" }))

    expect(commands.writeClipboardText).not.toHaveBeenCalled()
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
    const dialog = await screen.findByRole("dialog", { name: "Review change" })
    fireEvent.click(within(dialog).getByRole("button", { name: "Apply change" }))

    expect(within(dialog).getByRole("button", { name: "Cancel" })).toBeDisabled()
    fireEvent.keyDown(dialog, { key: "Escape" })
    fireEvent.mouseDown(dialog.parentElement!)
    expect(screen.getByRole("dialog", { name: "Review change" })).toHaveAttribute(
      "aria-busy",
      "true",
    )
    expect(commands.apply).toHaveBeenCalledOnce()

    resolveApply({ outcome: "appliedAwaitingVerification", watchId: "watch-1" })
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument())
  })

  it("traps keyboard focus in the review and restores it after Escape", async () => {
    setup()
    const fix = await screen.findByRole("button", { name: "Fix" })
    fireEvent.click(fix)
    const dialog = await screen.findByRole("dialog", { name: "Review change" })
    const cancel = within(dialog).getByRole("button", { name: "Cancel" })
    const apply = within(dialog).getByRole("button", { name: "Apply change" })

    expect(dialog).toHaveAttribute("aria-modal", "true")
    expect(cancel).toHaveFocus()
    apply.focus()
    fireEvent.keyDown(dialog, { key: "Tab" })
    expect(cancel).toHaveFocus()
    fireEvent.keyDown(dialog, { key: "Escape" })
    await waitFor(() => expect(fix).toHaveFocus())
  })

  it("keeps review semantics usable with reduced motion, narrow width, and both themes", async () => {
    for (const theme of ["light", "dark"]) {
      document.documentElement.dataset.theme = theme
      setup()
      fireEvent.click(await screen.findByRole("button", { name: "Fix" }))
      const dialog = await screen.findByRole("dialog", { name: "Review change" })

      expect(dialog).toHaveClass("bg-surface-card", "text-label")
      expect(dialog.querySelector("dl")).toBeNull()
      expect(dialog).toHaveTextContent("~/.claude/settings.json · model")
      expect(within(dialog).getByRole("button", { name: "Cancel" }).parentElement).toHaveClass(
        "flex-col-reverse",
        "sm:flex-row",
      )
      expect(dialog.querySelector("[class*='animate-']")).toBeNull()

      fireEvent.keyDown(dialog, { key: "Escape" })
      await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument())
    }
    delete document.documentElement.dataset.theme
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
      const dialog = await screen.findByRole("dialog", { name: "Review change" })
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
    const prompt = screen.getByRole("button", { name: "Copy fix prompt" })

    expect(finding.compareDocumentPosition(fix) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0)
    expect(finding.compareDocumentPosition(prompt) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(
      0,
    )
    expect(fix.parentElement?.parentElement).toBe(prompt.parentElement)
    expect(fix.parentElement?.parentElement).toHaveClass("items-start")
    expect(screen.queryByRole("heading", { name: "claude-opus-4-6" })).not.toBeInTheDocument()
    expect(screen.queryByText(target.finding.observation)).not.toBeInTheDocument()
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
