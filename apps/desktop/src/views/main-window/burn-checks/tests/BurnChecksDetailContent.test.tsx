import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { ChecksReportPayload } from "../../../../lib/insightsIpc"
import type * as ClipboardModule from "../../../../lib/clipboard"
import type * as InsightsIpcModule from "../../../../lib/insightsIpc"
import type * as IpcModule from "../../../../lib/ipc"
import { CheckPromptAction } from "../BurnCheckDetail"

import {
  report,
  target,
  aggregate,
  deferred,
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
}))

vi.mock("../../../../lib/insightsIpc", async (importOriginal) => ({
  ...(await importOriginal<typeof InsightsIpcModule>()),
  prepareAutoFixBurnCheckTarget: commands.prepare,
  applyPreparedBurnCheckOperation: commands.apply,
  copyPromptFixBurnCheckTarget: commands.copy,
  copyPromptFixBurnCheck: commands.copyFallback,
  copyPromptFixBurnCheckTargets: commands.copyBatch,
  openBurnCheckSample: commands.openSample,
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

  it("shows the exact agent mark and configuration scope", async () => {
    setup()

    expect(await screen.findAllByRole("img", { name: "Claude Code" })).toHaveLength(1)
    expect(screen.getByText("Global configuration")).toBeVisible()
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
})
