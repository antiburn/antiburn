import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { useSyncExternalStore } from "react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type {
  AggregateWinsPayload,
  ApplyPreparedBurnCheckOperationOutcome,
  BurnCheckTargetPayload,
  ChecksReportPayload,
  CopyPromptFixBurnCheckOutcome,
  PrepareAutoFixBurnCheckTargetOutcome,
} from "../../lib/insightsIpc"
import type * as ClipboardModule from "../../lib/clipboard"
import type * as InsightsIpcModule from "../../lib/insightsIpc"
import type * as IpcModule from "../../lib/ipc"
import * as SnoozedBurnChecks from "../../lib/snoozedBurnChecks"
import { BurnChecksSession, type BurnChecksAdapter } from "./BurnChecksSession"
import { BurnChecksView } from "./BurnChecksView"
import { BurnCheckDetail, CheckPromptAction } from "./burn-checks/BurnCheckDetail"
import { searchApp } from "../../lib/appSearch"

const commands = vi.hoisted(() => ({
  prepare: vi.fn(),
  apply: vi.fn(),
  copy: vi.fn(),
  copyFallback: vi.fn(),
  copyBatch: vi.fn(),
  writeClipboardText: vi.fn(),
  openSample: vi.fn(),
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

const namedTargetReport: ChecksReportPayload = {
  ...report,
  categories: [
    { ...report.categories[0]!, id: "unusedMcpServers" },
    ...report.categories.slice(1),
  ],
}

const passedTargetReport: ChecksReportPayload = {
  ...report,
  categories: [
    { ...report.categories[0]!, lifecycle: "passing" },
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
    estimatedTokenBurnBasisPoints: null,
    verificationLimit: "freshEvidenceFromSameSourceAndTarget",
  },
  occurrenceCount: 1,
  affectedSessionCount: 1,
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

const aggregate: AggregateWinsPayload = {
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

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason: unknown) => void
  const promise = new Promise<T>((complete, fail) => {
    resolve = complete
    reject = fail
  })
  return { promise, resolve, reject }
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
  aggregatePayload: AggregateWinsPayload | Promise<AggregateWinsPayload> = aggregate,
  reportPayload: ChecksReportPayload | Promise<ChecksReportPayload> = namedTargetReport,
  checkSamples?: BurnCheckTargetPayload["samples"],
) {
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
  commands.writeClipboardText.mockResolvedValue(undefined)
})

afterEach(() => {
  vi.useRealTimers()
  if (innerWidth) Object.defineProperty(window, "innerWidth", innerWidth)
})

describe("BurnChecksView", () => {
  it("keeps findings and actions hidden until snoozes are ready", () => {
    const state = vi
      .spyOn(SnoozedBurnChecks, "useSnoozedBurnChecks")
      .mockReturnValue({ status: "loading", records: [] })
    const { view } = setup(target, false, aggregate, report)

    expect(screen.getByRole("region", { name: "Loading Burn checks" })).toBeVisible()
    expect(screen.queryByRole("button", { name: "Snooze" })).not.toBeInTheDocument()
    expect(screen.queryByRole("button", { name: /Old model usage/ })).not.toBeInTheDocument()

    view.unmount()
    state.mockRestore()
  })

  it("uses the backend's mixed-agent selection instead of the first target's samples", async () => {
    const first = target.samples[0]!
    const codex = {
      ...first,
      navigationHandle: "opaque-codex",
      agent: "codex",
      title: "Review with Codex",
    }
    setup(
      target,
      true,
      aggregate,
      {
        ...report,
        categories: [{ ...report.categories[0]!, finding: 7 }],
      },
      [first, codex],
    )
    await screen.findByRole("button", { name: /Review with Codex/ })
    expect(screen.getByText("7 sessions affected")).toBeVisible()
    expect(screen.queryByRole("button", { name: /Failed sessions/ })).toBeNull()
    expect(screen.getByText("Claude Code")).toBeVisible()
    expect(screen.getByText("Codex")).toBeVisible()
    fireEvent.click(screen.getByRole("button", { name: /Review with Codex/ }))
    await waitFor(() => expect(commands.openSample).toHaveBeenCalledWith("opaque-codex"))
  })

  it("keeps anchored selection and session cards visible across window resizes", async () => {
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
          lifecycle: "failing",
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

    expect(await screen.findByRole("button", { name: /Update model/ })).toBeVisible()

    setWindowWidth(1000)
    const resizedOverthinking = await screen.findByRole("button", {
      name: /Model overthinking/,
    })
    expect(resizedOverthinking).toHaveAttribute("aria-pressed", "true")
    expect(resizedOverthinking).not.toHaveAttribute("aria-expanded")
    expect(
      within(document.getElementById("burn-check-modelOverthinking-detail")!).getByRole(
        "button",
        { name: /Update model/ },
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
          lifecycle: "failing",
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
      categories: [{ ...report.categories[0]!, finding: 0, clean: 4, lifecycle: "passing" }],
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
    expect(screen.getByRole("button", { name: /Old model usage.*Passed/ })).toHaveAttribute(
      "aria-pressed",
      "true",
    )
  })

  it("moves focus to the Passed trigger before hiding a focused row", async () => {
    setup(target, false, aggregate, passedTargetReport)
    const trigger = await screen.findByRole("button", { name: "Passed checks 2" })
    const row = screen.getByRole("button", { name: /Old model usage.*Passed/ })
    row.focus()

    fireEvent.click(trigger)

    await waitFor(() => expect(trigger).toHaveFocus())
    expect(row).not.toBeVisible()
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
      categories: [{ ...report.categories[1]!, finding: 0, clean: 3, lifecycle: "passing" }],
    })
    act(() => session.refresh())

    expect(
      await screen.findByRole("button", { name: /Unused skills.*Passed/ }),
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
          lifecycle: "failing",
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

  it("shows finding actions beside the title and keeps the explanation full width", async () => {
    setup(target, false, aggregate, report)
    const action = await screen.findByRole("button", { name: "Copy fix prompt" })
    const description = screen.getByText(
      "Some sessions used an older model when a newer one was available.",
    )
    const snooze = screen.getByRole("button", { name: "Snooze" })
    const fix = screen.getByRole("button", { name: "Fix" })
    const heading = screen.getByRole("heading", { name: "Old model usage", level: 2 })
    const titleRow = heading.parentElement
    const failedCount = within(action.closest("header")!).getByText("1 failed")
    expect(action).toBeEnabled()
    expect(description).toHaveClass("w-full")
    expect(titleRow).toContainElement(action)
    expect(titleRow).not.toContainElement(description)
    expect(failedCount.parentElement?.parentElement).not.toHaveClass("-mt-2")
    expect(
      action.closest(".burn-check-detail")?.querySelector(".burn-checks-detail-content"),
    ).toHaveClass("pt-[var(--space-lg)]")
    expect(action.closest("header")).toHaveTextContent(
      "Some sessions used an older model when a newer one was available.",
    )
    expect(
      screen.getAllByText("Some sessions used an older model when a newer one was available."),
    ).toHaveLength(1)
    expect(snooze.compareDocumentPosition(fix) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0)
    expect(fix.compareDocumentPosition(action) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0)
    fireEvent.click(action)
    await waitFor(() => expect(commands.copyBatch).toHaveBeenCalledWith(["action-fresh"]))
    expect(commands.writeClipboardText).toHaveBeenCalledWith("Batch backend prompt")
    expect(screen.getByRole("region", { name: "Burn check details" })).toHaveTextContent(
      "1 failed·2 passed",
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

  describe("check state labels", () => {
    const watchingTarget: BurnCheckTargetPayload = {
      ...target,
      watch: {
        watchId: "watch-1",
        origin: "action",
        lifecycle: "watching",
        verification: { status: "watching" },
        savings: { status: "pending" },
      },
    }
    const awaitingReport: ChecksReportPayload = {
      ...report,
      categories: report.categories.map((check) =>
        check.id === "oldModelUsage" ? { ...check, lifecycle: "awaitingVerification" } : check,
      ),
    }

    function mockSnoozes(initial: readonly SnoozedBurnChecks.SnoozedBurnCheck[]) {
      let records = initial
      let snapshot = { status: "ready" as const, records }
      const listeners = new Set<() => void>()
      const subscribe = (listener: () => void) => {
        listeners.add(listener)
        return () => {
          listeners.delete(listener)
        }
      }
      const getSnapshot = () => snapshot
      vi.spyOn(SnoozedBurnChecks, "useSnoozedBurnChecks").mockImplementation(
        function useSnoozedChecksSnapshot() {
          return useSyncExternalStore(subscribe, getSnapshot)
        },
      )
      return (next: readonly SnoozedBurnChecks.SnoozedBurnCheck[]) => {
        records = next
        snapshot = { status: "ready", records }
        for (const listener of listeners) listener()
      }
    }

    afterEach(() => vi.restoreAllMocks())

    it.each([false, true])(
      "search targets an awaiting check once with snoozed=%s",
      async (snoozed) => {
        HTMLElement.prototype.scrollIntoView = vi.fn()
        mockSnoozes(snoozed ? [{ detector: "oldModelUsage", scope: "check", until: null }] : [])
        const { session, view } = setup(watchingTarget, false, aggregate, awaitingReport)
        await waitFor(() => expect(session.getSnapshot().report).not.toBeNull())
        view.rerender(
          <BurnChecksView
            active
            session={session}
            focusedCheck="oldModelUsage"
            focusRevision={1}
          />,
        )
        const rows = screen.getAllByRole("button", { name: /Old model usage/ })
        expect(rows).toHaveLength(1)
        expect(rows[0]).toHaveAttribute("aria-pressed", "true")
        expect(rows[0]).toHaveFocus()
        expect(
          screen.queryByText("This check has not been assessed for the available sessions."),
        ).not.toBeInTheDocument()
        if (snoozed)
          expect(screen.getByRole("button", { name: "Snoozed 1" })).toHaveAttribute(
            "aria-expanded",
            "true",
          )
      },
    )

    it("keeps a zero-count awaiting search result assessed", async () => {
      HTMLElement.prototype.scrollIntoView = vi.fn()
      const { session, view } = setup(watchingTarget, false, aggregate, {
        ...awaitingReport,
        categories: awaitingReport.categories.map((check) =>
          check.id === "oldModelUsage" ? { ...check, finding: 0, clean: 0 } : check,
        ),
      })
      await screen.findByRole("button", { name: /Old model usage/ })
      view.rerender(
        <BurnChecksView
          active
          session={session}
          focusedCheck="oldModelUsage"
          focusRevision={1}
        />,
      )
      const row = screen.getByRole("button", { name: /Old model usage, Awaiting verification/ })
      expect(row).toHaveAttribute("aria-pressed", "true")
      expect(row).toHaveFocus()
      expect(screen.queryByText("Not assessed")).not.toBeInTheDocument()
      expect(
        screen.queryByText("This check has not been assessed for the available sessions."),
      ).not.toBeInTheDocument()
    })

    it("shows awaiting from the report before target details load", async () => {
      const pending = deferred<BurnCheckTargetPayload[]>()
      setup(pending.promise, false, aggregate, awaitingReport)
      const heading = await screen.findByRole("heading", { name: "Old model usage", level: 2 })
      const header = heading.closest("header")!
      const badge = await within(header).findByText("Awaiting verification")
      expect(badge.closest("button")).toBeNull()
      expect(header).toHaveTextContent("1 failed")
      expect(header).toHaveTextContent("2 passed")
      const groupHeading = screen.getByRole("heading", { name: "Awaiting verification 1" })
      expect(groupHeading.querySelector("svg")).toHaveAttribute("aria-hidden", "true")
      expect(
        within(groupHeading.closest("section")!).getByRole("button", {
          name: /Old model usage/,
        }),
      ).toHaveAttribute("aria-pressed", "true")
      expect(
        screen.queryByText("A later complete session confirms each change."),
      ).not.toBeInTheDocument()
      await act(async () => pending.resolve([watchingTarget]))
    })

    it.each([
      { name: "ordinary findings", targets: [target] },
      { name: "empty target details", targets: [] },
      { name: "mixed verification states", targets: [watchingTarget, target] },
    ])("does not show awaiting for $name", async ({ targets }) => {
      const { session } = setup(targets, false, aggregate, report)
      await waitFor(() =>
        expect(session.getSnapshot().targets.oldModelUsage?.data).toBeDefined(),
      )
      const header = screen
        .getByRole("heading", { name: "Old model usage", level: 2 })
        .closest("header")!
      expect(within(header).queryByText("Awaiting verification")).not.toBeInTheDocument()
      expect(
        screen.queryByRole("heading", { name: /Awaiting verification/ }),
      ).not.toBeInTheDocument()
    })

    it.each([null, new Date("2026-09-25T12:00:00Z").getTime()])(
      "shows snooze %s before awaiting and restores awaiting after unsnooze",
      async (until) => {
        const setSnoozes = mockSnoozes([{ detector: "oldModelUsage", scope: "check", until }])
        const unsnooze = vi.spyOn(SnoozedBurnChecks, "unsnoozeBurnCheck").mockResolvedValue()
        const { session } = setup(watchingTarget, false, aggregate, awaitingReport)
        fireEvent.click(await screen.findByRole("button", { name: "Snoozed 1" }))
        fireEvent.click(screen.getByRole("button", { name: /Old model usage/ }))
        await waitFor(() =>
          expect(session.getSnapshot().targets.oldModelUsage?.data).toBeDefined(),
        )
        const heading = screen.getByRole("heading", { name: "Old model usage", level: 2 })
        const header = heading.closest("header")!
        const label = SnoozedBurnChecks.formatSnoozeUntil(until)
        expect(within(header).getByText(label)).toBeInTheDocument()
        expect(within(header).queryByText("Awaiting verification")).not.toBeInTheDocument()
        expect(
          screen.queryByRole("heading", { name: /Awaiting verification/ }),
        ).not.toBeInTheDocument()
        fireEvent.click(within(header).getByRole("button", { name: "Unsnooze" }))
        expect(unsnooze).toHaveBeenCalledWith("oldModelUsage")

        act(() => setSnoozes([]))

        expect(await within(header).findByText("Awaiting verification")).toBeInTheDocument()
        expect(within(header).queryByText(label)).not.toBeInTheDocument()
        expect(within(header).getByRole("button", { name: "Snooze" })).toBeInTheDocument()
        expect(screen.getByRole("button", { name: /Old model usage/ })).toHaveAttribute(
          "aria-pressed",
          "true",
        )
      },
    )

    it("removes the header state when an ordinary finding's snooze expires", async () => {
      const until = new Date("2026-09-25T12:00:00Z").getTime()
      const setSnoozes = mockSnoozes([{ detector: "oldModelUsage", scope: "check", until }])
      setup(target, false, aggregate, report)
      fireEvent.click(await screen.findByRole("button", { name: "Snoozed 1" }))
      fireEvent.click(screen.getByRole("button", { name: /Old model usage/ }))
      const header = screen
        .getByRole("heading", { name: "Old model usage", level: 2 })
        .closest("header")!
      const label = SnoozedBurnChecks.formatSnoozeUntil(until)
      expect(within(header).getByText(label)).toBeInTheDocument()

      act(() => setSnoozes([]))

      expect(within(header).queryByText(label)).not.toBeInTheDocument()
      expect(within(header).queryByText("Awaiting verification")).not.toBeInTheDocument()
      expect(screen.getByRole("button", { name: /Old model usage/ })).toHaveAttribute(
        "aria-pressed",
        "true",
      )
    })

    it("keeps running checks visible when every assessed check is snoozed", async () => {
      mockSnoozes([
        { detector: "oldModelUsage", scope: "check", until: null },
        { detector: "unusedSkills", scope: "check", until: null },
      ])
      setup(target, false, aggregate, report)

      expect(await screen.findByRole("button", { name: "Snoozed 2" })).toBeVisible()
      expect(screen.queryByRole("button", { name: /Old model usage/ })).not.toBeInTheDocument()
      expect(screen.queryByRole("button", { name: /Unused skills/ })).not.toBeInTheDocument()
      expect(screen.queryByRole("heading", { name: "Failed checks 1" })).not.toBeInTheDocument()
      expect(screen.queryByRole("region", { name: "Savings" })).not.toBeInTheDocument()
      expect(screen.queryByText("No active checks.")).not.toBeInTheDocument()
    })

    it("moves focus to the Snoozed trigger before hiding a focused row", async () => {
      mockSnoozes([{ detector: "oldModelUsage", scope: "check", until: null }])
      setup(target, false, aggregate, report)
      const trigger = await screen.findByRole("button", { name: "Snoozed 1" })
      fireEvent.click(trigger)
      const row = screen.getByRole("button", { name: /Old model usage/ })
      row.focus()

      fireEvent.click(trigger)

      await waitFor(() => expect(trigger).toHaveFocus())
      expect(row).not.toBeVisible()
    })

    it("moves focus to the collapsed Snoozed trigger when a focused row is regrouped", async () => {
      const setSnoozes = mockSnoozes([])
      setup(target, false, aggregate, report)
      const row = await screen.findByRole("button", { name: /Old model usage/ })
      row.focus()

      act(() => setSnoozes([{ detector: "oldModelUsage", scope: "check", until: null }]))

      const trigger = screen.getByRole("button", { name: "Snoozed 1" })
      await waitFor(() => expect(trigger).toHaveFocus())
      expect(row).not.toBeInTheDocument()
    })

    it("keeps ordinary passed checks free of state badges", async () => {
      setup(target, false, aggregate, report)
      fireEvent.click(await screen.findByRole("button", { name: "Passed checks 1" }))
      fireEvent.click(screen.getByRole("button", { name: /Unused skills/ }))
      const header = screen
        .getByRole("heading", { name: "Unused skills", level: 2 })
        .closest("header")!
      expect(within(header).queryByText("Awaiting verification")).not.toBeInTheDocument()
      expect(within(header).queryByText(/Snoozed/)).not.toBeInTheDocument()
    })
  })

  it("moves focus to the collapsed Passed trigger when a focused row starts passing", async () => {
    const secondFailure = {
      id: "modelOverthinking" as const,
      finding: 2,
      clean: 1,
      unavailable: 0,
      estimatedTokenBurnBasisPoints: 400,
      lifecycle: "failing" as const,
    }
    const twoFailures = { ...report, categories: [report.categories[0]!, secondFailure] }
    const { adapter, session } = setup(target, false, aggregate, twoFailures)
    const row = await screen.findByRole("button", { name: /Model overthinking/ })
    row.focus()
    vi.mocked(adapter.getReport).mockResolvedValue({
      ...twoFailures,
      categories: [
        report.categories[0]!,
        { ...secondFailure, finding: 0, clean: 3, lifecycle: "passing" },
      ],
    })

    act(() => session.refresh())

    const trigger = await screen.findByRole("button", { name: "Passed checks 1" })
    await waitFor(() => expect(trigger).toHaveFocus())
    expect(row).not.toBeInTheDocument()
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

  it("keeps group counts and the period without an assessment info control", async () => {
    setup(target, false, aggregate, report)
    expect(await screen.findByRole("heading", { name: "Failed checks 1" })).toBeVisible()
    expect(screen.getByRole("button", { name: "Passed checks 1" })).toBeVisible()
    expect(screen.getByText("30 days")).toBeVisible()
    expect(screen.queryByRole("button", { name: "Assessment details" })).not.toBeInTheDocument()
    expect(screen.queryByText("Coverage details")).not.toBeInTheDocument()
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
            lifecycle: "passing",
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
    expect(await screen.findByText("1 session affected")).toBeVisible()
    const row = await screen.findByRole("button", {
      name: /Old model usage.*8% estimated burn/,
    })
    expect(row).toBeVisible()
    expect(within(row).getByText("8% estimated burn")).toBeVisible()
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
    expect(screen.queryByRole("region", { name: "Your savings" })).not.toBeInTheDocument()
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
    expect(selected.querySelector(".burn-check-category-icon")).toHaveClass("text-check-mcp")
    expect(selected.closest(".burn-checks-group-body")).toBeInTheDocument()
  })

  it("shows the named resource count in the outcome line", async () => {
    setWindowWidth(1400)
    const first = setup(target, true)

    expect(await screen.findByText(/At least 1 affected MCP server$/)).toBeVisible()
    first.view.unmount()

    setup(target)
    expect(await screen.findByText(/1 affected MCP server$/)).toBeVisible()
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

    const resources = await screen.findByText(/At least 2 affected MCP servers$/)
    expect(resources).toHaveTextContent("At least 2 affected MCP servers")
    expect(screen.queryByText("23 sessions affected")).not.toBeInTheDocument()
    expect(screen.getByRole("region", { name: "Burn check details" })).toHaveTextContent(
      "23 failed",
    )
  })

  it("shows session cards with authoritative impact and project identity", async () => {
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
    await screen.findAllByRole("button", { name: /Update model/ })
    expect(screen.queryByRole("button", { name: /Failed sessions/ })).toBeNull()
    expect(screen.getByText("12 sessions affected")).toBeVisible()
    expect(screen.getByText("3 sessions affected")).toBeVisible()
    expect(screen.getByText(/· antiburn/)).toBeVisible()
    expect(screen.getByText(/· browser-tests/)).toBeVisible()
    expect(screen.getAllByRole("button", { name: /Update model/ })).toHaveLength(2)
  })

  it("shows multiple session cards without a disclosure", async () => {
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
      { ...report, categories: [{ ...report.categories[0]!, finding: 2 }] },
    )

    expect(await screen.findByRole("button", { name: /Update model/ })).toBeVisible()
    expect(screen.queryByRole("button", { name: /Failed sessions/ })).toBeNull()
    expect(screen.getByRole("button", { name: /Review model/ })).toBeVisible()
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
          lifecycle: "failing",
        },
        {
          id: "modelOverthinking",
          finding: 1,
          clean: 0,
          unavailable: 0,
          estimatedTokenBurnBasisPoints: 350,
          lifecycle: "failing",
        },
        {
          id: "overpoweredSubagents",
          finding: 1,
          clean: 0,
          unavailable: 0,
          estimatedTokenBurnBasisPoints: 880,
          lifecycle: "failing",
        },
        {
          id: "unusedMcpServers",
          finding: 1,
          clean: 0,
          unavailable: 0,
          estimatedTokenBurnBasisPoints: 100,
          lifecycle: "failing",
        },
        {
          id: "unusedBuiltInTools",
          finding: 1,
          clean: 0,
          unavailable: 0,
          estimatedTokenBurnBasisPoints: 1,
          lifecycle: "failing",
        },
        {
          id: "unusedSkills",
          finding: 1,
          clean: 0,
          unavailable: 0,
          estimatedTokenBurnBasisPoints: 100,
          lifecycle: "failing",
        },
        {
          id: "oldModelUsage",
          finding: 1,
          clean: 0,
          unavailable: 0,
          estimatedTokenBurnBasisPoints: 400,
          lifecycle: "failing",
        },
        {
          id: "overuseOfFastMode",
          finding: 1,
          clean: 0,
          unavailable: 0,
          estimatedTokenBurnBasisPoints: 333,
          lifecycle: "failing",
        },
        {
          id: "cacheChurn",
          finding: 1,
          clean: 0,
          unavailable: 0,
          estimatedTokenBurnBasisPoints: 700,
          lifecycle: "failing",
        },
      ],
    }
    setup(target, false, aggregate, allFailures)

    for (const metric of [
      "8% estimated burn",
      "3% estimated burn",
      "8% estimated burn",
      "1% estimated burn",
      "Under 1% estimated burn",
      "1% estimated burn",
      "4% estimated burn",
      "3% estimated burn",
      "7% estimated burn",
    ]) {
      expect(
        (await screen.findAllByRole("button", { name: new RegExp(metric) })).length,
      ).toBeGreaterThan(0)
    }
  })

  it("shows the exact agent mark and configuration scope", async () => {
    setup()

    expect(await screen.findAllByRole("img", { name: "Claude Code" })).toHaveLength(1)
    expect(screen.getByText("Global configuration")).toBeVisible()
  })

  it("selects and refocuses a searched check without remounting the report", async () => {
    HTMLElement.prototype.scrollIntoView = vi.fn()
    const { session, view } = setup()
    await screen.findByRole("button", { name: /Unused MCP servers/ })
    view.rerender(
      <BurnChecksView
        active
        session={session}
        focusedCheck="unusedMcpServers"
        focusRevision={1}
      />,
    )
    const row = screen.getByRole("button", { name: /Unused MCP servers/ })
    expect(row).toHaveAttribute("aria-pressed", "true")
    expect(row).toHaveFocus()
    fireEvent.click(screen.getByRole("button", { name: "Passed checks 1" }))
    fireEvent.click(screen.getByRole("button", { name: /Unused skills, / }))
    expect(row).toHaveAttribute("aria-pressed", "false")
    view.rerender(
      <BurnChecksView
        active={false}
        session={session}
        focusedCheck="unusedMcpServers"
        focusRevision={1}
      />,
    )
    view.rerender(
      <BurnChecksView
        active
        session={session}
        focusedCheck="unusedMcpServers"
        focusRevision={1}
      />,
    )
    expect(row).toHaveAttribute("aria-pressed", "false")
    expect(screen.getByRole("button", { name: /Unused skills, / })).toHaveAttribute(
      "aria-pressed",
      "true",
    )
    view.rerender(
      <BurnChecksView
        active
        session={session}
        focusedCheck="unusedMcpServers"
        focusRevision={2}
      />,
    )
    expect(screen.getByRole("button", { name: /Unused MCP servers/ })).toBe(row)
    expect(row).toHaveAttribute("aria-pressed", "true")
    expect(row).toHaveFocus()
  })

  it.each(searchApp("").filter(({ target }) => target.kind === "check"))(
    "reaches $label from its search destination",
    async (result) => {
      if (result.target.kind !== "check") throw new Error("Unexpected search target")
      HTMLElement.prototype.scrollIntoView = vi.fn()
      const other =
        result.target.check === "oldModelUsage" ? "unusedMcpServers" : "oldModelUsage"
      const { session, view } = setup(target, false, aggregate, {
        ...report,
        categories: [
          { ...report.categories[0]!, id: other },
          { ...report.categories[0]!, id: result.target.check },
        ],
      })
      const row = await screen.findByRole("button", { name: new RegExp(result.label) })
      expect(searchApp(result.label)[0]?.target).toEqual(result.target)
      view.rerender(
        <BurnChecksView
          active
          session={session}
          focusedCheck={result.target.check}
          focusRevision={1}
        />,
      )
      await waitFor(() => expect(row).toHaveFocus())
      expect(row).toHaveAttribute("aria-pressed", "true")
    },
  )

  it("keeps an unassessed search destination reachable without inventing a pass", async () => {
    HTMLElement.prototype.scrollIntoView = vi.fn()
    const { session, view } = setup()
    await screen.findByRole("button", { name: /Unused MCP servers/ })
    view.rerender(
      <BurnChecksView active session={session} focusedCheck="cacheChurn" focusRevision={1} />,
    )
    expect(
      screen.getByText("This check has not been assessed for the available sessions."),
    ).toBeVisible()
    expect(screen.getByRole("button", { name: /Excess cache rehydration/ })).toHaveFocus()
    view.rerender(
      <BurnChecksView
        active={false}
        session={session}
        focusedCheck="cacheChurn"
        focusRevision={1}
      />,
    )
    view.rerender(
      <BurnChecksView active session={session} focusedCheck="cacheChurn" focusRevision={1} />,
    )
    expect(screen.getByRole("button", { name: /Excess cache rehydration/ })).toHaveAttribute(
      "aria-pressed",
      "true",
    )
    expect(
      screen.getByText("This check has not been assessed for the available sessions."),
    ).toBeVisible()
  })

  it("allows ordinary selection after searching for an absent check", async () => {
    HTMLElement.prototype.scrollIntoView = vi.fn()
    const missing = {
      ...report,
      categories: report.categories.filter((check) => check.id !== "cacheChurn"),
    }
    const { session, view } = setup(target, false, aggregate, missing)
    await screen.findByRole("button", { name: /Old model usage/ })
    view.rerender(
      <BurnChecksView active session={session} focusedCheck="cacheChurn" focusRevision={1} />,
    )
    expect(
      screen.getByText("This check has not been assessed for the available sessions."),
    ).toBeVisible()
    fireEvent.click(screen.getByRole("button", { name: /Old model usage/ }))
    expect(
      screen.queryByText("This check has not been assessed for the available sessions."),
    ).not.toBeInTheDocument()
    expect(screen.getByRole("heading", { name: "Old model usage" })).toBeVisible()
  })

  it("selects a pending search target when a later report supplies it", async () => {
    HTMLElement.prototype.scrollIntoView = vi.fn()
    const missing = {
      ...report,
      categories: report.categories.filter((check) => check.id !== "cacheChurn"),
    }
    const { adapter, session, view } = setup(target, false, aggregate, missing)
    await screen.findByRole("button", { name: /Old model usage/ })
    view.rerender(
      <BurnChecksView active session={session} focusedCheck="cacheChurn" focusRevision={1} />,
    )
    vi.mocked(adapter.getReport).mockResolvedValueOnce({
      ...report,
      categories: report.categories.map((check) =>
        check.id === "cacheChurn"
          ? { ...check, clean: 3, unavailable: 0, lifecycle: "passing" as const }
          : check,
      ),
    })
    await act(async () => session.refresh())
    const row = await screen.findByRole("button", { name: /Excess cache rehydration/ })
    expect(row).toHaveAttribute("aria-pressed", "true")
    expect(row).toHaveFocus()
    expect(
      screen.queryByText("This check has not been assessed for the available sessions."),
    ).not.toBeInTheDocument()
  })

  it("leaves shared titlebar ownership to the layout while loading and after load", async () => {
    const userAgent = vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Macintosh")
    try {
      const pending = deferred<ChecksReportPayload>()
      const { view } = setup(target, false, aggregate, pending.promise)
      expect(view.container.querySelector("[data-tauri-drag-region]")).toBeNull()
      await act(async () => pending.resolve(report))
      await screen.findByRole("button", { name: /Old model usage.*8% estimated burn/ })
      expect(view.container.querySelectorAll("[data-tauri-drag-region]")).toHaveLength(0)
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

  it("leaves the macOS empty error state below the shared titlebar", async () => {
    const userAgent = vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Macintosh")
    try {
      const { view } = setup(target, false, aggregate, Promise.reject(new Error("Unavailable")))
      expect(await screen.findByRole("alert")).toHaveTextContent("Burn checks are unavailable.")
      expect(view.container.querySelector("[data-tauri-drag-region]")).toBeNull()
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
    vi.mocked(adapter.getTargets).mockResolvedValueOnce({
      targets: [target],
      samples: [],
      truncated: false,
    })
    fireEvent.click(screen.getByRole("button", { name: "Retry" }))

    expect(await screen.findByRole("button", { name: "Copy fix prompt" })).toBeVisible()
  })

  it("uses backend prepare, apply, and prompt commands without duplicate actions", async () => {
    const { adapter, session } = setup()
    const fix = await screen.findByRole("button", { name: "Fix" })
    vi.useFakeTimers()
    fireEvent.click(fix)
    fireEvent.click(fix)
    expect(commands.prepare).toHaveBeenCalledOnce()
    await act(async () => undefined)
    const dialog = screen.getByRole("dialog", { name: "Review change" })
    expect(dialog).toHaveTextContent("Claude Code · Model · Global configuration")
    expect(dialog).toHaveTextContent("~/.claude/settings.json · model")
    expect(dialog).toHaveTextContent("claude-opus-4-6 → claude-sonnet-5")
    expect(dialog).toHaveTextContent("Responses can change")
    expect(dialog).toHaveTextContent("previous content in a sibling .bak file")
    fireEvent.click(within(dialog).getByRole("button", { name: "Apply change" }))
    await act(async () => undefined)
    expect(screen.getByRole("button", { name: "Change applied" })).toBeDisabled()
    expect(commands.apply).toHaveBeenCalledWith("prepared-1")
    expect(fix).toHaveFocus()

    fireEvent.click(screen.getByRole("button", { name: "Copy fix prompt" }))
    await act(async () => undefined)
    expect(commands.writeClipboardText).toHaveBeenCalledWith("Batch backend prompt")
    expect(commands.copyBatch).toHaveBeenCalledWith(["action-fresh"])
    expect(commands.copy).not.toHaveBeenCalled()
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
      samples: [],
      truncated: false,
    })
    const refreshCount = vi.mocked(adapter.getTargets).mock.calls.length
    await act(async () => session.loadTargets("unusedMcpServers", true))
    expect(vi.mocked(adapter.getTargets).mock.calls.length).toBeGreaterThan(refreshCount)
    expect(session.getSnapshot().targets.unusedMcpServers?.data?.targets[0]?.actionId).toBe(
      "action-new",
    )
    expect(screen.getByRole("button", { name: "Copy fix prompt" })).toBeEnabled()
    expect(screen.getByRole("button", { name: "Change applied" })).toBeDisabled()
    expect(screen.queryByRole("button", { name: "Fix" })).not.toBeInTheDocument()

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
      samples: [],
      truncated: false,
    })
    await act(async () => session.loadTargets("unusedMcpServers", true))
    expect(session.getSnapshot().targets.unusedMcpServers?.data?.targets[0]?.actionId).toBe(
      "action-next-attempt",
    )
    expect(screen.getByRole("button", { name: "Copy fix prompt" })).toBeEnabled()
    expect(screen.getByRole("button", { name: "Fix" })).toBeEnabled()
  }, 10_000)

  it("confirms an applied change", async () => {
    commands.apply.mockResolvedValueOnce({
      outcome: "appliedAwaitingVerification",
      watchId: "watch-1",
    })
    setup()

    fireEvent.click(await screen.findByRole("button", { name: "Fix" }))
    const dialog = await screen.findByRole("dialog", { name: "Review change" })
    fireEvent.click(within(dialog).getByRole("button", { name: "Apply change" }))

    await waitFor(() =>
      expect(commands.noteInteraction).toHaveBeenCalledWith({
        kind: "burnCheckAutoFixCompleted",
        outcome: "applied_awaiting_verification",
      }),
    )
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
    expect(fix.parentElement?.parentElement).toBe(prompt.parentElement?.parentElement)
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

  it("keeps long config values readable in the batch review", async () => {
    const currentValue = "copy-value-with-a-long-config-selector-".repeat(8)
    const proposedValue = "claude-sonnet-5-replacement-value-".repeat(8)
    commands.prepare.mockResolvedValue({
      outcome: "reviewReady",
      review: {
        preparedOperationId: "prepared-long-value",
        expiresAtEpoch: 100,
        agent: "claude-code",
        scope: "project",
        setting: "model",
        configFile: "~/Sites/pickleheads/.claude/settings.local.json",
        selectorLabel: "copy-cluade-local",
        currentValue,
        proposedValue,
        effect: "modelSelection",
        sideEffect: "modelBehaviorMayChange",
      },
    })
    const targets = [0, 1].map((index) => ({
      ...target,
      findingId: `finding-long-${index}`,
      actionId: `action-long-${index}`,
      display: { ...target.display, resourceIdentity: `model-long-${index}` },
    }))
    setup(targets)

    fireEvent.click(await screen.findByRole("button", { name: "Fix" }))
    let dialog = await screen.findByRole("dialog", { name: "Choose changes" })
    fireEvent.click(within(dialog).getByRole("button", { name: "Select all" }))
    fireEvent.click(within(dialog).getByRole("button", { name: "Review 2 changes" }))

    dialog = await screen.findByRole("dialog", { name: "Review 2 changes" })
    const values = within(dialog).getAllByText(`${currentValue} → ${proposedValue}`)
    expect(values).toHaveLength(2)
    for (const value of values) {
      expect(value).toHaveClass("block", "max-w-full", "break-all")
      expect(value).toBeVisible()
    }
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

  it("restores whole-check actions after their brief success state", async () => {
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
    expect(commands.copyBatch).toHaveBeenCalledWith(["action-fresh"])
    expect(commands.copy).not.toHaveBeenCalled()
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

  it("shows a busy label while the check prompt is prepared", async () => {
    const pending = deferred<Awaited<ReturnType<typeof commands.copyBatch>>>()
    commands.copyBatch.mockReturnValueOnce(pending.promise)
    render(<CheckPromptAction detector="unusedSkills" targets={[target]} refresh={vi.fn()} />)

    fireEvent.click(await screen.findByRole("button", { name: "Copy fix prompt" }))
    expect(await screen.findByRole("button", { name: "Preparing…" })).toBeDisabled()

    pending.resolve({ outcome: "promptReady", prompt: "Batch backend prompt" })
    expect(await screen.findByRole("button", { name: "Copied" })).toBeDisabled()
    expect(commands.writeClipboardText).toHaveBeenCalledWith("Batch backend prompt")
  })

  it("keeps the click-again message after a stale id refreshes the list", async () => {
    commands.copyBatch.mockResolvedValueOnce({ outcome: "unavailable" })
    const refresh = vi.fn()
    const { rerender } = render(
      <CheckPromptAction detector="unusedSkills" targets={[target]} refresh={refresh} />,
    )

    fireEvent.click(await screen.findByRole("button", { name: "Copy fix prompt" }))
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "The list was refreshed. Click again to copy.",
    )
    expect(refresh).toHaveBeenCalledOnce()
    expect(commands.writeClipboardText).not.toHaveBeenCalled()

    // The refresh lists the check again with new ids. The message must survive that.
    rerender(
      <CheckPromptAction
        detector="unusedSkills"
        targets={[{ ...target, actionId: "action-new" }]}
        refresh={refresh}
      />,
    )
    expect(screen.getByRole("alert")).toHaveTextContent(
      "The list was refreshed. Click again to copy.",
    )
    const copy = screen.getByRole("button", { name: "Copy fix prompt" })
    expect(copy).toBeEnabled()

    fireEvent.click(copy)
    expect(await screen.findByRole("button", { name: "Copied" })).toBeDisabled()
    expect(commands.copyBatch).toHaveBeenLastCalledWith(["action-new"])
    expect(commands.writeClipboardText).toHaveBeenCalledWith("Batch backend prompt")
    expect(screen.queryByRole("alert")).not.toBeInTheDocument()
  })

  it("clears an error message when the list refreshes with new ids", async () => {
    commands.copyBatch.mockRejectedValueOnce(new Error("prepare failed"))
    const { rerender } = render(
      <CheckPromptAction detector="unusedSkills" targets={[target]} refresh={vi.fn()} />,
    )

    fireEvent.click(await screen.findByRole("button", { name: "Copy fix prompt" }))
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Could not prepare the prompt. Try again.",
    )

    // The error was about the old list. A relist with new ids drops it.
    rerender(
      <CheckPromptAction
        detector="unusedSkills"
        targets={[{ ...target, actionId: "action-new" }]}
        refresh={vi.fn()}
      />,
    )
    expect(screen.queryByRole("alert")).not.toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Copy fix prompt" })).toBeEnabled()
  })

  it("keeps an open review when the expiring action handle rotates", async () => {
    const { adapter, session } = setup()
    fireEvent.click(await screen.findByRole("button", { name: "Fix" }))
    const dialog = await screen.findByRole("dialog", { name: "Review change" })
    vi.mocked(adapter.getTargets).mockResolvedValueOnce({
      targets: [{ ...target, actionId: "action-rotated" }],
      samples: [],
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
      samples: [],
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
      samples: [],
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
    const pending = deferred<CopyPromptFixBurnCheckOutcome | null>()
    commands.copyBatch.mockReturnValueOnce(pending.promise)
    const { adapter, session } = setup()
    fireEvent.click(await screen.findByRole("button", { name: "Copy fix prompt" }))
    vi.mocked(adapter.getTargets).mockResolvedValueOnce({
      targets: [recurredTarget("action-after-recurrence")],
      samples: [],
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
      samples: [],
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
      samples: [],
      truncated: false,
    })

    session.loadTargets("unusedMcpServers", true)

    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Copy fix prompt" })).toBeEnabled(),
    )
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
    const prompt = "Batch backend prompt\n\nRemediation reference: ABR-shared-group"
    commands.copyBatch.mockResolvedValueOnce({
      outcome: "promptReady",
      prompt,
    })
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
    expect(commands.writeClipboardText).toHaveBeenNthCalledWith(1, prompt)
    expect(commands.writeClipboardText).toHaveBeenNthCalledWith(2, prompt)
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

    await waitFor(() =>
      expect(within(dialog).getByRole("alert")).toHaveTextContent(
        "Could not confirm the result. Check the setting before you try again.",
      ),
    )
    expect(screen.getByRole("dialog", { name: "Review change" })).toBeVisible()
    await waitFor(() =>
      expect(commands.noteInteraction).toHaveBeenCalledWith({
        kind: "burnCheckAutoFixCompleted",
        outcome: "failed",
      }),
    )
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
    fireEvent.click(await screen.findByRole("button", { name: /Update model/ }))
    expect(await screen.findByText("This session was deleted.")).toHaveAttribute(
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
      name: /Update model/,
    })

    fireEvent.click(sample)

    expect(sample).toHaveAttribute("aria-busy", "true")
    expect(sample).toHaveAttribute("aria-disabled", "true")
    await act(async () => opening.resolve({ outcome: "opened" }))
    await waitFor(() => expect(sample).not.toHaveAttribute("aria-disabled"))
  })

  it.each([
    [
      "recurred",
      { status: "recurred", methodRevision: 1, evidenceRevision: "e2" },
      "This finding returned.",
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
      <BurnCheckDetail
        detector="unusedMcpServers"
        targets={[]}
        samples={[]}
        failedSessionCount={0}
        refresh={vi.fn()}
        contained
      />,
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

  it("uses a fallback prompt when exact targets are unavailable", async () => {
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
    fireEvent.click(screen.getByRole("button", { name: "Copy fix prompt" }))

    await waitFor(() => expect(commands.copyFallback).toHaveBeenCalledWith("oldModelUsage"))
    expect(commands.writeClipboardText).toHaveBeenCalledWith("Inspect representative evidence.")
    expect(commands.copyBatch).not.toHaveBeenCalled()
  })

  it("shows a retryable fallback prompt error", async () => {
    commands.copyFallback.mockRejectedValueOnce(new Error("Private backend error"))
    render(
      <BurnCheckDetail
        detector="unusedMcpServers"
        targets={[]}
        samples={[]}
        failedSessionCount={0}
        refresh={vi.fn()}
        contained
      />,
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

  it("keeps one whole-check prompt when target actions are unavailable", async () => {
    setup(
      {
        ...target,
        autoFix: { status: "unavailable", reason: "safetyCheckFailed" },
        promptFix: { status: "unavailable", reason: "unsupportedSourceFormat" },
      },
      false,
      aggregate,
      namedTargetReport,
    )

    const prompt = await screen.findByRole("button", { name: "Copy fix prompt" })
    expect(prompt).toBeEnabled()
    expect(screen.getAllByRole("button", { name: "Copy fix prompt" })).toHaveLength(1)
    expect(screen.getAllByRole("button", { name: "Snooze" })).toHaveLength(1)
    expect(screen.queryByText(/write safety check/)).not.toBeInTheDocument()
    expect(
      screen.queryByText(/does not support a safe automatic change/),
    ).not.toBeInTheDocument()
    expect(screen.queryByText(/^Automatic fix:/)).not.toBeInTheDocument()
    expect(screen.queryByText(/^Prompt fix:/)).not.toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "Fix" })).not.toBeInTheDocument()

    fireEvent.click(prompt)
    await waitFor(() => expect(commands.copyBatch).toHaveBeenCalledWith(["action-fresh"]))
  })

  it.each([
    ["conflict", { outcome: "conflict" }],
    ["unavailable", { outcome: "unavailable", reason: "safetyCheckFailed" }],
  ] as const)(
    "keeps the %s apply outcome in the review with feedback",
    async (name, outcome) => {
      commands.apply.mockResolvedValueOnce(outcome)
      setup()
      fireEvent.click(await screen.findByRole("button", { name: "Fix" }))
      const dialog = await screen.findByRole("dialog", { name: "Review change" })
      fireEvent.click(within(dialog).getByRole("button", { name: "Apply change" }))

      await waitFor(() =>
        expect(within(dialog).getByRole("button", { name: "Close" })).toBeEnabled(),
      )
      expect(within(dialog).getByRole("alert")).toHaveTextContent(
        name === "conflict"
          ? "Another change now conflicts with this operation. Close this review and check the setting."
          : "The current setting no longer passes the write safety check.",
      )
      expect(within(dialog).getByRole("button", { name: "Apply change" })).toBeDisabled()
    },
  )

  it("shows check-level actions in the title row", async () => {
    setup(target, false, aggregate, report)
    const finding = await screen.findByText(
      "Some sessions used an older model when a newer one was available.",
    )
    const fix = screen.getByRole("button", { name: "Fix" })
    const prompt = screen.getByRole("button", { name: "Copy fix prompt" })

    const heading = screen.getByRole("heading", { name: "Old model usage", level: 2 })
    expect(heading.parentElement).toContainElement(fix)
    expect(heading.parentElement).toContainElement(prompt)
    expect(fix.compareDocumentPosition(finding) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0)
    expect(prompt.compareDocumentPosition(finding) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(
      0,
    )
    expect(fix.parentElement?.parentElement).toBe(prompt.parentElement?.parentElement)
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

  it("shows aggregate savings for a passed check", async () => {
    setup(
      target,
      false,
      {
        wins: [
          aggregate.wins[0]!,
          { ...aggregate.wins[0]!, verifiedBoundaryMs: 3 },
          {
            ...aggregate.wins[0]!,
            findingId: "win-2",
            display: {
              ...aggregate.wins[0]!.display,
              estimatedOpportunity: { value: 400, unit: "literalInputTokens" },
            },
            savings: {
              status: { status: "unavailable" },
              tokenSavings: null,
              apiEquivalentCostAvoidedUsd: null,
              improvementCount: null,
              method: null,
            },
          },
        ],
      },
      passedTargetReport,
    )

    const savings = await screen.findByRole("region", { name: "Savings" })
    expect(within(savings).getByText("2 verified remediation cycles")).toBeVisible()
    expect(within(savings).getAllByText("~400 input tokens projected")).toHaveLength(2)
    expect(within(savings).getAllByText("Unavailable for this check.")).toHaveLength(2)
    expect(
      within(savings).getAllByText("~$1.25 confirmed from 1 of 2 verified cycles"),
    ).toHaveLength(2)
    expect(within(savings).getByRole("button", { name: "Details" })).toHaveAttribute(
      "aria-expanded",
      "false",
    )
  })

  it("uses backend-trusted active wins without remediation progress", async () => {
    const unrelated = {
      ...aggregate.wins[0]!,
      findingId: "unrelated-finding",
      remediationCycleId: "unrelated-cycle",
      display: {
        ...aggregate.wins[0]!.display,
        estimatedOpportunity: { value: 900, unit: "literalInputTokens" as const },
      },
    }
    setup(target, false, { wins: [aggregate.wins[0]!, unrelated] }, passedTargetReport)

    const savings = await screen.findByRole("region", { name: "Savings" })
    expect(within(savings).getByText("2 verified remediation cycles")).toBeVisible()
    expect(within(savings).getAllByText("~900 input tokens projected")).toHaveLength(2)
  })

  it("hides savings while aggregate wins load and shows them when ready", async () => {
    const pending = deferred<AggregateWinsPayload>()
    setup(target, false, pending.promise, passedTargetReport)

    await screen.findByRole("button", { name: /Passed checks/ })
    expect(screen.queryByRole("region", { name: "Savings" })).not.toBeInTheDocument()

    pending.resolve(aggregate)
    expect(await screen.findByRole("region", { name: "Savings" })).toBeVisible()
  })

  it("hides savings when aggregate wins fail to load", async () => {
    const pending = deferred<AggregateWinsPayload>()
    setup(target, false, pending.promise, passedTargetReport)

    await screen.findByRole("button", { name: /Passed checks/ })
    pending.reject(new Error("Unavailable"))
    await waitFor(() => expect(screen.queryByRole("region", { name: "Savings" })).toBeNull())
  })

  it("counts one estimated opportunity per exact target", async () => {
    const opportunity = {
      ...aggregate.wins[0]!,
      display: {
        ...aggregate.wins[0]!.display,
        estimatedOpportunity: { value: 400, unit: "literalInputTokens" as const },
      },
    }
    setup(
      target,
      false,
      {
        wins: [opportunity, { ...opportunity, remediationCycleId: "cycle-2" }],
      },
      passedTargetReport,
    )

    const savings = await screen.findByRole("region", { name: "Savings" })
    expect(within(savings).getAllByText("~400 input tokens projected")).toHaveLength(2)
    expect(within(savings).queryByText("~800 input tokens projected")).not.toBeInTheDocument()
  })

  it("uses generic clean copy without unrelated action attribution", async () => {
    setup(target, false, aggregate, passedTargetReport)

    expect(await screen.findByText("No finding in 2 complete sessions.")).toBeVisible()
    expect(screen.queryByText("Current verification passed.")).not.toBeInTheDocument()
    expect(screen.queryByText("Verified after your fix.")).not.toBeInTheDocument()
  })

  it("pairs authoritative token and cost totals that cover the same wins", async () => {
    setup(
      target,
      false,
      {
        wins: [
          {
            ...aggregate.wins[0]!,
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
          },
        ],
      },
      passedTargetReport,
    )

    const savings = await screen.findByRole("region", { name: "Savings" })
    expect(within(savings).getAllByText("~$1.25 confirmed")).toHaveLength(2)
    fireEvent.click(within(savings).getByRole("button", { name: "Details" }))
    expect(within(savings).getByText("Old model usage")).toHaveClass("text-label")
  })

  it("removes savings when a passed check regresses", async () => {
    const { adapter, session } = setup(target, false, aggregate, passedTargetReport)

    expect(await screen.findByRole("region", { name: "Savings" })).toBeVisible()

    vi.mocked(adapter.getReport).mockResolvedValue(report)
    act(() => session.refresh())

    await screen.findByRole("heading", { name: "Failed checks 1" })
    expect(screen.queryByRole("region", { name: "Savings" })).not.toBeInTheDocument()
  })

  it("shows pending confirmed savings and the approved tooltip text", async () => {
    setup(
      target,
      false,
      {
        wins: [
          {
            ...aggregate.wins[0]!,
            savings: {
              ...aggregate.wins[0]!.savings,
              status: { status: "pending", methodRevision: 1 },
              apiEquivalentCostAvoidedUsd: null,
            },
          },
        ],
      },
      passedTargetReport,
    )

    const savings = await screen.findByRole("region", { name: "Savings" })
    expect(within(savings).getAllByText("Pending recent usage.")).toHaveLength(2)
    fireEvent.focus(within(savings).getByRole("button", { name: "About confirmed savings" }))
    expect(
      await screen.findByText(
        "Savings observed across sessions that passed after remediation. This is still an estimate and may not match provider billing exactly.",
      ),
    ).toBeVisible()
  })

  it("describes estimated savings as a pre-remediation opportunity", async () => {
    setup(target, false, aggregate, passedTargetReport)

    const savings = await screen.findByRole("region", { name: "Savings" })
    fireEvent.focus(within(savings).getByRole("button", { name: "About estimated savings" }))
    expect(
      await screen.findByText(
        "Pre-remediation opportunity estimated from evidence observed before the fix. Actual results can vary.",
      ),
    ).toBeVisible()
    expect(screen.queryByText(/your recent usage/i)).not.toBeInTheDocument()
  })
})
