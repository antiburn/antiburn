import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type {
  BurnCheckTargetEvidencePayload,
  ChecksReportPayload,
} from "../../../../lib/insightsIpc"
import type * as ClipboardModule from "../../../../lib/clipboard"
import type * as InsightsIpcModule from "../../../../lib/insightsIpc"
import type * as IpcModule from "../../../../lib/ipc"
import { BurnCheckDetail, CheckPromptAction } from "../BurnCheckDetail"
import { BurnCheckTargetDetail } from "../BurnCheckTargetDetail"
import { BurnCheckTargetActions } from "../BurnCheckTargetActions"
import { BurnChecksView } from "../../BurnChecksView"
import { CHECK_LABELS } from "../../../../lib/presentation/checkReport"

import {
  report,
  target,
  aggregate,
  setWindowWidth,
  setup,
  installBurnChecksCommandMocks,
  restoreBurnChecksTestWindow,
} from "./burnChecksTestSupport"

const commands = vi.hoisted(() => ({
  prepare: vi.fn(),
  apply: vi.fn(),
  copy: vi.fn(),
  copyFallback: vi.fn(),
  copyBatch: vi.fn(),
  writeClipboardText: vi.fn(),
  openSample: vi.fn(),
  noteInteraction: vi.fn(),
  evidence: vi.fn(),
}))

vi.mock("../../../../lib/insightsIpc", async (importOriginal) => ({
  ...(await importOriginal<typeof InsightsIpcModule>()),
  prepareAutoFixBurnCheckTarget: commands.prepare,
  applyPreparedBurnCheckOperation: commands.apply,
  copyPromptFixBurnCheckTarget: commands.copy,
  copyPromptFixBurnCheck: commands.copyFallback,
  copyPromptFixBurnCheckTargets: commands.copyBatch,
  openBurnCheckSample: commands.openSample,
  getBurnCheckTargetEvidence: commands.evidence,
}))

vi.mock("../../../../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof IpcModule>()),
  noteInteraction: commands.noteInteraction,
}))

vi.mock("../../../../lib/clipboard", async (importOriginal) => ({
  ...(await importOriginal<typeof ClipboardModule>()),
  writeClipboardText: commands.writeClipboardText,
}))

beforeEach(() => {
  installBurnChecksCommandMocks(commands)
})

afterEach(() => {
  restoreBurnChecksTestWindow()
})

