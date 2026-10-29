import { act, fireEvent, render, screen } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import type {
  BurnCheckEvidenceItemPayload,
  BurnCheckTargetPayload,
} from "../../../lib/insightsIpc"
import { performProjectFolderAction } from "../../../lib/projectFolder"
import { BurnCheckTargetDetail, EvidenceExcerpt, targetCostLine } from "./BurnCheckTargetDetail"
import { target } from "./tests/burnChecksTestSupport"

vi.mock("../../../lib/projectFolder", () => ({
  performProjectFolderAction: vi.fn().mockResolvedValue(undefined),
}))

beforeEach(() => vi.mocked(performProjectFolderAction).mockClear())

function pricedTarget(overrides: Partial<BurnCheckTargetPayload> = {}): BurnCheckTargetPayload {
  return {
    ...target,
    display: {
      ...target.display,
      estimatedOpportunity: { value: 8.2, unit: "apiEquivalentUsd" },
    },
    occurrenceCount: 2,
    ...overrides,
  }
}

function evidenceItem(
  label: BurnCheckEvidenceItemPayload["label"],
  excerpt: string,
): BurnCheckEvidenceItemPayload {
  return {
    label,
    sourceLabel: "Session source",
    reference: label,
    observedAtMs: null,
    startLine: null,
    endLine: null,
    excerpt,
    explanation: "",
    limitation: null,
  }
}

describe("target cost line", () => {
  it("uses the distinct affected-session count instead of the occurrence count", () => {
    expect(targetCostLine(pricedTarget({ affectedSessionCount: 1, occurrenceCount: 4 }))).toBe(
      "Estimated ~$8.20 in cache reads across 1 session, including helper requests.",
    )
  })

  it("uses occurrence wording when the affected-session count is unavailable", () => {
    expect(targetCostLine(pricedTarget({ affectedSessionCount: null }))).toBe(
      "Estimated ~$8.20 in cache reads across 2 occurrences, including helper requests.",
    )
  })

  it("omits the line when there is no estimate", () => {
    expect(
      targetCostLine({ ...target, display: { ...target.display, estimatedOpportunity: null } }),
    ).toBeNull()
  })

  it("omits the line when the estimate did not price to dollars", () => {
    expect(
      targetCostLine({
        ...target,
        display: {
          ...target.display,
          estimatedOpportunity: { value: 400, unit: "literalInputTokens" },
        },
      }),
    ).toBeNull()
  })
})

describe("BurnCheckTargetDetail", () => {
  it("keeps native target cards without an unavailable evidence loader", () => {
    render(<BurnCheckTargetDetail target={target} refresh={vi.fn()} />)
    expect(screen.getByText("Global configuration")).toBeVisible()
    expect(screen.queryByRole("button", { name: /source details/i })).not.toBeInTheDocument()
  })

  it("shows the cost line when priced", () => {
    render(<BurnCheckTargetDetail target={pricedTarget()} refresh={vi.fn()} />)
    expect(screen.getByText(/~\$8\.20 in cache reads/)).toBeVisible()
  })

  it("offers full-path folder actions and keeps affected-session counts", async () => {
    render(
      <BurnCheckTargetDetail
        reportRow
        target={{
          ...target,
          projectName: "example-project",
          projectLocation: "…/worktrees/example-project",
          projectPath: "/tmp/worktrees/example-project",
          affectedSessionCount: 7,
          display: { ...target.display, scopeKind: "project" },
        }}
        refresh={vi.fn()}
      />,
    )
    expect(screen.getByText("· example-project")).toBeVisible()
    expect(screen.getByText("7 sessions affected")).toBeVisible()
    expect(screen.getByRole("button", { name: /Update model/ })).toBeVisible()
    act(() => screen.getByRole("button", { name: "Project folder" }).focus())
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
  })

  it("does not use a shortened display location as an actionable path", () => {
    render(
      <BurnCheckTargetDetail
        reportRow
        target={{
          ...target,
          projectPath: null,
          projectLocation: "…/worktrees/example-project",
        }}
        refresh={vi.fn()}
      />,
    )
    expect(screen.queryByRole("button", { name: "Project folder" })).not.toBeInTheDocument()
  })
})

describe("EvidenceExcerpt", () => {
  it("replaces a long preview with the full text when expanded", () => {
    const text = `Start ${"more evidence ".repeat(40).trimEnd()} end`
    render(<EvidenceExcerpt item={evidenceItem("context", text)} collapseLong />)
    expect(screen.queryByText(text)).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Show full details" }))
    expect(screen.getByText(text)).toBeVisible()
  })

  it("formats typed action fields", () => {
    render(<EvidenceExcerpt item={evidenceItem("observedAction", '{"command":"pnpm test"}')} />)
    expect(screen.getByText("Command: pnpm test")).toBeVisible()
  })
})
