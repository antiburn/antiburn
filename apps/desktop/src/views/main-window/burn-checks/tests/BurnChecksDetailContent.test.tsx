import { fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { BurnCheckDetectorId, ChecksReportPayload } from "../../../../lib/insightsIpc"
import type * as ClipboardModule from "../../../../lib/clipboard"
import type * as InsightsIpcModule from "../../../../lib/insightsIpc"
import type * as IpcModule from "../../../../lib/ipc"
import { BurnCheckDetail, CheckPromptAction } from "../BurnCheckDetail"
import { BurnCheckTargetDetail } from "../BurnCheckTargetDetail"
import { BurnCheckTargetActions } from "../BurnCheckTargetActions"
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
beforeEach(() => installBurnChecksCommandMocks(commands))
afterEach(() => restoreBurnChecksTestWindow())

describe("BurnChecksView detail content", { timeout: 15_000 }, () => {
  it.each(["check", "target"] as const)("keeps %s prompt analytics generic", async (kind) => {
    render(
      kind === "check" ? (
        <CheckPromptAction detector="oldModelUsage" targets={[target]} refresh={vi.fn()} />
      ) : (
        <BurnCheckTargetActions target={target} refresh={vi.fn()} />
      ),
    )
    fireEvent.click(screen.getByRole("button", { name: "Copy fix prompt" }))
    await screen.findByRole("button", { name: "Copied" })
    expect(commands.noteInteraction.mock.calls).toEqual([
      [{ kind: "burnCheckPromptPrepared", outcome: "ready" }],
      [{ kind: "burnCheckPromptCopied" }],
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
    expect(screen.getAllByText(description.textContent!)).toHaveLength(1)
    expect(snooze.compareDocumentPosition(fix) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0)
    expect(fix.compareDocumentPosition(action) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0)
    fireEvent.click(action)
    await waitFor(() => expect(commands.copyFallback).toHaveBeenCalledWith("oldModelUsage"))
    expect(commands.writeClipboardText).toHaveBeenCalledWith("Inspect representative evidence.")
    expect(screen.getByRole("region", { name: "Burn check details" })).toHaveTextContent(
      "1 failed · 2 passed",
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

  it("requests the current whole-check prompt instead of rendered target handles", async () => {
    const targets = Array.from({ length: 13 }, (_, index) => ({
      ...target,
      findingId: `finding-${index}`,
      actionId: `action-${index}`,
    }))
    render(<CheckPromptAction detector="oldModelUsage" targets={targets} refresh={vi.fn()} />)

    fireEvent.click(await screen.findByRole("button", { name: "Copy fix prompt" }))

    await waitFor(() => expect(commands.copyFallback).toHaveBeenCalledWith("oldModelUsage"))
    expect(commands.copyBatch).not.toHaveBeenCalled()
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
          { ...target.samples[0]!, navigationHandle: "opaque-handle-2", title: "Review model" },
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
    const estimates: Array<[BurnCheckDetectorId, number, string]> = [
      ["sessionsOverDepth", 800, "8%"],
      ["modelOverthinking", 350, "3%"],
      ["overpoweredSubagents", 880, "8%"],
      ["unusedMcpServers", 100, "1%"],
      ["unusedBuiltInTools", 1, "Under 1%"],
      ["unusedSkills", 100, "1%"],
      ["oldModelUsage", 400, "4%"],
      ["overuseOfFastMode", 333, "3%"],
      ["cacheChurn", 700, "7%"],
    ]
    const allFailures: ChecksReportPayload = {
      ...report,
      estimatedTokenBurnBasisPoints: 880,
      categories: estimates.map(([id, estimatedTokenBurnBasisPoints]) => ({
        id,
        finding: 1,
        clean: 0,
        unavailable: 0,
        estimatedTokenBurnBasisPoints,
        lifecycle: "failing",
      })),
    }
    setup(target, false, aggregate, allFailures)
    for (const [, , metric] of estimates) {
      expect(
        (await screen.findAllByRole("button", { name: new RegExp(`${metric} estimated burn`) }))
          .length,
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
      "model",
      "futureModelDefault",
      "activeSessionsKeepTheirModel",
      "Model",
      "Active sessions keep their selected model",
    ],
    [
      "skill",
      "skillAdvertising",
      "explicitSkillUseRemainsAvailable",
      "Skill",
      "Explicit skill use remains available",
    ],
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
      "Helper model",
      "Future worker responses can change",
    ],
    [
      "mcpServer",
      "mcpAvailability",
      "serverWillNotBeAvailable",
      "Tool server",
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
    expect(await screen.findByRole("dialog", { name: "Confirm change" })).toHaveTextContent(
      "mcp_servers.reviewed.enabled",
    )
  })

  it("renders a recurred verification state", () => {
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
              lifecycle: "recurred",
              verification: { status: "recurred", methodRevision: 1, evidenceRevision: "e2" },
              savings: { status: "pending" },
            },
          },
        ]}
        samples={target.samples}
        failedSessionCount={1}
        refresh={vi.fn()}
      />,
    )
    expect(screen.getByText("This finding returned.")).toBeVisible()
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