// This file holds the detail content tests for the Burn Checks view.
// The suite is split across files in this folder by theme. Each test renders
// the full view. CI runs this file next to the other heavy view suites, so
// one test can take five times its local run time. 15 s is the bound, not a
// target.
describe("BurnChecksView detail content", { timeout: 15_000 }, () => {
  const smartChecks = [
    ["ignoredInstructions", "ignored_instructions"],
    ["scopeCreep", "scope_creep"],
    ["overExploring", "over_exploring"],
    ["skillOpportunities", "skill_opportunities"],
  ] as const

  it.each(smartChecks)(
    "classifies the %s batch prompt without serializing target metadata",
    async (detector, check) => {
      render(
        <CheckPromptAction
          detector={detector}
          targets={[{ ...target, finding: { ...target.finding, detector } }]}
          refresh={vi.fn()}
        />,
      )
      fireEvent.click(screen.getByRole("button", { name: "Copy fix prompt" }))
      await screen.findByRole("button", { name: "Copied" })
      expect(commands.copyBatch).toHaveBeenCalledWith([target.actionId])
      expect(commands.noteInteraction.mock.calls).toEqual([
        [{ kind: "burnCheckPromptPrepared", check, outcome: "ready" }],
        [{ kind: "burnCheckPromptCopied", check }],
      ])
    },
  )

  it("preserves generic legacy prompt events without a fabricated Smart Check label", async () => {
    render(<CheckPromptAction detector="oldModelUsage" targets={[target]} refresh={vi.fn()} />)
    fireEvent.click(screen.getByRole("button", { name: "Copy fix prompt" }))
    await screen.findByRole("button", { name: "Copied" })
    expect(commands.noteInteraction.mock.calls).toEqual([
      [{ kind: "burnCheckPromptPrepared", outcome: "ready" }],
      [{ kind: "burnCheckPromptCopied" }],
    ])
  })

  it("does not report awaiting-verification detail as a visible failing finding", async () => {
    setup([], false, aggregate, {
      ...report,
      categories: [
        {
          ...report.categories[0]!,
          id: "ignoredInstructions",
          lifecycle: "awaitingVerification",
        },
      ],
    })
    fireEvent.click(
      await screen.findByRole("button", {
        name: /Ignored instructions, Awaiting verification/,
      }),
    )
    expect(
      commands.noteInteraction.mock.calls.filter(
        ([event]) => event.kind === "smartCheckObserved",
      ),
    ).toEqual([])
  })

  it.each(smartChecks)(
    "classifies %s prompt preparation and only its first clipboard success",
    async (detector, check) => {
      const surfaces = ["check", "target"] as const
      for (const surface of surfaces) {
        commands.noteInteraction.mockClear()
        commands.copy.mockClear()
        commands.copyFallback.mockClear()
        commands.writeClipboardText.mockClear()
        commands.writeClipboardText.mockRejectedValueOnce(new Error("private clipboard error"))
        const view = render(
          surface === "check" ? (
            <CheckPromptAction detector={detector} targets={[]} refresh={vi.fn()} />
          ) : (
            <BurnCheckTargetActions
              target={{
                ...target,
                finding: { ...target.finding, detector },
                autoFix: { status: "unavailable", reason: "unsupportedOrUnprovenTarget" },
              }}
              refresh={vi.fn()}
            />
          ),
        )
        fireEvent.click(screen.getByRole("button", { name: "Copy fix prompt" }))
        expect(await screen.findByRole("alert")).toHaveTextContent(
          "Could not copy the prompt. Try again.",
        )
        expect(commands.noteInteraction.mock.calls).toEqual([
          [{ kind: "burnCheckPromptPrepared", check, outcome: "ready" }],
        ])
        fireEvent.click(screen.getByRole("button", { name: "Copy fix prompt" }))
        fireEvent.click(await screen.findByRole("button", { name: "Copied" }))
        await screen.findByRole("button", { name: "Copied" })
        expect(commands.writeClipboardText).toHaveBeenCalledTimes(3)
        expect(
          surface === "check" ? commands.copyFallback : commands.copy,
        ).toHaveBeenCalledOnce()
        expect(commands.noteInteraction.mock.calls).toEqual([
          [{ kind: "burnCheckPromptPrepared", check, outcome: "ready" }],
          [{ kind: "burnCheckPromptCopied", check }],
        ])
        view.unmount()
      }
    },
  )

  it.each(smartChecks)(
    "classifies %s failed and unavailable preparations without copy or legacy events",
    async (detector, check) => {
      for (const surface of ["check", "target"] as const) {
        for (const outcome of ["unavailable", "failed"] as const) {
          commands.noteInteraction.mockClear()
          commands.writeClipboardText.mockClear()
          const prepare = surface === "check" ? commands.copyFallback : commands.copy
          if (outcome === "failed")
            prepare.mockRejectedValueOnce(new Error("private preparation error"))
          else
            prepare.mockResolvedValueOnce({
              outcome: "unavailable",
              reason: "checkUnsupportedForAgent",
            })
          const view = render(
            surface === "check" ? (
              <CheckPromptAction detector={detector} targets={[]} refresh={vi.fn()} />
            ) : (
              <BurnCheckTargetActions
                target={{
                  ...target,
                  finding: { ...target.finding, detector },
                  autoFix: { status: "unavailable", reason: "unsupportedOrUnprovenTarget" },
                }}
                refresh={vi.fn()}
              />
            ),
          )
          fireEvent.click(screen.getByRole("button", { name: "Copy fix prompt" }))
          await screen.findByRole("alert")
          expect(commands.noteInteraction.mock.calls).toEqual([
            [{ kind: "burnCheckPromptPrepared", check, outcome }],
          ])
          expect(commands.writeClipboardText).not.toHaveBeenCalled()
          view.unmount()
        }
      }
    },
  )

  it.each(["stale", "expired"] as const)(
    "retains the classified %s target preparation outcome",
    async (outcome) => {
      commands.copy.mockResolvedValueOnce({ outcome })
      const refresh = vi.fn()
      render(
        <BurnCheckTargetActions
          target={{
            ...target,
            finding: { ...target.finding, detector: "scopeCreep" },
            autoFix: { status: "unavailable", reason: "unsupportedOrUnprovenTarget" },
          }}
          refresh={refresh}
        />,
      )
      fireEvent.click(screen.getByRole("button", { name: "Copy fix prompt" }))
      await screen.findByRole("alert")
      expect(commands.noteInteraction.mock.calls).toEqual([
        [{ kind: "burnCheckPromptPrepared", check: "scope_creep", outcome }],
      ])
      expect(refresh).toHaveBeenCalledOnce()
      expect(commands.writeClipboardText).not.toHaveBeenCalled()
    },
  )

  it("reports deliberate smart finding visibility once and preserves keyboard and search routes", async () => {
    const allChecks = {
      ...report,
      categories: [
        report.categories[0]!,
        ...smartChecks.map(([id]) => ({
          ...report.categories[0]!,
          id,
          estimatedTokenBurnBasisPoints: null,
        })),
      ],
    }
    const { session, view } = setup([], false, aggregate, allChecks)
    const legacyRow = await screen.findByRole("button", { name: /Old model usage, 1 failed/ })
    const findingEvents = () =>
      commands.noteInteraction.mock.calls.filter(
        ([event]) => event.kind === "smartCheckObserved",
      )
    expect(findingEvents()).toEqual([])
    for (const [detector, check] of smartChecks) {
      const row = screen.getByRole("button", {
        name: new RegExp(`${CHECK_LABELS[detector]}, 1 failed`),
      })
      fireEvent.keyDown(row, { key: "Enter" })
      await act(async () => undefined)
      expect(document.getElementById(`burn-check-${detector}-detail`)).toHaveFocus()
      fireEvent.click(row)
      fireEvent.click(legacyRow)
      fireEvent.click(row)
      expect(
        commands.noteInteraction.mock.calls.filter(([event]) => event.check === check),
      ).toEqual([[{ kind: "smartCheckObserved", check, observation: "finding_visible" }]])
    }
    view.rerender(<BurnChecksView active={false} session={session} />)
    const before = findingEvents().length
    await act(async () => session.refresh())
    expect(findingEvents()).toHaveLength(before)
    const searchRow = screen.getByRole("button", { name: /Scope creep, 1 failed/ })
    searchRow.scrollIntoView = vi.fn()
    view.rerender(
      <BurnChecksView active session={session} focusedCheck="scopeCreep" focusRevision={1} />,
    )
    expect(await screen.findByRole("button", { name: /Scope creep, 1 failed/ })).toHaveFocus()
    expect(searchRow.scrollIntoView).toHaveBeenCalledWith({ block: "nearest" })
    expect(findingEvents()).toHaveLength(before)
    expect(
      findingEvents().every(
        ([event]) => Object.keys(event).sort().join(":") === "check:kind:observation",
      ),
    ).toBe(true)
  })

  it("opens each ignored-instruction finding with instruction and action evidence", async () => {
    const items: BurnCheckTargetEvidencePayload["items"] = [
      {
        label: "context",
        sourceLabel: "Context",
        reference: "later",
        observedAtMs: 800,
        startLine: null,
        endLine: null,
        excerpt: "Later context",
        explanation: "",
        limitation: null,
      },
      {
        label: "observedAction",
        sourceLabel: "Session action",
        reference: "action",
        observedAtMs: 1000,
        startLine: null,
        endLine: null,
        excerpt: "Delivered without tests",
        explanation: "",
        limitation: null,
      },
      {
        label: "context",
        sourceLabel: "Context",
        reference: "earlier",
        observedAtMs: 500,
        startLine: null,
        endLine: null,
        excerpt: "Earlier event",
        explanation: "",
        limitation: null,
      },
      {
        label: "instruction",
        sourceLabel: "AGENTS.md · Testing",
        reference: "rule",
        observedAtMs: null,
        startLine: 24,
        endLine: 27,
        excerpt: "Run tests before delivery.",
        explanation: "",
        limitation: null,
      },
    ]
    commands.evidence.mockResolvedValue({ status: "available", items })
    const sample = {
      ...target.samples[0]!,
      hygiene: {
        evidenceState: "ready" as const,
        unusedResources: null,
        badges: [
          {
            id: "ignoredInstructions" as const,
            status: "finding" as const,
            notAssessedReason: null,
          },
        ],
      },
    }
    setup(
      [
        {
          ...target,
          findingId: "first",
          actionId: "first-action",
          finding: { ...target.finding, detector: "ignoredInstructions" },
          evidenceAvailable: true,
          autoFix: { status: "unavailable", reason: "unsupportedOrUnprovenTarget" },
          samples: [sample],
        },
        {
          ...target,
          findingId: "second",
          actionId: "second-action",
          finding: { ...target.finding, detector: "ignoredInstructions" },
          evidenceAvailable: true,
          autoFix: { status: "unavailable", reason: "unsupportedOrUnprovenTarget" },
          samples: [sample],
        },
      ],
      false,
      aggregate,
      {
        ...report,
        categories: [
          {
            ...report.categories[0]!,
            id: "ignoredInstructions",
            finding: 1,
            clean: 0,
            estimatedTokenBurnBasisPoints: null,
          },
        ],
      },
    )
    expect(await screen.findAllByText("Run tests before delivery.")).toHaveLength(2)
    expect(screen.getAllByText("Delivered without tests")).toHaveLength(2)
    expect(
      screen.getByRole("button", { name: /Ignored instructions, 1 failed/ }),
    ).toHaveAttribute("aria-pressed", "true")
    expect(screen.getAllByText("1/1 failed").length).toBeGreaterThan(0)
    expect(
      screen
        .getAllByText("Run tests before delivery.")[0]!
        .compareDocumentPosition(screen.getAllByText("Delivered without tests")[0]!) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy()
    expect(screen.queryByRole("button", { name: "Show evidence" })).not.toBeInTheDocument()
    expect(screen.getAllByText("Supporting events")).toHaveLength(2)
    expect(screen.getAllByText("Earlier event")).toHaveLength(2)
    expect(commands.evidence).toHaveBeenCalledWith("first-action")
    expect(commands.evidence).toHaveBeenCalledWith("second-action")
    expect(
      commands.noteInteraction.mock.calls.filter(
        ([event]) =>
          event.kind === "smartCheckObserved" && event.observation.startsWith("evidence_"),
      ),
    ).toEqual([
      [
        {
          kind: "smartCheckObserved",
          check: "ignored_instructions",
          observation: "evidence_available",
        },
      ],
      [
        {
          kind: "smartCheckObserved",
          check: "ignored_instructions",
          observation: "evidence_available",
        },
      ],
    ])
    expect(screen.queryByRole("button", { name: "Show context" })).not.toBeInTheDocument()
    expect(screen.getAllByText("Earlier event")).toHaveLength(2)
    expect(screen.getAllByText("Later context")).toHaveLength(2)
    expect(screen.getByRole("button", { name: "Copy fix prompt" })).toBeEnabled()
  })

  it("retries evidence reads and discards responses after leaving the selected check", async () => {
    let resolveLate!: (evidence: BurnCheckTargetEvidencePayload) => void
    commands.evidence.mockRejectedValueOnce(new Error("offline")).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveLate = resolve
        }),
    )
    setup(
      {
        ...target,
        finding: { ...target.finding, detector: "ignoredInstructions" },
        evidenceAvailable: true,
      },
      false,
      aggregate,
      {
        ...report,
        categories: [
          { ...report.categories[0]!, id: "ignoredInstructions", clean: 0 },
          report.categories[1]!,
        ],
      },
    )
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Could not load the saved excerpts.",
    )
    fireEvent.click(screen.getByRole("button", { name: "Retry" }))
    fireEvent.click(screen.getByRole("button", { name: "Passed checks 1" }))
    fireEvent.click(screen.getByRole("button", { name: /Unused skills, Passed/ }))
    await act(async () =>
      resolveLate({
        status: "available",
        items: [
          {
            label: "observedAction",
            sourceLabel: "Session action",
            reference: "action",
            observedAtMs: null,
            startLine: null,
            endLine: null,
            excerpt: "private late action",
            explanation: "",
            limitation: null,
          },
        ],
      }),
    )
    expect(screen.queryByText("private late action")).not.toBeInTheDocument()
    expect(
      screen.queryByRole("alert", { name: "Could not load the saved excerpts." }),
    ).not.toBeInTheDocument()
    expect(
      commands.noteInteraction.mock.calls.filter(
        ([event]) =>
          event.kind === "smartCheckObserved" && event.observation.startsWith("evidence_"),
      ),
    ).toEqual([
      [
        {
          kind: "smartCheckObserved",
          check: "ignored_instructions",
          observation: "evidence_failed",
        },
      ],
    ])
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

  it("shows finding actions beside the title and keeps the explanation full width", async () => {
    setup(target, false, aggregate, report)
    const action = await screen.findByRole("button", { name: "Copy fix prompt" })
    const description = screen.getByText(
      "Some sessions used an older model when a newer one was available.",
    )
    const snooze = screen.getByRole("button", { name: "Snooze" })
    const fix = screen.getByRole("button", { name: "Fix" })
    const heading = screen.getByRole("heading", { name: "Old model usage", level: 2 })
    const titleRow = heading.parentElement?.parentElement
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

  it("shows the exact agent mark and configuration scope", () => {
    render(<BurnCheckTargetDetail target={target} refresh={vi.fn()} />)

    expect(screen.getAllByRole("img", { name: "Claude Code" })).toHaveLength(1)
    expect(screen.getByText("Global configuration")).toBeVisible()
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
      render(<BurnCheckTargetDetail target={target} refresh={vi.fn()} />)

      fireEvent.click(screen.getByRole("button", { name: "Fix" }))

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
    render(
      <BurnCheckTargetDetail
        target={{ ...target, display: { ...target.display, resourceIdentity: null } }}
        refresh={vi.fn()}
      />,
    )

    fireEvent.click(screen.getByRole("button", { name: "Fix" }))

    expect(await screen.findByRole("dialog", { name: "Review change" })).toHaveTextContent(
      "mcp_servers.reviewed.enabled",
    )
  })

  it.each([
    [
      "recurred",
      { status: "recurred", methodRevision: 1, evidenceRevision: "e2" },
      "This finding returned.",
    ],
  ] as const)("renders the typed %s state", (_name, verification, expected) => {
    render(
      <BurnCheckDetail
        detector="oldModelUsage"
        targets={[
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
        ]}
        samples={target.samples}
        failedSessionCount={1}
        refresh={vi.fn()}
      />,
    )

    expect(screen.getByText(expected)).toBeVisible()
  })

  it("hides passive verification status", () => {
    render(
      <BurnCheckDetail
        detector="oldModelUsage"
        targets={[
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
        ]}
        samples={target.samples}
        failedSessionCount={1}
        refresh={vi.fn()}
      />,
    )

    expect(
      screen.getByText("Some sessions used an older model when a newer one was available."),
    ).toBeVisible()
    expect(
      screen.queryByText("Fresh evidence verified this improvement."),
    ).not.toBeInTheDocument()
  })
})
