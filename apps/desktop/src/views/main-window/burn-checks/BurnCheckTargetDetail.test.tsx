import { act, fireEvent, render, screen } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import type { BurnCheckTargetPayload } from "../../../lib/insightsIpc"
import { BurnCheckTargetDetail, targetCostLine } from "./BurnCheckTargetDetail"
import { scopeLabel, targetTitle } from "./BurnCheckTargetPresentation"
import { performProjectFolderAction } from "../../../lib/projectFolder"
import { getBurnCheckTargetEvidence, openBurnCheckSample } from "../../../lib/insightsIpc"

vi.mock("../../../lib/insightsIpc", () => ({
  getBurnCheckTargetEvidence: vi.fn(),
  openBurnCheckSample: vi.fn(),
}))

vi.mock("../../../lib/projectFolder", () => ({
  performProjectFolderAction: vi.fn().mockResolvedValue(undefined),
}))
beforeEach(() => {
  vi.mocked(performProjectFolderAction).mockClear()
  vi.mocked(getBurnCheckTargetEvidence).mockReset()
  vi.mocked(openBurnCheckSample).mockReset()
})

function target(overrides: Partial<BurnCheckTargetPayload> = {}): BurnCheckTargetPayload {
  return {
    findingId: "finding-stable",
    actionId: "action-fresh",
    finding: {
      detector: "unusedBuiltInTools",
      agent: "claude-code",
      sourceFormat: "claudeJsonl",
      observation: "The Workflow tool was never invoked.",
      labels: [],
      omitted: 0,
    },
    display: {
      resourceKind: "builtInTool",
      resourceIdentity: "Workflow",
      currentValue: null,
      replacementValue: null,
      scopeKind: "session",
      quantity: 400,
      quantityUnit: "tokens",
      observationCount: 2,
      firstObservedAtMs: 1,
      lastObservedAtMs: 2,
      estimateMethod: "builtInDefinitionReplication",
      estimatedOpportunity: null,
      estimatedTokenBurnBasisPoints: null,
      verificationLimit: "freshEvidenceFromSameSourceAndTarget",
    },
    occurrenceCount: 2,
    autoFix: { status: "unavailable", reason: "unsupportedOrUnprovenTarget" },
    promptFix: { status: "unavailable", reason: "checkUnsupportedForAgent" },
    watch: null,
    evidenceAvailable: false,
    coverageLimits: ["currentPublishedEvidenceOnly"],
    samples: [],
    expiresAtEpoch: 100,
    ...overrides,
  }
}

describe("target cost line", () => {
  it("uses the distinct affected-session count instead of the occurrence count", () => {
    expect(
      targetCostLine(
        target({
          affectedSessionCount: 1,
          occurrenceCount: 4,
          display: {
            ...target().display,
            estimatedOpportunity: { value: 8.2, unit: "apiEquivalentUsd" },
          },
        }),
      ),
    ).toBe(
      "~$8.20 in cache reads of this unused definition across 1 session, sub-agent requests included.",
    )
  })

  it("falls back to occurrence wording when the affected-session count is unavailable", () => {
    expect(
      targetCostLine(
        target({
          display: {
            ...target().display,
            estimatedOpportunity: { value: 1, unit: "apiEquivalentUsd" },
          },
        }),
      ),
    ).toBe(
      "~$1.00 in cache reads of this unused definition across 2 occurrences, sub-agent requests included.",
    )
  })

  it("omits the line when there is no estimate", () => {
    expect(targetCostLine(target())).toBeNull()
  })

  it("omits the line when the estimate did not price to dollars", () => {
    expect(
      targetCostLine(
        target({
          display: {
            ...target().display,
            estimatedOpportunity: { value: 400, unit: "literalInputTokens" },
          },
        }),
      ),
    ).toBeNull()
  })
})

