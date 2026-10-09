import { act, fireEvent, render, screen, within } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import type {
  BurnCheckTargetEvidencePayload,
  BurnCheckTargetPayload,
} from "../../../lib/insightsIpc"
import { BurnCheckTargetDetail, targetCostLine } from "./BurnCheckTargetDetail"
import { scopeLabel, targetTitle } from "./BurnCheckTargetPresentation"
import { performProjectFolderAction } from "../../../lib/projectFolder"
import {
  copyPromptFixBurnCheckTarget,
  getBurnCheckTargetEvidence,
  openBurnCheckSample,
} from "../../../lib/insightsIpc"
import { writeClipboardText } from "../../../lib/clipboard"
import { skillOpportunityEvidence } from "./fixtures/skillOpportunityEvidence"

vi.mock("../../../lib/insightsIpc", () => ({
  getBurnCheckTargetEvidence: vi.fn(),
  openBurnCheckSample: vi.fn(),
  copyPromptFixBurnCheckTarget: vi.fn(),
}))

vi.mock("../../../lib/clipboard", () => ({ writeClipboardText: vi.fn() }))

vi.mock("../../../lib/projectFolder", () => ({
  performProjectFolderAction: vi.fn().mockResolvedValue(undefined),
}))
beforeEach(() => {
  vi.mocked(performProjectFolderAction).mockClear()
  vi.mocked(getBurnCheckTargetEvidence).mockReset()
  vi.mocked(openBurnCheckSample).mockReset()
  vi.mocked(copyPromptFixBurnCheckTarget).mockReset()
  vi.mocked(writeClipboardText).mockReset()
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

function openManualEvidence() {
  const button = screen.queryByRole("button", { name: /Show / })
  if (button) fireEvent.click(button)
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
    ).toBe("Estimated ~$8.20 in cache reads across 1 session, including helper requests.")
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
    ).toBe("Estimated ~$1.00 in cache reads across 2 occurrences, including helper requests.")
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
  it("shows the saved finding first and loads source details only when opened", async () => {
    vi.mocked(getBurnCheckTargetEvidence).mockResolvedValue({
      status: "available",
      items: [evidenceItem("observedAction", "the saved read")],
    })
    render(
      <BurnCheckTargetDetail
        target={target({
          finding: {
            ...target().finding,
            detector: "overExploring",
            observation: "Reads more of a file than the task needs.",
          },
          evidenceAvailable: true,
        })}
        reportRow
        refresh={() => undefined}
      />,
    )

    expect(screen.getByText("Reads more of a file than the task needs.")).toBeVisible()
    expect(screen.getByRole("button", { name: "Show reads under review" })).toHaveAttribute(
      "aria-expanded",
      "false",
    )
    expect(getBurnCheckTargetEvidence).not.toHaveBeenCalled()

    fireEvent.click(screen.getByRole("button", { name: "Show reads under review" }))
    expect(getBurnCheckTargetEvidence).toHaveBeenCalledOnce()
    expect(await screen.findByRole("button", { name: "Show returned content" })).toBeVisible()
  })

  it("replaces a collapsed excerpt preview with the full text when expanded", async () => {
    const fullText = `Start of evidence ${"more evidence ".repeat(40)} end of evidence`
    vi.mocked(getBurnCheckTargetEvidence).mockResolvedValue({
      status: "available",
      items: [evidenceItem("context", "task", { excerpt: fullText })],
    })
    render(
      <BurnCheckTargetDetail
        target={target({
          finding: { ...target().finding, detector: "scopeCreep" },
          evidenceAvailable: true,
        })}
        openEvidence
        refresh={() => undefined}
      />,
    )

    const preview = await screen.findByText(
      (_, element) =>
        element?.tagName === "PRE" &&
        element.textContent?.startsWith("Start of evidence") === true,
    )
    expect(preview.textContent).toMatch(/…$/)
    fireEvent.click(screen.getByText("Show full details"))

    expect(
      screen.getByText(
        (_, element) =>
          element?.tagName === "PRE" &&
          element.textContent?.includes("end of evidence") === true,
      ),
    ).toBeVisible()
    expect(
      screen.queryByText(
        (_, element) =>
          element?.tagName === "PRE" && element.textContent?.endsWith("…") === true,
      ),
    ).not.toBeInTheDocument()
  })

  it("clears unavailable excerpts while retaining the occurrence for a later validated refresh", async () => {
    const occurrence = (id: string) => ({
      findingId: id,
      status: "available" as const,
      items: [evidenceItem("observedAction", id)],
    })
    vi.mocked(getBurnCheckTargetEvidence)
      .mockResolvedValueOnce({ status: "available", items: [], occurrences: [occurrence("A")] })
      .mockResolvedValueOnce({ status: "unavailable", items: [] })
      .mockResolvedValueOnce({
        status: "available",
        items: [],
        occurrences: [occurrence("B"), occurrence("A")],
      })
    const current = target({
      finding: { ...target().finding, detector: "scopeCreep" },
      evidenceAvailable: true,
    })
    const view = render(
      <BurnCheckTargetDetail
        target={current}
        reportRow
        openEvidence
        refresh={() => undefined}
      />,
    )
    await screen.findByText("Text A")
    view.rerender(
      <BurnCheckTargetDetail
        target={{ ...current, actionId: "unavailable" }}
        reportRow
        openEvidence
        refresh={() => undefined}
      />,
    )
    await screen.findByText("Source details could not be verified.")
    expect(screen.queryByText("Text A")).not.toBeInTheDocument()
    view.rerender(
      <BurnCheckTargetDetail
        target={{ ...current, actionId: "validated" }}
        reportRow
        openEvidence
        refresh={() => undefined}
      />,
    )
    expect(await screen.findByText("Text A")).toBeVisible()
    expect(screen.queryByText("Text B")).not.toBeInTheDocument()
  })
  it("retains the occurrence while hidden and pauses revised evidence until the view resumes", async () => {
    const occurrence = (id: string) => ({
      findingId: id,
      status: "available" as const,
      items: [evidenceItem("observedAction", id)],
    })
    vi.mocked(getBurnCheckTargetEvidence)
      .mockResolvedValueOnce({ status: "available", items: [], occurrences: [occurrence("A")] })
      .mockResolvedValueOnce({
        status: "available",
        items: [],
        occurrences: [occurrence("B"), occurrence("A")],
      })
    const current = target({
      finding: { ...target().finding, detector: "scopeCreep" },
      evidenceAvailable: true,
    })
    const view = render(
      <BurnCheckTargetDetail
        target={current}
        reportRow
        openEvidence
        refresh={() => undefined}
      />,
    )
    const excerpt = await screen.findByText("Text A")
    view.rerender(
      <BurnCheckTargetDetail
        target={{ ...current, actionId: "revised" }}
        reportRow
        openEvidence
        evidenceActive={false}
        refresh={() => undefined}
      />,
    )
    expect(screen.getByText("Text A")).toBe(excerpt)
    expect(getBurnCheckTargetEvidence).toHaveBeenCalledOnce()
    view.rerender(
      <BurnCheckTargetDetail
        target={{ ...current, actionId: "revised" }}
        reportRow
        openEvidence
        refresh={() => undefined}
      />,
    )
    await act(async () => undefined)
    expect(screen.getByText("Text A")).toBeVisible()
    expect(screen.queryByText("Text B")).not.toBeInTheDocument()
    expect(getBurnCheckTargetEvidence).toHaveBeenCalledTimes(2)
  })
  it("renders the published skill contract with selected work and comparison-owned task evidence", async () => {
    vi.mocked(getBurnCheckTargetEvidence).mockResolvedValue(skillOpportunityEvidence)
    render(
      <BurnCheckTargetDetail
        target={target({
          finding: { ...target().finding, detector: "skillOpportunities" },
          evidenceAvailable: true,
        })}
        reportRow
        openEvidence
        refresh={() => undefined}
      />,
    )
    expect(
      await screen.findByText(skillOpportunityEvidence.comparison.explanation.text),
    ).toBeVisible()
    expect(
      screen.getByText("Review parser boundaries and test malformed records."),
    ).toBeVisible()
    expect(screen.getByText("pnpm test parser-boundaries")).toBeVisible()
    expect(screen.getByText("Parser boundary tests passed.")).toBeVisible()
    expect(screen.getByText("Requested task")).toBeVisible()
    expect(screen.getByText("Explain the parser fix.")).toBeVisible()
    expect(
      screen.queryByRole("button", { name: "Show source details" }),
    ).not.toBeInTheDocument()
  })
  it.each([
    "ignoredInstructions",
    "scopeCreep",
    "overExploring",
    "skillOpportunities",
  ] as const)(
    "keeps report evidence collapsed until the disclosure opens for %s",
    async (detector) => {
      vi.mocked(getBurnCheckTargetEvidence).mockResolvedValue({
        status: "available",
        items: [evidenceItem("observedAction", "work")],
      })
      render(
        <BurnCheckTargetDetail
          target={target({
            finding: { ...target().finding, detector },
            evidenceAvailable: true,
          })}
          reportRow
          refresh={() => undefined}
        />,
      )
      const disclosure = screen.getByRole("button", {
        name: `Show ${
          detector === "ignoredInstructions"
            ? "instruction and action"
            : detector === "scopeCreep"
              ? "task and work"
              : detector === "overExploring"
                ? "reads under review"
                : "skill and related work"
        }`,
      })
      expect(disclosure).toHaveAttribute("aria-expanded", "false")
      expect(getBurnCheckTargetEvidence).not.toHaveBeenCalled()
      fireEvent.click(disclosure)
      const excerpt = await screen.findByText("Text work")
      expect(excerpt.closest("[data-snapshot-action-id]")).not.toHaveClass("min-h-72")
      expect(disclosure).toHaveAttribute("aria-expanded", "true")
      expect(getBurnCheckTargetEvidence).toHaveBeenCalledOnce()
    },
  )

  it("renders repeated paths as independent paired reads with requested and returned extents", async () => {
    const items = [
      evidenceItem("context", "task", { excerpt: "Fix the login timeout." }),
      evidenceItem("observedAction", "request-1", { observedAtMs: 1000 }),
      evidenceItem("observedAction", "result-1", { excerpt: "Selected file contents" }),
      evidenceItem("observedAction", "request-2"),
    ]
    vi.mocked(getBurnCheckTargetEvidence).mockResolvedValue({
      status: "available",
      items,
      comparison: {
        reads: [
          {
            requestReference: "request-1",
            resultReference: "result-1",
            paths: ["src/login.ts"],
            requestedExtent: { unit: "lines", offset: 10, limit: 20, end_inclusive: null },
            returnedExtent: { unit: "lines", offset: 10, limit: null, end_inclusive: 18 },
            resultStatus: "success",
          },
          {
            requestReference: "request-2",
            paths: ["src/login.ts"],
            requestedExtent: {
              unit: "unknown",
              offset: null,
              limit: null,
              end_inclusive: null,
            },
          },
        ],
      },
    })
    const view = render(
      <BurnCheckTargetDetail
        target={target({
          finding: { ...target().finding, detector: "overExploring" },
          evidenceAvailable: true,
        })}
        reportRow
        openEvidence
        refresh={() => undefined}
      />,
    )
    expect(screen.queryByText("Fix the login timeout.")).not.toBeInTheDocument()
    expect(await screen.findByRole("heading", { name: "Read under review" })).toBeVisible()
    const reads = screen.getByRole("region", { name: "Reads under review" })
    expect(within(reads).getAllByText("src/login.ts")).toHaveLength(2)
    expect(view.container.querySelectorAll("time")).toHaveLength(1)
    expect(screen.getByText("Requested: line 10 · limit 20 lines")).toBeVisible()
    expect(screen.getByText("Returned: line 10–18")).toBeVisible()
    expect(screen.getByText("Read request")).toBeVisible()
    const content = screen.getByText("Selected file contents")
    expect(content).not.toBeVisible()
    const button = screen.getByRole("button", { name: "Show returned content" })
    expect(button).toHaveAttribute("aria-expanded", "false")
    act(() => button.focus())
    fireEvent.click(button)
    expect(content).toBeVisible()
    expect(screen.getByRole("button", { name: "Hide returned content" })).toHaveAttribute(
      "aria-expanded",
      "true",
    )
    fireEvent.click(screen.getByRole("button", { name: "Hide returned content" }))
    expect(content).not.toBeVisible()
  })

  it.each(["proposal", "attempt", "recorded"] as const)(
    "labels %s work from the occurrence contract",
    async (observationKind) => {
      vi.mocked(getBurnCheckTargetEvidence).mockResolvedValue({
        status: "available",
        items: [evidenceItem("observedAction", "work")],
        comparison: { observationKind, reads: [] },
      })
      render(
        <BurnCheckTargetDetail
          target={target({
            finding: { ...target().finding, detector: "scopeCreep" },
            evidenceAvailable: true,
          })}
          reportRow
          openEvidence
          refresh={() => undefined}
        />,
      )
      expect(
        await screen.findByText(
          observationKind === "proposal"
            ? "Proposed work"
            : observationKind === "attempt"
              ? "Attempted work"
              : "Recorded work",
        ),
      ).toBeVisible()
      expect(screen.queryByText("Work performed")).not.toBeInTheDocument()
    },
  )

  it("refreshes explanation and excerpts atomically when occurrences reorder or disappear", async () => {
    const occurrence = (id: string) => ({
      findingId: id,
      status: "available" as const,
      items: [
        evidenceItem("instruction", `${id}-task`),
        evidenceItem("observedAction", `${id}-work`),
      ],
      comparison: {
        reads: [],
        explanation: {
          version: 1 as const,
          relationship: "separateObjective",
          text: `Specific ${id} explanation.`,
          references: [`${id}-task`, `${id}-work`],
        },
      },
    })
    vi.mocked(getBurnCheckTargetEvidence)
      .mockResolvedValueOnce({ status: "available", items: [], occurrences: [occurrence("A")] })
      .mockResolvedValueOnce({
        status: "available",
        items: [],
        occurrences: [occurrence("B"), occurrence("A")],
      })
      .mockResolvedValueOnce({ status: "available", items: [], occurrences: [occurrence("B")] })
    const current = target({
      finding: { ...target().finding, detector: "scopeCreep" },
      evidenceAvailable: true,
    })
    const view = render(
      <BurnCheckTargetDetail
        target={current}
        reportRow
        openEvidence
        refresh={() => undefined}
      />,
    )
    expect(await screen.findByText("Specific A explanation.")).toBeVisible()
    view.rerender(
      <BurnCheckTargetDetail
        target={{ ...current, actionId: "second" }}
        reportRow
        openEvidence
        refresh={() => undefined}
      />,
    )
    await act(async () => undefined)
    expect(screen.getByText("Specific A explanation.")).toBeVisible()
    expect(screen.getByText("Text A-work")).toBeVisible()
    expect(screen.queryByText("Specific B explanation.")).not.toBeInTheDocument()
    view.rerender(
      <BurnCheckTargetDetail
        target={{ ...current, actionId: "third" }}
        reportRow
        openEvidence
        refresh={() => undefined}
      />,
    )
    expect(await screen.findByText("Specific B explanation.")).toBeVisible()
    expect(screen.getByText("Text B-work")).toBeVisible()
    expect(screen.queryByText("Specific A explanation.")).not.toBeInTheDocument()
    expect(screen.queryByText("Text A-work")).not.toBeInTheDocument()
  })

  const evidenceItem = (
    label: BurnCheckTargetEvidencePayload["items"][number]["label"],
    reference: string,
    overrides: Partial<BurnCheckTargetEvidencePayload["items"][number]> = {},
  ): BurnCheckTargetEvidencePayload["items"][number] => ({
    label,
    reference,
    sourceLabel: `Source ${reference}`,
    excerpt: `Text ${reference}`,
    observedAtMs: null,
    startLine: null,
    endLine: null,
    explanation: "",
    limitation: null,
    ...overrides,
  })
  const observedProof: NonNullable<BurnCheckTargetEvidencePayload["decisionProof"]> = {
    contrast: "The recorded tool request conflicts with this instruction.",
    prerequisite: "not_required",
    citations: [
      { claim: "rule_requirement", source_ids: ["rule"] },
      { claim: "anchored_action", source_ids: ["action"] },
      { claim: "observed_context", source_ids: ["context"] },
    ],
    coverage: {
      source_complete: true,
      selected_history_complete: true,
      read_request_inventory_complete: false,
      results_excluded: false,
      user_authority_excluded: false,
      limitations: ["Only selected events support this comparison."],
    },
    contextRevision: "private-revision",
  }

  it.each([
    "ignoredInstructions",
    "scopeCreep",
    "overExploring",
    "skillOpportunities",
  ] as const)("shows compact loading while the initial %s evidence loads", async (detector) => {
    let resolveInitial!: (value: BurnCheckTargetEvidencePayload) => void
    let resolveRefresh!: (value: BurnCheckTargetEvidencePayload) => void
    vi.mocked(getBurnCheckTargetEvidence)
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveInitial = resolve
          }),
      )
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveRefresh = resolve
          }),
      )
    const current = target({
      finding: { ...target().finding, detector },
      display: {
        ...target().display,
        instructionTitle: "Git Style",
        resourceIdentity: "Quality Review",
      },
      evidenceAvailable: true,
    })
    const view = render(
      <BurnCheckTargetDetail target={current} refresh={() => undefined} openEvidence />,
    )
    const loading = screen.getByText("Loading source details…")
    expect(loading.closest("[aria-busy=true]")).not.toHaveClass("min-h-72")
    expect(view.container.querySelector("[data-placeholder]")).toBeNull()
    expect(screen.queryByText("Loading the instruction…")).not.toBeInTheDocument()
    expect(screen.queryByRole("heading", { name: "Details" })).not.toBeInTheDocument()

    await act(async () =>
      resolveInitial({
        status: "available",
        items: [
          {
            ...evidenceItem("instruction", "rule"),
            limitation: "Current source does not prove historical activation.",
          },
          evidenceItem("observedAction", "action"),
        ],
        occurrences: [
          {
            findingId: "main",
            status: "available",
            items: [
              {
                ...evidenceItem("instruction", "rule"),
                limitation: "Current source does not prove historical activation.",
              },
              evidenceItem("observedAction", "action"),
            ],
          },
        ],
      }),
    )
    const excerpt = screen.getByText("Text action")
    expect(excerpt.closest("[data-snapshot-action-id]")).not.toBeNull()
    expect(
      screen.queryByRole("status", { name: "Loading source details" }),
    ).not.toBeInTheDocument()
    expect(view.container.querySelector("[data-placeholder]")).toBeNull()
    expect(screen.queryByLabelText("About this evidence")).not.toBeInTheDocument()
    expect(
      screen.queryByText("Current source does not prove historical activation."),
    ).not.toBeInTheDocument()

    view.rerender(
      <BurnCheckTargetDetail
        target={{ ...current, actionId: "refreshed-action" }}
        refresh={() => undefined}
        openEvidence
      />,
    )
    expect(screen.getByText("Text action")).toBe(excerpt)
    expect(view.container.querySelector("[data-placeholder]")).toBeNull()
    expect(screen.queryByRole("heading", { name: "Details" })).not.toBeInTheDocument()
    await act(async () => resolveRefresh({ status: "unavailable", items: [] }))
    expect(screen.queryByText("Text action")).not.toBeInTheDocument()
  })

  it("renders observed_context with exact supporting-event navigation", async () => {
    vi.mocked(getBurnCheckTargetEvidence).mockResolvedValue({
      status: "available",
      decisionProof: observedProof,
      items: [
        evidenceItem("instruction", "rule"),
        evidenceItem("observedAction", "action"),
        evidenceItem("context", "context"),
      ],
    })
    render(
      <BurnCheckTargetDetail
        target={target({
          finding: { ...target().finding, detector: "ignoredInstructions" },
          evidenceAvailable: true,
        })}
        refresh={() => undefined}
        openEvidence
      />,
    )
    expect(await screen.findByText("What happened")).toBeVisible()
    expect(
      screen.queryByRole("region", { name: "Assessment decision" }),
    ).not.toBeInTheDocument()
    expect(screen.getByText("Text context")).toBeVisible()
    expect(screen.queryByText("private-revision")).not.toBeInTheDocument()
  })

  it.each(["available", "unavailable"] as const)(
    "hides %s proof with missing citation targets",
    async (status) => {
      vi.mocked(getBurnCheckTargetEvidence).mockResolvedValue({
        status,
        decisionProof: observedProof,
        items: [evidenceItem("instruction", "rule"), evidenceItem("observedAction", "action")],
      })
      render(
        <BurnCheckTargetDetail
          target={target({
            finding: {
              ...target().finding,
              detector: "ignoredInstructions",
              decisionProof: observedProof,
            },
            evidenceAvailable: true,
          })}
          refresh={() => undefined}
          openEvidence
        />,
      )
      await act(async () => undefined)
      expect(
        screen.queryByRole("region", { name: "Assessment decision" }),
      ).not.toBeInTheDocument()
      expect(screen.queryByText(observedProof.contrast)).not.toBeInTheDocument()
    },
  )

  it.each(["", "Instruction text unavailable."])(
    "hides proof whose requirement text is %j",
    async (excerpt) => {
      vi.mocked(getBurnCheckTargetEvidence).mockResolvedValue({
        status: "available",
        decisionProof: observedProof,
        items: [
          { ...evidenceItem("instruction", "rule"), excerpt },
          evidenceItem("observedAction", "action"),
          evidenceItem("context", "context"),
        ],
      })
      render(
        <BurnCheckTargetDetail
          target={target({
            finding: { ...target().finding, detector: "ignoredInstructions" },
            evidenceAvailable: true,
          })}
          refresh={() => undefined}
          openEvidence
        />,
      )
      expect(await screen.findByText("Text action")).toBeVisible()
      expect(
        screen.queryByRole("region", { name: "Assessment decision" }),
      ).not.toBeInTheDocument()
    },
  )

  it("refreshes manually opened evidence, retains it after failure, and replaces it with an unavailable result", async () => {
    let rejectRefresh!: (reason: Error) => void
    let resolveRetry!: (value: BurnCheckTargetEvidencePayload) => void
    vi.mocked(getBurnCheckTargetEvidence)
      .mockResolvedValueOnce({
        status: "available",
        items: [
          evidenceItem("instruction", "rule"),
          evidenceItem("observedAction", "action"),
          evidenceItem("context", "context"),
        ],
        decisionProof: observedProof,
      })
      .mockImplementationOnce(
        () =>
          new Promise((_resolve, reject) => {
            rejectRefresh = reject
          }),
      )
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveRetry = resolve
          }),
      )
    const current = target({
      finding: { ...target().finding, detector: "ignoredInstructions" },
      evidenceAvailable: true,
    })
    const view = render(<BurnCheckTargetDetail target={current} refresh={() => undefined} />)
    openManualEvidence()
    const oldExcerpt = await screen.findByText("Text action")
    view.rerender(
      <BurnCheckTargetDetail
        target={{ ...current, actionId: "refreshed-action" }}
        refresh={() => undefined}
      />,
    )
    expect(getBurnCheckTargetEvidence).toHaveBeenLastCalledWith("refreshed-action")
    expect(screen.getByText("Text action")).toBe(oldExcerpt)
    await act(async () => rejectRefresh(new Error("offline")))
    expect(screen.getByRole("alert")).toHaveTextContent(
      "Could not update source details. Earlier details remain available.",
    )
    expect(screen.getByText("Text action")).toBe(oldExcerpt)
    expect(screen.queryAllByRole("link")).toHaveLength(0)
    fireEvent.click(screen.getByRole("button", { name: "Retry" }))
    expect(screen.getByText("Text action")).toBe(oldExcerpt)
    await act(async () => resolveRetry({ status: "unavailable", items: [] }))
    expect(screen.queryByText("Text action")).not.toBeInTheDocument()
    expect(screen.getByText("Source details could not be verified.")).toBeVisible()
    expect(screen.queryByRole("alert")).not.toBeInTheDocument()
    expect(screen.queryByText(/Showing previous evidence/)).not.toBeInTheDocument()
  })

  it.each([
    ["unrelated_files", "The assessed reads included files unrelated to the work."],
    ["excessive_file_breadth", "The assessed work read more files than it needed."],
    ["excessive_within_file_reading", "The assessed work read more of a file than it needed."],
  ] as const)(
    "binds the %s explanation to task, request, and result citations",
    async (overExploringReason, contrast) => {
      vi.mocked(getBurnCheckTargetEvidence).mockResolvedValue({
        status: "available",
        items: [
          evidenceItem("context", "task"),
          evidenceItem("observedAction", "request"),
          evidenceItem("observedAction", "result"),
        ],
      })
      render(
        <BurnCheckTargetDetail
          target={target({
            finding: { ...target().finding, detector: "overExploring", overExploringReason },
            evidenceAvailable: true,
          })}
          refresh={() => undefined}
          openEvidence
        />,
      )
      expect(screen.queryByText(contrast)).not.toBeInTheDocument()
      expect(
        screen.queryByRole("region", { name: "Assessment decision" }),
      ).not.toBeInTheDocument()
      expect(await screen.findByText("Text request")).toBeVisible()
      expect(screen.getByRole("button", { name: "Show returned content" })).toBeVisible()
      expect(screen.getByText("Text result")).not.toBeVisible()
    },
  )

  it.each([
    "ignoredInstructions",
    "scopeCreep",
    "overExploring",
    "skillOpportunities",
  ] as const)(
    "retains the loaded %s occurrence during deferred snapshot refreshes",
    async (detector) => {
      const items = [
        evidenceItem("instruction", "rule"),
        evidenceItem("observedAction", "action"),
        evidenceItem("context", "context"),
      ]
      let resolveSuperseded!: (value: BurnCheckTargetEvidencePayload) => void
      let resolveCurrent!: (value: BurnCheckTargetEvidencePayload) => void
      vi.mocked(getBurnCheckTargetEvidence)
        .mockResolvedValueOnce({
          status: "available",
          items,
          decisionProof: observedProof,
          occurrences: [
            {
              findingId: "main-occurrence",
              status: "available",
              items,
              decisionProof: observedProof,
            },
          ],
        })
        .mockImplementationOnce(
          () =>
            new Promise((resolve) => {
              resolveSuperseded = resolve
            }),
        )
        .mockImplementationOnce(
          () =>
            new Promise((resolve) => {
              resolveCurrent = resolve
            }),
        )
      const current = target({
        finding: { ...target().finding, detector, overExploringReason: "unrelated_files" },
        evidenceAvailable: true,
      })
      const view = render(
        <BurnCheckTargetDetail target={current} refresh={() => undefined} openEvidence />,
      )
      const oldExcerpt = await screen.findByText("Text action")
      const oldDestination =
        detector === "ignoredInstructions" ? oldExcerpt : oldExcerpt.closest("li")!
      const oldCitationId = oldDestination.id
      expect(screen.queryAllByRole("link")).toHaveLength(0)

      view.rerender(
        <BurnCheckTargetDetail
          target={{ ...current, actionId: "superseded-action" }}
          refresh={() => undefined}
          openEvidence
        />,
      )
      expect(screen.getByText("Text action")).toBe(oldExcerpt)
      expect(oldDestination.id).toBe(oldCitationId)
      expect(oldDestination.closest("[data-snapshot-action-id]")).toHaveAttribute(
        "data-snapshot-action-id",
        "action-fresh",
      )
      expect(oldDestination.closest("[aria-busy]")).toHaveAttribute("aria-busy", "true")
      expect(screen.queryAllByRole("link")).toHaveLength(0)
      view.rerender(
        <BurnCheckTargetDetail
          target={{ ...current, actionId: "superseded-action" }}
          refresh={() => undefined}
          openEvidence
        />,
      )
      expect(getBurnCheckTargetEvidence).toHaveBeenCalledTimes(2)
      expect(screen.queryAllByRole("link")).toHaveLength(0)

      view.rerender(
        <BurnCheckTargetDetail
          target={{ ...current, actionId: "current-action" }}
          refresh={() => undefined}
          openEvidence
        />,
      )
      await act(async () =>
        resolveSuperseded({
          status: "available",
          items: [{ ...items[1]!, excerpt: "Superseded work" }],
        }),
      )
      expect(screen.queryByText("Superseded work")).not.toBeInTheDocument()
      expect(screen.getByText("Text action")).toBe(oldExcerpt)

      const newItems = items.map((item) =>
        item.label === "observedAction" ? { ...item, excerpt: "Updated main work" } : item,
      )
      await act(async () =>
        resolveCurrent({
          status: "available",
          items: [{ ...items[1]!, excerpt: "Different first occurrence" }],
          occurrences: [
            {
              findingId: "other-occurrence",
              status: "available",
              items: [{ ...items[1]!, excerpt: "Different first occurrence" }],
            },
            {
              findingId: "main-occurrence",
              status: "available",
              items: newItems,
              decisionProof: observedProof,
            },
          ],
        }),
      )
      const updatedExcerpt = screen.getByText("Updated main work")
      expect(screen.queryByText("Different first occurrence")).not.toBeInTheDocument()
      expect(screen.queryByText("Text action")).not.toBeInTheDocument()
      expect(updatedExcerpt.closest("[data-snapshot-action-id]")).toHaveAttribute(
        "data-snapshot-action-id",
        "current-action",
      )
      expect(updatedExcerpt.closest("[aria-busy]")).toHaveAttribute("aria-busy", "false")
      expect(screen.getByText("Updated main work")).toBeVisible()
      expect(screen.queryAllByRole("link")).toHaveLength(0)
      expect(getBurnCheckTargetEvidence).toHaveBeenCalledTimes(3)
    },
  )

  it.each([
    "ignoredInstructions",
    "scopeCreep",
    "overExploring",
    "skillOpportunities",
  ] as const)(
    "clears retained %s evidence when a different finding replaces a pending refresh",
    async (detector) => {
      let resolveOld!: (value: BurnCheckTargetEvidencePayload) => void
      let resolveNew!: (value: BurnCheckTargetEvidencePayload) => void
      vi.mocked(getBurnCheckTargetEvidence)
        .mockResolvedValueOnce({
          status: "available",
          items: [evidenceItem("observedAction", "old")],
        })
        .mockImplementationOnce(
          () =>
            new Promise((resolve) => {
              resolveOld = resolve
            }),
        )
        .mockImplementationOnce(
          () =>
            new Promise((resolve) => {
              resolveNew = resolve
            }),
        )
      const current = target({
        finding: { ...target().finding, detector },
        evidenceAvailable: true,
      })
      const view = render(
        <BurnCheckTargetDetail target={current} refresh={() => undefined} openEvidence />,
      )
      await screen.findByText("Text old")
      view.rerender(
        <BurnCheckTargetDetail
          target={{ ...current, actionId: "refresh-action" }}
          refresh={() => undefined}
          openEvidence
        />,
      )
      expect(screen.getByText("Text old")).toBeVisible()
      view.rerender(
        <BurnCheckTargetDetail
          target={{ ...current, findingId: "different-finding", actionId: "different-action" }}
          refresh={() => undefined}
          openEvidence
        />,
      )
      expect(screen.queryByText("Text old")).not.toBeInTheDocument()
      await act(async () =>
        resolveOld({
          status: "available",
          items: [evidenceItem("observedAction", "late-old")],
        }),
      )
      expect(screen.queryByText("Text late-old")).not.toBeInTheDocument()
      await act(async () =>
        resolveNew({ status: "available", items: [evidenceItem("observedAction", "new")] }),
      )
      expect(screen.getByText("Text new")).toBeVisible()
      expect(screen.getByText("Text new").closest("[data-snapshot-action-id]")).toHaveAttribute(
        "data-snapshot-action-id",
        "different-action",
      )
    },
  )

  it.each([
    "ignoredInstructions",
    "scopeCreep",
    "overExploring",
    "skillOpportunities",
  ] as const)("withdraws stale %s evidence and rejects its late response", async (detector) => {
    let resolveOld!: (value: BurnCheckTargetEvidencePayload) => void
    vi.mocked(getBurnCheckTargetEvidence)
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveOld = resolve
          }),
      )
      .mockResolvedValueOnce({ status: "unavailable", items: [] })
    const current = target({
      finding: { ...target().finding, detector },
      evidenceAvailable: true,
    })
    const view = render(
      <BurnCheckTargetDetail target={current} refresh={() => undefined} openEvidence />,
    )
    view.rerender(
      <BurnCheckTargetDetail
        target={{ ...current, actionId: "new-action" }}
        refresh={() => undefined}
        openEvidence
      />,
    )
    await act(async () =>
      resolveOld({
        status: "available",
        decisionProof: observedProof,
        items: [
          evidenceItem("instruction", "rule"),
          evidenceItem("observedAction", "action"),
          evidenceItem("context", "context"),
        ],
      }),
    )
    expect(getBurnCheckTargetEvidence).toHaveBeenLastCalledWith("new-action")
    expect(screen.queryByText("Text action")).not.toBeInTheDocument()
    expect(
      screen.queryByRole("region", { name: "Assessment decision" }),
    ).not.toBeInTheDocument()
  })

  it.each(["scopeCreep", "skillOpportunities"] as const)(
    "links every %s supporting citation without inventing a decisive excerpt",
    async (detector) => {
      vi.mocked(getBurnCheckTargetEvidence).mockResolvedValue({
        status: "available",
        items: [
          evidenceItem("instruction", "requirement"),
          evidenceItem("observedAction", "work"),
          evidenceItem("context", "support-1"),
          evidenceItem("context", "support-2"),
        ],
      })
      render(
        <BurnCheckTargetDetail
          target={target({
            finding: { ...target().finding, detector },
            evidenceAvailable: true,
          })}
          refresh={() => undefined}
          openEvidence
        />,
      )
      expect(
        screen.queryByRole("region", { name: "Assessment decision" }),
      ).not.toBeInTheDocument()
      expect(await screen.findByText("Recorded work")).toBeVisible()
      expect(screen.queryByText("Suggestion")).not.toBeInTheDocument()
      expect(screen.queryAllByRole("link")).toHaveLength(0)
      expect(screen.queryByText(/chain.of.thought/i)).not.toBeInTheDocument()
    },
  )

  it.each(["scopeCreep", "overExploring", "skillOpportunities"] as const)(
    "keeps %s evidence limits beside the card title even without a complete comparison",
    async (detector) => {
      const limitation = "Only selected events are available."
      vi.mocked(getBurnCheckTargetEvidence).mockResolvedValue({
        status: "available",
        items: [{ ...evidenceItem("observedAction", "work"), limitation }],
      })
      render(
        <BurnCheckTargetDetail
          target={target({
            finding: { ...target().finding, detector },
            evidenceAvailable: true,
          })}
          refresh={() => undefined}
          openEvidence
        />,
      )
      await screen.findByText("Text work")
      expect(screen.queryByLabelText("About this evidence")).not.toBeInTheDocument()
      expect(screen.queryByRole("heading", { name: "Details" })).not.toBeInTheDocument()
      expect(screen.queryByText(limitation)).not.toBeInTheDocument()
    },
  )

  it.each(["scopeCreep", "overExploring", "skillOpportunities"] as const)(
    "keeps incomplete %s citations as excerpts without a decisive explanation",
    async (detector) => {
      vi.mocked(getBurnCheckTargetEvidence).mockResolvedValue({
        status: "available",
        items: [evidenceItem("observedAction", "work")],
      })
      render(
        <BurnCheckTargetDetail
          target={target({
            finding: { ...target().finding, detector },
            evidenceAvailable: true,
          })}
          refresh={() => undefined}
          openEvidence
        />,
      )
      expect(await screen.findByText("Text work")).toBeVisible()
      expect(screen.getByText("Text work")).toBeVisible()
      expect(
        screen.queryByRole("region", { name: "Assessment decision" }),
      ).not.toBeInTheDocument()
    },
  )

  it("copies a published Scope Creep prompt for future work", async () => {
    const prompt =
      "For future work, stay within the agreed task and ask for approval before adding work."
    vi.mocked(copyPromptFixBurnCheckTarget).mockResolvedValue({
      outcome: "promptReady",
      prompt,
      watch: null,
    })
    render(
      <BurnCheckTargetDetail
        target={target({
          finding: {
            ...target().finding,
            detector: "scopeCreep",
            agent: "opencode",
            sourceFormat: "openCodeSqliteV2",
          },
          promptFix: { status: "available" },
        })}
        refresh={() => undefined}
      />,
    )
    fireEvent.click(screen.getByRole("button", { name: "Copy fix prompt" }))
    expect(await screen.findByRole("button", { name: "Copied" })).toBeVisible()
    expect(copyPromptFixBurnCheckTarget).toHaveBeenCalledWith("action-fresh")
    expect(writeClipboardText).toHaveBeenCalledWith(prompt)
  })
  it("renders recorded task and work citations with future guidance", async () => {
    vi.mocked(getBurnCheckTargetEvidence).mockResolvedValue({
      status: "available",
      items: [
        {
          label: "instruction",
          sourceLabel: "Recorded user task",
          reference: "task-1",
          observedAtMs: 1000,
          startLine: null,
          endLine: null,
          excerpt: "Fix the login timeout.",
          explanation: "Latest recorded root scope",
          limitation: null,
        },
        {
          label: "observedAction",
          sourceLabel: "Recorded edit",
          reference: "work-1",
          observedAtMs: 2000,
          startLine: null,
          endLine: null,
          excerpt: "Added an unrelated dashboard.",
          explanation: "Work outside the agreed task",
          limitation: null,
        },
        {
          label: "context",
          sourceLabel: "Recorded user approval",
          reference: "approval-1",
          observedAtMs: 3000,
          startLine: null,
          endLine: null,
          excerpt: "Approve only the login fix.",
          explanation: "Approval evidence",
          limitation: null,
        },
      ],
    })
    const current = target({
      finding: {
        ...target().finding,
        detector: "scopeCreep",
        agent: "opencode",
        sourceFormat: "openCodeSqliteV2",
      },
      evidenceAvailable: true,
    })
    render(
      <BurnCheckTargetDetail
        target={current}
        refresh={() => undefined}
        reportRow
        openEvidence
      />,
    )
    expect(await screen.findByText("Fix the login timeout.")).toBeVisible()
    expect(screen.getByText("Requested task · Recorded user task")).toBeVisible()
    expect(screen.getByText("Recorded work")).toBeVisible()
    expect(screen.getByText("Added an unrelated dashboard.")).toBeVisible()
    expect(screen.queryByText("Work outside the agreed task")).not.toBeInTheDocument()
    expect(
      screen.queryByText(
        "Keep future work within the agreed task. Ask for approval before adding work.",
      ),
    ).not.toBeInTheDocument()
    expect(screen.getByText("Approve only the login fix.")).toBeVisible()
    expect(screen.queryByRole("button", { name: "Show context" })).not.toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "Fix" })).not.toBeInTheDocument()
  })
  it.each([
    ["unrelated_files", "The assessed reads included files unrelated to the work."],
    ["excessive_file_breadth", "The assessed work read more files than it needed."],
    ["excessive_within_file_reading", "The assessed work read more of a file than it needed."],
  ] as const)("renders the bounded %s reason", (overExploringReason, copy) => {
    render(
      <BurnCheckTargetDetail
        target={target({
          finding: { ...target().finding, detector: "overExploring", overExploringReason },
        })}
        refresh={() => undefined}
        reportRow
      />,
    )
    expect(screen.queryByText(copy)).not.toBeInTheDocument()
    expect(
      screen.queryByText("Read only the files and sections needed for future work."),
    ).not.toBeInTheDocument()
  })
  it("reloads skill evidence after an action revision and rejects the old response", async () => {
    let resolveOld!: (value: Awaited<ReturnType<typeof getBurnCheckTargetEvidence>>) => void
    vi.mocked(getBurnCheckTargetEvidence)
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveOld = resolve
          }),
      )
      .mockResolvedValueOnce({ status: "unavailable", items: [] })
    const current = target({
      finding: { ...target().finding, detector: "skillOpportunities" },
      evidenceAvailable: true,
    })
    const view = render(
      <BurnCheckTargetDetail target={current} refresh={() => undefined} openEvidence />,
    )
    view.rerender(
      <BurnCheckTargetDetail
        target={{ ...current, actionId: "revised-action" }}
        refresh={() => undefined}
        openEvidence
      />,
    )
    expect(await screen.findByText("Source details could not be verified.")).toBeVisible()
    await act(async () =>
      resolveOld({
        status: "available",
        items: [
          {
            label: "instruction",
            sourceLabel: "old-skill",
            reference: "old-reference",
            observedAtMs: null,
            startLine: null,
            endLine: null,
            excerpt: "Old skill description.",
            explanation: "",
            limitation: null,
          },
        ],
      }),
    )
    expect(getBurnCheckTargetEvidence).toHaveBeenNthCalledWith(2, "revised-action")
    expect(screen.queryByText("Old skill description.")).not.toBeInTheDocument()
  })

  it("renders current skill descriptions, recorded work, and optional time limits with a future prompt", async () => {
    const limit =
      "Skill creation time is unknown. Current inventory does not prove past access."
    vi.mocked(getBurnCheckTargetEvidence).mockResolvedValue({
      status: "available",
      items: [
        {
          label: "instruction",
          sourceLabel: "parser-review",
          reference: "skill-citation",
          observedAtMs: null,
          startLine: null,
          endLine: null,
          excerpt: "Review parser boundaries and test malformed records.",
          explanation: "Current skill description.",
          limitation: limit,
        },
        {
          label: "observedAction",
          sourceLabel: "Recorded edit",
          reference: "work-citation",
          observedAtMs: null,
          startLine: null,
          endLine: null,
          excerpt: "Added malformed-record parser tests.",
          explanation: "Work cited by this assessment.",
          limitation: "No matching skill use appears in the selected session evidence.",
        },
      ],
    })
    const prompt =
      "For future parser work, find matching installed skills and use them when useful. This does not repair past work."
    vi.mocked(copyPromptFixBurnCheckTarget).mockResolvedValue({
      outcome: "promptReady",
      prompt,
      watch: null,
    })
    const refresh = vi.fn()
    const view = render(
      <BurnCheckTargetDetail
        target={target({
          finding: {
            ...target().finding,
            detector: "skillOpportunities",
            agent: "opencode",
            sourceFormat: "openCodeSqliteV2",
          },
          display: {
            ...target().display,
            resourceKind: "skill",
            resourceIdentity: "parser-review",
            quantity: null,
            quantityUnit: null,
            verificationLimit: "currentEvidenceCannotProveFix",
          },
          evidenceAvailable: true,
          promptFix: { status: "available" },
        })}
        refresh={refresh}
      />,
    )
    expect(
      screen.queryByText("Use matching skills for similar future work."),
    ).not.toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "Fix" })).not.toBeInTheDocument()
    openManualEvidence()
    expect(
      await screen.findByText("Review parser boundaries and test malformed records."),
    ).toBeVisible()
    expect(screen.getByText("Relevant skill · parser-review")).toBeVisible()
    expect(screen.getByText("Recorded work")).toBeVisible()
    expect(screen.getByText("Added malformed-record parser tests.")).toBeVisible()
    expect(view.container.querySelector("time")).toBeNull()
    expect(screen.queryByText(limit)).not.toBeInTheDocument()
    expect(screen.queryByLabelText("About this evidence")).not.toBeInTheDocument()
    expect(screen.queryByText(limit)).not.toBeInTheDocument()
    expect(screen.queryByText("skill-citation")).not.toBeInTheDocument()
    expect(screen.queryByText("work-citation")).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Copy fix prompt" }))
    expect(await screen.findByRole("button", { name: "Copied" })).toBeVisible()
    expect(copyPromptFixBurnCheckTarget).toHaveBeenCalledWith("action-fresh")
    expect(writeClipboardText).toHaveBeenCalledWith(prompt)
    expect(refresh).toHaveBeenCalledOnce()
    expect(view.container).not.toHaveTextContent(
      /savings|historically available|could have used/i,
    )
  })

  it("keeps unknown skill evidence unavailable and refreshes a stale prompt", async () => {
    vi.mocked(getBurnCheckTargetEvidence).mockResolvedValue({
      status: "unavailable",
      items: [],
    })
    vi.mocked(copyPromptFixBurnCheckTarget).mockResolvedValue({ outcome: "stale" })
    const refresh = vi.fn()
    render(
      <BurnCheckTargetDetail
        target={target({
          finding: { ...target().finding, detector: "skillOpportunities" },
          evidenceAvailable: true,
          promptFix: { status: "available" },
        })}
        refresh={refresh}
        openEvidence
      />,
    )
    expect(await screen.findByText("Source details could not be verified.")).toBeVisible()
    fireEvent.click(screen.getByRole("button", { name: "Copy fix prompt" }))
    expect(await screen.findByRole("alert")).toHaveTextContent("Checking the current change.")
    expect(refresh).toHaveBeenCalledOnce()
    expect(writeClipboardText).not.toHaveBeenCalled()
  })

  it("renders a validated deterministic decision and citation summary", async () => {
    vi.mocked(getBurnCheckTargetEvidence).mockResolvedValue({
      status: "available",
      decisionProof: {
        contrast:
          "The selected earlier events and this action conflict with the prerequisite rule.",
        prerequisite: "selected_history_conflict",
        citations: [
          { claim: "rule_requirement", source_ids: ["rule-id"] },
          { claim: "anchored_action", source_ids: ["action-id"] },
          { claim: "prerequisite_contrast", source_ids: ["action-id", "earlier-event"] },
        ],
        coverage: {
          source_complete: true,
          selected_history_complete: true,
          read_request_inventory_complete: true,
          results_excluded: true,
          user_authority_excluded: true,
          limitations: [],
        },
        contextRevision: "revision-digest",
      },
      items: [
        {
          label: "instruction",
          sourceLabel: "AGENTS.md · Release",
          reference: "rule-id",
          observedAtMs: null,
          startLine: 1,
          endLine: 1,
          excerpt: "Request validation before publishing.",
          explanation: "",
          limitation: null,
        },
        {
          label: "observedAction",
          sourceLabel: "Session action",
          reference: "action-id",
          observedAtMs: null,
          startLine: null,
          endLine: null,
          excerpt: "Published without validation.",
          explanation: "",
          limitation: null,
        },
        {
          label: "context",
          sourceLabel: "Earlier validation request",
          reference: "earlier-event",
          observedAtMs: null,
          startLine: null,
          endLine: null,
          excerpt: "Request the required validation.",
          explanation: "Selected prerequisite event.",
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
    openManualEvidence()
    expect(await screen.findByText("What happened")).toBeVisible()
    expect(
      screen.queryByRole("region", { name: "Assessment decision" }),
    ).not.toBeInTheDocument()
    expect(screen.queryAllByRole("link")).toHaveLength(0)
    expect(screen.getAllByText(/AGENTS\.md · Release/)).toHaveLength(1)
    expect(screen.queryByLabelText("About this evidence")).not.toBeInTheDocument()
    expect(screen.getByText("Request validation before publishing.")).toBeInTheDocument()
    expect(screen.getByText("Published without validation.")).toBeInTheDocument()
    expect(screen.queryByText("revision-digest")).not.toBeInTheDocument()
    expect(screen.queryByText("earlier-event")).not.toBeInTheDocument()
    expect(screen.getByText("Request the required validation.")).toBeVisible()
  })

  it("keeps legacy evidence rendering when no decision proof is saved", async () => {
    vi.mocked(getBurnCheckTargetEvidence).mockResolvedValue({
      status: "available",
      items: [
        {
          label: "observedAction",
          sourceLabel: "Session action",
          reference: "action-id",
          observedAtMs: null,
          startLine: null,
          endLine: null,
          excerpt: "Used a blocked command.",
          explanation: "Saved action text that Antiburn compared with the instruction.",
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
    openManualEvidence()
    expect(await screen.findByText("Used a blocked command.")).toBeInTheDocument()
    expect(
      screen.queryByText(/Saved action text that Antiburn compared/),
    ).not.toBeInTheDocument()
    expect(
      screen.queryByRole("region", { name: "Assessment decision" }),
    ).not.toBeInTheDocument()
  })

  it("renders the main occurrence once with ordered context and hover-only limits", async () => {
    const item = {
      label: "observedAction" as const,
      sourceLabel: "Session action",
      reference: "anchor-1",
      observedAtMs: 1000,
      startLine: null,
      endLine: null,
      excerpt: "command\n  --flag",
      explanation: "Saved action text used for this comparison.",
      limitation: "Some nearby context is no longer available.",
    }
    const items = [
      item,
      ...["first", "second"].map((name) => ({
        ...item,
        label: "context" as const,
        reference: name,
        excerpt: `${name}\n  event`,
        explanation: "Cited counterevidence.",
        limitation: null,
      })),
    ]
    vi.mocked(getBurnCheckTargetEvidence).mockResolvedValue({
      status: "available",
      items,
      occurrences: [
        { findingId: "saved-1", status: "available", items },
        {
          findingId: "saved-2",
          status: "available",
          items: [{ ...item, reference: "anchor-2", excerpt: "different action" }],
        },
      ],
    })
    render(
      <BurnCheckTargetDetail
        target={target({
          evidenceAvailable: true,
          finding: { ...target().finding, detector: "ignoredInstructions" },
        })}
        openEvidence
        refresh={() => undefined}
      />,
    )
    await screen.findByText("command --flag", { exact: false })
    const evidence = screen.getByRole("region", { name: "Source details" })
    expect(screen.queryByRole("region", { name: "Occurrences" })).not.toBeInTheDocument()
    expect(screen.queryByText(/Occurrence \d/)).not.toBeInTheDocument()
    expect(screen.queryByText("different action")).not.toBeInTheDocument()
    expect(within(evidence).getByText("What happened")).toBeVisible()
    expect(within(evidence).queryAllByRole("link")).toHaveLength(0)
    const excerpts = evidence.querySelectorAll("pre")
    expect([...excerpts].map((node) => node.textContent)).toEqual([
      "command\n  --flag",
      "first\n  event",
      "second\n  event",
    ])
    expect(within(evidence).queryByText(/Saved action text/)).not.toBeInTheDocument()
    expect(
      within(evidence).queryByText("Some nearby context is no longer available."),
    ).not.toBeInTheDocument()
    expect(screen.queryByLabelText("About this evidence")).not.toBeInTheDocument()
    expect(screen.queryByRole("heading", { name: "Details" })).not.toBeInTheDocument()
    expect(
      screen.queryByRole("button", { name: "About this evidence" }),
    ).not.toBeInTheDocument()
    expect(
      screen.queryByText("Some nearby context is no longer available."),
    ).not.toBeInTheDocument()
    expect(getBurnCheckTargetEvidence).toHaveBeenCalledTimes(1)
  })

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
    openManualEvidence()
    expect(
      await screen.findByText("This instruction has changed or is no longer in the source."),
    ).toBeInTheDocument()
    expect(screen.queryByText("Instruction text unavailable.")).not.toBeInTheDocument()
    expect(screen.queryByText("The instruction file changed.")).not.toBeInTheDocument()
    expect(
      screen.getByText("Current instruction · AGENTS.md · Testing · lines 24–27"),
    ).toBeInTheDocument()
    expect(screen.getByText("x".repeat(360) + "…")).toBeVisible()
    expect(screen.getByRole("button", { name: "Show full details" })).toHaveAttribute(
      "aria-expanded",
      "false",
    )
    expect(screen.queryByText("x".repeat(900))).not.toBeInTheDocument()
    expect(screen.queryByText("earlier event")).not.toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "Show context" })).not.toBeInTheDocument()
  })

  it("shows the original action and supporting events for Ignored Instructions", async () => {
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

    openManualEvidence()
    expect(
      await screen.findByText("Do not use broad searches when exploring a codebase."),
    ).toBeInTheDocument()
    expect(screen.getByText("Instruction")).toBeInTheDocument()
    expect(screen.getByText(".config/opencode/AGENTS.md · line 45")).toBeInTheDocument()
    expect(screen.getAllByText(/\.config\/opencode\/AGENTS\.md/)).toHaveLength(1)
    expect(screen.getByRole("region", { name: "Source details" })).not.toHaveClass("border-t")
    expect(screen.getByText("Command: rg -n 'long search string' .")).toBeInTheDocument()
    expect(screen.getByText("Unneeded surrounding event")).toBeVisible()
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
    openManualEvidence()
    expect(
      await screen.findByText(
        (_, element) =>
          element?.tagName === "PRE" &&
          element.textContent === `first line\n  ${"detail ".repeat(40)}`,
      ),
    ).toBeInTheDocument()
    expect(screen.queryByText("This is the cited action.")).not.toBeInTheDocument()
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
    openManualEvidence()
    expect(await screen.findByText("Current instruction")).toBeInTheDocument()
    expect(screen.queryByText(limitation)).not.toBeInTheDocument()
    expect(screen.queryByLabelText("About this evidence")).not.toBeInTheDocument()
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
    openManualEvidence()
    expect(
      await screen.findByText("Do not run the shell command without approval."),
    ).toBeInTheDocument()
    expect(screen.getByText("Ran the shell command without approval.")).toBeInTheDocument()
  })

  it("retries failed requests and keeps the evidence section open", async () => {
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
    openManualEvidence()
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not load source details.")
    fireEvent.click(screen.getByRole("button", { name: "Retry" }))
    await act(async () => resolveLate({ status: "available", items: [] }))
    expect(screen.queryByText("Could not load source details.")).not.toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "Hide details" })).not.toBeInTheDocument()
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
    openManualEvidence()
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
    openManualEvidence()
    expect(getBurnCheckTargetEvidence).toHaveBeenCalledTimes(2)
    expect(await screen.findAllByText("Quality Review")).not.toHaveLength(0)
    expect(await screen.findByText("Source details could not be verified.")).toBeVisible()
    expect(screen.getByText("Source details could not be verified.")).toBeInTheDocument()
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

    await act(async () => openManualEvidence())
    expect(await screen.findByText("git push --force")).toBeInTheDocument()
    expect(screen.queryByText("Observed session action")).not.toBeInTheDocument()
    expect(
      screen.getByText("Global configuration (~/.config/opencode/AGENTS.md)"),
    ).toBeInTheDocument()
    expect(screen.getByRole("heading", { name: /Code Discovery/ })).toBeInTheDocument()
    expect(
      screen.getByText("What happened").parentElement?.querySelector("time"),
    ).not.toBeNull()
    expect(getBurnCheckTargetEvidence).toHaveBeenCalledWith("action-fresh")
    await act(async () =>
      fireEvent.click(screen.getByRole("button", { name: /Session Claude Code/ })),
    )
    expect(openBurnCheckSample).toHaveBeenCalledWith("opaque-session")
  })

  it("labels legacy grouped evidence as representative when occurrence records are absent", async () => {
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
    expect(screen.queryByLabelText("About this evidence")).not.toBeInTheDocument()
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

  it("shows compact headings for all four smart checks", async () => {
    const cases = [
      ["ignoredInstructions", "What happened"],
      ["scopeCreep", "Recorded work"],
      ["overExploring", "Read records used for this finding"],
      ["skillOpportunities", "Recorded work"],
    ] as const
    for (const [detector, heading] of cases) {
      vi.mocked(getBurnCheckTargetEvidence).mockResolvedValueOnce({
        status: "available",
        items: [evidenceItem("observedAction", `${detector}-action`)],
      })
      const view = render(
        <BurnCheckTargetDetail
          target={target({
            finding: { ...target().finding, detector },
            evidenceAvailable: true,
          })}
          refresh={() => undefined}
          openEvidence
        />,
      )
      expect(await screen.findByText(heading)).toBeVisible()
      expect(
        screen.queryByRole("region", { name: "Assessment decision" }),
      ).not.toBeInTheDocument()
      expect(screen.queryByText("Assessment")).not.toBeInTheDocument()
      expect(screen.queryByText("Suggestion")).not.toBeInTheDocument()
      view.unmount()
    }
  })

  it("formats typed action fields and keeps read contents collapsed", async () => {
    vi.mocked(getBurnCheckTargetEvidence).mockResolvedValue({
      status: "available",
      items: [
        evidenceItem("observedAction", "request", {
          sourceLabel: "Recorded read request",
          excerpt: JSON.stringify({
            type: "tool_use",
            name: "read",
            file_path: "src/detail.ts",
            offset: 12,
            limit: 4,
          }),
        }),
        evidenceItem("observedAction", "codex-read", {
          sourceLabel: "Recorded command",
          excerpt: JSON.stringify({
            command: ["/bin/zsh", "-lc", "nl -ba src/codex.rs"],
            parsed_cmd: [{ type: "read", path: "src/codex.rs", cmd: "nl -ba src/codex.rs" }],
          }),
        }),
        evidenceItem("observedAction", "pi-read", {
          sourceLabel: "Recorded read request",
          excerpt: JSON.stringify({ arguments: { path: "src/pi.rs", offset: 5 } }),
        }),
        evidenceItem("observedAction", "opencode-read", {
          sourceLabel: "Recorded read request",
          excerpt: JSON.stringify({
            input: { filePath: "src/opencode.rs", offset: 3, limit: 2 },
          }),
        }),
        evidenceItem("observedAction", "result", {
          sourceLabel: "Read result",
          excerpt: `<path>src/detail.ts</path>\n${"private file content ".repeat(20)}`,
        }),
      ],
    })
    render(
      <BurnCheckTargetDetail
        target={target({
          finding: { ...target().finding, detector: "overExploring" },
          evidenceAvailable: true,
        })}
        refresh={() => undefined}
        openEvidence
      />,
    )
    expect(await screen.findByText("Read: src/detail.ts, line 12–15")).toBeVisible()
    expect(screen.getByText("Read: src/codex.rs")).toBeVisible()
    expect(screen.getByText("Read: src/pi.rs, line 5")).toBeVisible()
    expect(screen.getByText("Read: src/opencode.rs, line 3–4")).toBeVisible()
    expect(screen.getByRole("button", { name: "Show returned content" })).toBeVisible()
    const content = screen.getByText(
      (_, element) =>
        element?.tagName === "PRE" &&
        (element.textContent?.includes("private file content ".repeat(20)) ?? false),
    )
    expect(content).not.toBeVisible()
    fireEvent.click(screen.getByRole("button", { name: "Show returned content" }))
    expect(content).toBeVisible()
    expect(
      screen.queryByText(/Assessment|Suggestion|Supporting events|Show context/),
    ).not.toBeInTheDocument()
  })

  it("keeps report evidence visible across rerenders without a detail disclosure", async () => {
    vi.mocked(getBurnCheckTargetEvidence).mockResolvedValue({
      status: "available",
      items: [evidenceItem("observedAction", "action")],
    })
    const view = render(
      <BurnCheckTargetDetail
        target={target({
          finding: { ...target().finding, detector: "scopeCreep" },
          evidenceAvailable: true,
        })}
        reportRow
        openEvidence
        refresh={() => undefined}
      />,
    )
    expect(await screen.findByText("Text action")).toBeVisible()
    expect(screen.queryByRole("button", { name: "Hide details" })).not.toBeInTheDocument()
    expect(
      screen.queryByRole("button", { name: "Show source details" }),
    ).not.toBeInTheDocument()
    view.rerender(
      <BurnCheckTargetDetail
        target={target({
          finding: { ...target().finding, detector: "scopeCreep" },
          evidenceAvailable: true,
        })}
        reportRow
        openEvidence
        refresh={() => undefined}
      />,
    )
    expect(screen.getByText("Text action")).toBeVisible()
    expect(getBurnCheckTargetEvidence).toHaveBeenCalledOnce()
  })

  it("does not show a generic evidence disclaimer", async () => {
    vi.mocked(getBurnCheckTargetEvidence).mockResolvedValue({
      status: "available",
      items: [
        evidenceItem("observedAction", "action", {
          explanation: "Internal assessment explanation.",
          limitation: "Internal evidence limitation.",
        }),
      ],
    })
    render(
      <BurnCheckTargetDetail
        target={target({
          finding: { ...target().finding, detector: "skillOpportunities" },
          evidenceAvailable: true,
        })}
        refresh={() => undefined}
        openEvidence
      />,
    )
    expect(await screen.findByText("Text action")).toBeVisible()
    expect(screen.queryByText("Internal assessment explanation.")).not.toBeInTheDocument()
    expect(screen.queryByText("Internal evidence limitation.")).not.toBeInTheDocument()
    expect(
      screen.queryByText(/assessment decision|supporting events|selected events/i),
    ).not.toBeInTheDocument()
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