describe("BurnCheckTargetDetail", () => {
  it("shows missing instruction text, full bounded action text, and optional context", async () => {
    vi.mocked(getBurnCheckTargetEvidence).mockResolvedValue({
      status: "available",
      items: [
        {
          label: "instruction",
          sourceLabel: "AGENTS.md · Testing",
          reference: "rule",
          observedAtMs: null,
          startLine: 24,
          endLine: 27,
          excerpt: "Instruction text unavailable.",
          explanation: "",
          limitation: "The instruction file changed.",
        },
        {
          label: "observedAction",
          sourceLabel: "Session action",
          reference: "action",
          observedAtMs: null,
          startLine: null,
          endLine: null,
          excerpt: "x".repeat(900),
          explanation: "",
          limitation: null,
        },
        {
          label: "context",
          sourceLabel: "Context",
          reference: "context",
          observedAtMs: 1,
          startLine: null,
          endLine: null,
          excerpt: "earlier event",
          explanation: "",
          limitation: null,
        },
      ],
    })
    render(
      <BurnCheckTargetDetail
        target={target({ evidenceAvailable: true })}
        reportRow
        refresh={() => undefined}
      />,
    )
    fireEvent.click(screen.getByRole("button", { name: "Show evidence" }))
    expect(
      await screen.findByText("Unavailable or changed since this assessment."),
    ).toBeInTheDocument()
    expect(screen.queryByText("Instruction text unavailable.")).not.toBeInTheDocument()
    expect(screen.queryByText("The instruction file changed.")).not.toBeInTheDocument()
    expect(
      screen.getByText("Instruction · AGENTS.md · Testing · lines 24–27"),
    ).toBeInTheDocument()
    expect(screen.getByText("x".repeat(900))).toBeInTheDocument()
    expect(screen.queryByText("earlier event")).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Show context" }))
    expect(screen.getByText("earlier event")).toBeInTheDocument()
  })

  it("shows only the failed instruction and a compact action for Ignored Instructions", async () => {
    vi.mocked(getBurnCheckTargetEvidence).mockResolvedValue({
      status: "available",
      items: [
        {
          label: "instruction",
          sourceLabel: ".config/opencode/AGENTS.md",
          reference: "instruction",
          observedAtMs: null,
          startLine: 45,
          endLine: 45,
          excerpt: "Do not use broad searches when exploring a codebase.",
          explanation: "",
          limitation: null,
        },
        {
          label: "observedAction",
          sourceLabel: "Observed session action",
          reference: "action",
          observedAtMs: null,
          startLine: null,
          endLine: null,
          excerpt: JSON.stringify({
            command: "rg -n 'long search string' .",
            workdir: "/private/project",
          }),
          explanation: "",
          limitation: null,
        },
        {
          label: "context",
          sourceLabel: "Session context",
          reference: "context",
          observedAtMs: 1,
          startLine: null,
          endLine: null,
          excerpt: "Unneeded surrounding event",
          explanation: "",
          limitation: null,
        },
      ],
    })
    render(
      <BurnCheckTargetDetail
        target={target({
          evidenceAvailable: true,
          finding: { ...target().finding, detector: "ignoredInstructions" },
        })}
        refresh={() => undefined}
      />,
    )

    fireEvent.click(screen.getByRole("button", { name: "Show evidence" }))
    expect(
      await screen.findByText("Do not use broad searches when exploring a codebase."),
    ).toBeInTheDocument()
    expect(screen.getByText("Instruction")).toBeInTheDocument()
    expect(screen.getByText(".config/opencode/AGENTS.md · line 45")).toBeInTheDocument()
    expect(screen.getByRole("region", { name: "Evidence" })).not.toHaveClass("border-t")
    expect(screen.getByText("rg -n 'long search string' .")).toBeInTheDocument()
    expect(screen.queryByText("Unneeded surrounding event")).not.toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "Show context" })).not.toBeInTheDocument()
  })

  it("shows the full bounded evidence without formatting or expansion toggles", async () => {
    vi.mocked(getBurnCheckTargetEvidence).mockResolvedValue({
      status: "available",
      items: [
        {
          label: "observedAction",
          sourceLabel: "Bash input",
          reference: "action",
          observedAtMs: null,
          startLine: null,
          endLine: null,
          excerpt: `first line\n  ${"detail ".repeat(40)}`,
          explanation: "This is the cited action.",
          limitation: null,
        },
      ],
    })
    render(
      <BurnCheckTargetDetail
        target={target({ evidenceAvailable: true })}
        refresh={() => undefined}
      />,
    )
    fireEvent.click(screen.getByRole("button", { name: "Show evidence" }))
    expect(
      await screen.findByText(`first line ${"detail ".repeat(40)}`.trim()),
    ).toBeInTheDocument()
    expect(screen.getByText(/Bash input · This is the cited action/)).toBeInTheDocument()
    expect(
      screen.queryByRole("button", { name: "Show original formatting" }),
    ).not.toBeInTheDocument()
    expect(screen.queryByText("View full bounded evidence")).not.toBeInTheDocument()
  })

  it("shows the outdated-instructions note once when historical text is unconfirmed", async () => {
    const limitation = "Note: this session may have run on outdated instructions."
    vi.mocked(getBurnCheckTargetEvidence).mockResolvedValue({
      status: "available",
      items: [
        {
          label: "instruction",
          sourceLabel: "AGENTS.md",
          reference: "rule",
          observedAtMs: null,
          startLine: 3,
          endLine: 3,
          excerpt: "Run tests before merging.",
          explanation: "",
          limitation,
        },
        {
          label: "observedAction",
          sourceLabel: "Session action",
          reference: "action",
          observedAtMs: null,
          startLine: null,
          endLine: null,
          excerpt: "Merged without tests.",
          explanation: "",
          limitation: null,
        },
      ],
    })
    render(
      <BurnCheckTargetDetail
        target={target({
          evidenceAvailable: true,
          finding: { ...target().finding, detector: "ignoredInstructions" },
        })}
        refresh={() => undefined}
      />,
    )
    fireEvent.click(screen.getByRole("button", { name: "Show evidence" }))
    expect(await screen.findByText(limitation)).toBeInTheDocument()
    expect(screen.getAllByText(limitation)).toHaveLength(1)
    expect(screen.getByText("Merged without tests.")).toBeInTheDocument()
  })

  it.each([
    "claudeJsonl",
    "codexRolloutJsonl",
    "piV3Jsonl",
    "openCodeSqliteV2",
    "cursorCliAgentJsonl",
    "antigravityBrainJsonl",
  ] as const)("renders cited instruction and action for %s", async (sourceFormat) => {
    vi.mocked(getBurnCheckTargetEvidence).mockResolvedValue({
      status: "available",
      items: [
        {
          label: "instruction",
          sourceLabel: "AGENTS.md · Testing",
          reference: "rule",
          observedAtMs: null,
          startLine: 1,
          endLine: 1,
          excerpt: "Do not run the shell command without approval.",
          explanation: "",
          limitation: null,
        },
        {
          label: "observedAction",
          sourceLabel: "Session action",
          reference: "action",
          observedAtMs: 1,
          startLine: null,
          endLine: null,
          excerpt: "Ran the shell command without approval.",
          explanation: "",
          limitation: null,
        },
      ],
    })
    render(
      <BurnCheckTargetDetail
        target={target({
          evidenceAvailable: true,
          finding: { ...target().finding, sourceFormat },
        })}
        refresh={() => undefined}
      />,
    )
    fireEvent.click(screen.getByRole("button", { name: "Show evidence" }))
    expect(
      await screen.findByText("Do not run the shell command without approval."),
    ).toBeInTheDocument()
    expect(screen.getByText("Ran the shell command without approval.")).toBeInTheDocument()
  })

  it("retries failed requests and ignores a response after closing evidence", async () => {
    let resolveLate!: (value: Awaited<ReturnType<typeof getBurnCheckTargetEvidence>>) => void
    vi.mocked(getBurnCheckTargetEvidence)
      .mockRejectedValueOnce(new Error("offline"))
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveLate = resolve
          }),
      )
    render(
      <BurnCheckTargetDetail
        target={target({ evidenceAvailable: true })}
        refresh={() => undefined}
      />,
    )
    fireEvent.click(screen.getByRole("button", { name: "Show evidence" }))
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not load evidence.")
    fireEvent.click(screen.getByRole("button", { name: "Retry" }))
    fireEvent.click(screen.getByRole("button", { name: "Hide details" }))
    await act(async () => resolveLate({ status: "available", items: [] }))
    expect(screen.queryByText("Could not load evidence.")).not.toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Show evidence" })).toHaveAttribute(
      "aria-expanded",
      "false",
    )
  })

  it("ignores evidence from a replaced target", async () => {
    let resolveOld!: (value: Awaited<ReturnType<typeof getBurnCheckTargetEvidence>>) => void
    vi.mocked(getBurnCheckTargetEvidence)
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveOld = resolve
          }),
      )
      .mockResolvedValueOnce({ status: "unavailable", items: [] })
    const first = target({ evidenceAvailable: true })
    const view = render(<BurnCheckTargetDetail target={first} refresh={() => undefined} />)
    fireEvent.click(screen.getByRole("button", { name: "Show evidence" }))
    view.rerender(
      <BurnCheckTargetDetail
        target={{
          ...first,
          findingId: "finding-replaced",
          actionId: "replacement",
          finding: { ...first.finding, detector: "ignoredInstructions" },
          display: { ...first.display, instructionTitle: "Quality Review" },
        }}
        refresh={() => undefined}
      />,
    )
    fireEvent.click(screen.getByRole("button", { name: "Show evidence" }))
    expect(getBurnCheckTargetEvidence).toHaveBeenCalledTimes(2)
    expect(await screen.findAllByText("Quality Review")).not.toHaveLength(0)
    expect(screen.getByText("Where it was ignored")).toBeInTheDocument()
    expect(await screen.findByText("The Workflow tool was never invoked.")).toBeInTheDocument()
    expect(
      screen.getByText(/exact instruction and action text was not saved/),
    ).toBeInTheDocument()
    await act(async () =>
      resolveOld({
        status: "available",
        items: [
          {
            label: "observedAction",
            sourceLabel: "Session action",
            reference: "old",
            observedAtMs: null,
            startLine: null,
            endLine: null,
            excerpt: "old private action",
            explanation: "",
            limitation: null,
          },
        ],
      }),
    )
    expect(screen.queryByText("old private action")).not.toBeInTheDocument()
  })

  it("loads validated evidence on demand and opens its session", async () => {
    vi.mocked(getBurnCheckTargetEvidence).mockResolvedValue({
      status: "available",
      items: [
        {
          label: "observedAction",
          sourceLabel: "Observed session action",
          reference: "opaque-action",
          observedAtMs: 1000,
          startLine: null,
          endLine: null,
          excerpt: "git push --force",
          explanation: "This is the action cited by the assessment.",
          limitation: null,
        },
      ],
    })
    const currentTarget = target({
      finding: {
        ...target().finding,
        detector: "ignoredInstructions",
      },
      display: {
        ...target().display,
        resourceIdentity: "home:.config/opencode/AGENTS.md",
        instructionTitle: "Code Discovery",
        scopeKind: "global",
      },
      evidenceAvailable: true,
      samples: [
        {
          navigationHandle: "opaque-session",
          title: "Session",
          agent: "claude-code",
          surface: "cli",
          observedAtMs: 1000,
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
    })
    render(<BurnCheckTargetDetail target={currentTarget} refresh={() => undefined} />)
    expect(screen.getByRole("region", { name: "Failed sessions" })).toBeInTheDocument()

    await act(async () =>
      fireEvent.click(screen.getByRole("button", { name: "Show evidence" })),
    )
    expect(await screen.findByText("git push --force")).toBeInTheDocument()
    expect(
      screen.queryByText(
        "Observed session action · This is the action cited by the assessment.",
      ),
    ).not.toBeInTheDocument()
    expect(
      screen.getByText("Global configuration (~/.config/opencode/AGENTS.md)"),
    ).toBeInTheDocument()
    expect(screen.getByRole("heading", { name: /Code Discovery/ })).toBeInTheDocument()
    expect(
      screen.getByText("Where it was ignored").parentElement?.querySelector("time"),
    ).not.toBeNull()
    expect(getBurnCheckTargetEvidence).toHaveBeenCalledWith("action-fresh")
    await act(async () =>
      fireEvent.click(screen.getByRole("button", { name: /Session Claude Code/ })),
    )
    expect(openBurnCheckSample).toHaveBeenCalledWith("opaque-session")
  })

  it("shows grouped instruction evidence without an occurrence selector", async () => {
    vi.mocked(getBurnCheckTargetEvidence).mockResolvedValue({
      status: "available",
      items: [
        {
          label: "observedAction",
          sourceLabel: "Observed session action",
          reference: "selected-action",
          observedAtMs: 2000,
          startLine: null,
          endLine: null,
          excerpt: "Used a forbidden command.",
          explanation: "This action is cited by the assessment.",
          limitation: null,
        },
      ],
    })
    const currentTarget = target({
      finding: { ...target().finding, detector: "ignoredInstructions" },
      evidenceAvailable: true,
    })
    render(
      <BurnCheckTargetDetail target={currentTarget} openEvidence refresh={() => undefined} />,
    )

    expect(await screen.findByText("Used a forbidden command.")).toBeInTheDocument()
    expect(screen.queryByRole("button", { name: /Occurrence/ })).not.toBeInTheDocument()
    expect(getBurnCheckTargetEvidence).toHaveBeenCalledWith("action-fresh")
  })

  it("places the affected-session count between evidence and the session list", async () => {
    vi.mocked(getBurnCheckTargetEvidence).mockResolvedValue({
      status: "available",
      items: [
        {
          label: "observedAction",
          sourceLabel: "Observed session action",
          reference: "action",
          observedAtMs: 2000,
          startLine: null,
          endLine: null,
          excerpt: "Used a blocked tool.",
          explanation: "",
          limitation: null,
        },
      ],
    })
    render(
      <BurnCheckTargetDetail
        target={target({
          finding: { ...target().finding, detector: "ignoredInstructions" },
          affectedSessionCount: 3,
          evidenceAvailable: true,
        })}
        reportRow
        openEvidence
        refresh={() => undefined}
      />,
    )

    const evidence = await screen.findByText("Used a blocked tool.")
    const count = screen.getByText("3 sessions affected")
    const sessions = screen.getByRole("region", { name: "Failed sessions" })
    expect(evidence.compareDocumentPosition(count) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(
      0,
    )
    expect(count.compareDocumentPosition(sessions) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(
      0,
    )
  })

  it("shows the project instruction file as a real path", () => {
    render(
      <BurnCheckTargetDetail
        target={target({
          finding: { ...target().finding, detector: "ignoredInstructions" },
          display: {
            ...target().display,
            resourceIdentity: "project:AGENTS.md",
            instructionTitle: "Testing",
            scopeKind: "project",
          },
          projectPath: "/work/example",
        })}
        refresh={() => undefined}
      />,
    )
    expect(
      screen.getByText("Project configuration (/work/example/AGENTS.md)"),
    ).toBeInTheDocument()
  })

  it("loads auto-opened evidence once across parent rerenders", async () => {
    vi.mocked(getBurnCheckTargetEvidence).mockResolvedValue({
      status: "available",
      items: [
        {
          label: "instruction",
          sourceLabel: "AGENTS.md",
          reference: "instruction",
          observedAtMs: null,
          startLine: 1,
          endLine: 1,
          excerpt: "Use the focused search tool.",
          explanation: "",
          limitation: null,
        },
      ],
    })
    const currentTarget = target({ evidenceAvailable: true })
    const view = render(
      <BurnCheckTargetDetail target={currentTarget} refresh={() => undefined} openEvidence />,
    )
    expect(await screen.findByText("Use the focused search tool.")).toBeInTheDocument()
    view.rerender(
      <BurnCheckTargetDetail
        target={{ ...currentTarget, actionId: "action-refreshed" }}
        refresh={() => undefined}
        openEvidence
      />,
    )
    expect(getBurnCheckTargetEvidence).toHaveBeenCalledOnce()
    expect(screen.getByText("Use the focused search tool.")).toBeInTheDocument()
  })

  it("shows the cost line under the recommendation when priced", () => {
    render(
      <BurnCheckTargetDetail
        target={target({
          display: {
            ...target().display,
            estimatedOpportunity: { value: 8.2, unit: "apiEquivalentUsd" },
          },
        })}
        refresh={() => undefined}
      />,
    )
    expect(screen.getByText(/~\$8\.20 in cache reads/)).toBeInTheDocument()
  })

  it("offers full-path folder actions and distinguishes samples from affected sessions", async () => {
    render(
      <BurnCheckTargetDetail
        reportRow
        refresh={() => undefined}
        target={target({
          projectName: "example-project",
          projectLocation: "…/worktrees/example-project",
          projectPath: "/tmp/worktrees/example-project",
          affectedSessionCount: 7,
          display: { ...target().display, scopeKind: "project" },
          samples: [
            {
              navigationHandle: "sample-one",
              title: "Example session",
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
        })}
      />,
    )
    expect(screen.getByText("· example-project")).toBeInTheDocument()
    expect(screen.queryByText("…/worktrees/example-project")).not.toBeInTheDocument()
    act(() => screen.getByRole("button", { name: "Project folder" }).focus())
    expect(document.querySelector(".project-folder-path")).toHaveTextContent(
      "/tmp/worktrees/example-project",
    )
    expect(screen.getByRole("dialog").parentElement).toBe(document.body)
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Copy path" })))
    expect(performProjectFolderAction).toHaveBeenCalledWith(
      "/tmp/worktrees/example-project",
      "copy",
      { kind: "burnCheck", actionId: "action-fresh" },
    )
    await act(async () => fireEvent.click(screen.getByRole("button", { name: /^Open in/ })))
    expect(performProjectFolderAction).toHaveBeenCalledWith(
      "/tmp/worktrees/example-project",
      "open",
      { kind: "burnCheck", actionId: "action-fresh" },
    )
    fireEvent.keyDown(document, { key: "Escape" })
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
    expect(screen.queryByRole("button", { name: /Failed sessions/ })).toBeNull()
    expect(screen.getByRole("button", { name: /Example session/ })).toBeVisible()
  })

  it("does not use a shortened display location as an actionable path", () => {
    render(
      <BurnCheckTargetDetail
        reportRow
        refresh={() => undefined}
        target={target({ projectLocation: "…/worktrees/example-project" })}
      />,
    )
    expect(screen.queryByRole("button", { name: "Project folder" })).toBeNull()
  })
})

describe("scopeLabel", () => {
  it.each([
    ["global", "Global configuration"],
    ["project", "Project configuration"],
    ["session", "Session scope"],
    ["worker", "Worker scope"],
  ] as const)("formats %s scope", (scope, label) => {
    expect(scopeLabel(scope)).toBe(label)
  })
})

it.each([
  ["claude-code", "Code Discovery"],
  ["codex", "Repository Safety"],
  ["pi", "Review Rules"],
  ["opencode", "Tool Use"],
  ["cursor", "Project Instructions"],
  ["antigravity", "Testing Workflow"],
] as const)("uses the instruction heading for %s findings", (agent, heading) => {
  expect(
    targetTitle(
      target({
        finding: {
          ...target().finding,
          detector: "ignoredInstructions",
          agent,
        },
        display: {
          ...target().display,
          resourceIdentity: "home:AGENTS.md",
          instructionTitle: heading,
        },
      }),
    ),
  ).toBe(heading)
})
