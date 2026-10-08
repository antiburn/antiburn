import { act, fireEvent, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type * as ClipboardModule from "../../../../lib/clipboard"
import type * as InsightsIpcModule from "../../../../lib/insightsIpc"
import type * as IpcModule from "../../../../lib/ipc"
import { BurnChecksView } from "../../BurnChecksView"
import { searchApp } from "../../../../lib/appSearch"

import {
  report,
  target,
  aggregate,
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

// This file holds the search tests for the Burn Checks view.
// The suite is split across files in this folder by theme. Each test renders
// the full view. CI runs this file next to the other heavy view suites, so
// one test can take five times its local run time. 15 s is the bound, not a
// target.
describe("BurnChecksView search", { timeout: 15_000 }, () => {
  it.each([
    "ignoredInstructions",
    "scopeCreep",
    "overExploring",
    "skillOpportunities",
  ] as const)(
    "explains sampling on hover and keyboard focus only for sampled %s",
    async (id) => {
      const check = {
        ...report.categories[0]!,
        id,
        sampled: true,
      }
      const first = setup(null, false, aggregate, { ...report, categories: [check] })
      const info = await screen.findByLabelText("This check has been sampled")
      expect(info).toBeVisible()
      expect(info.tagName).toBe("SPAN")
      fireEvent.click(info)
      expect(screen.queryByRole("tooltip")).not.toBeInTheDocument()
      fireEvent.pointerMove(info, { pointerType: "mouse" })
      expect(await screen.findByText(/This check assesses selected evidence/)).toBeVisible()
      expect(screen.getByText(/The review percentage is unknown/)).toBeVisible()
      expect(screen.getByText(/The review target is 50%/)).toBeVisible()
      fireEvent.pointerLeave(info)
      fireEvent.focus(info)
      expect(
        await screen.findByText(/does not establish that all work has been assessed/),
      ).toBeVisible()
      first.view.unmount()

      const second = setup(null, false, aggregate, {
        ...report,
        categories: [{ ...check, sampled: false }],
      })
      await screen.findByRole("heading", { level: 2 })
      expect(screen.queryByLabelText("This check has been sampled")).not.toBeInTheDocument()
      second.view.unmount()

      setup(null, false, aggregate, {
        ...report,
        categories: [{ ...report.categories[0]!, id }],
      })
      await screen.findByRole("heading", { level: 2 })
      expect(screen.queryByLabelText("This check has been sampled")).not.toBeInTheDocument()
    },
  )

  it("opens ignored instruction evidence through the report and keeps the ordinary prompt action", async () => {
    commands.evidence.mockResolvedValue({
      status: "available",
      items: [
        {
          label: "instruction",
          sourceLabel: "AGENTS.md · Testing",
          reference: "rule",
          observedAtMs: null,
          startLine: 24,
          endLine: 27,
          excerpt: "Run tests before release.",
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
          excerpt: "Released without tests",
          explanation: "",
          limitation: null,
        },
      ],
    })
    const check = {
      ...report.categories[0]!,
      id: "ignoredInstructions" as const,
      finding: 1,
      clean: 0,
      estimatedTokenBurnBasisPoints: null,
    }
    setup(
      {
        ...target,
        finding: { ...target.finding, detector: "ignoredInstructions" },
        autoFix: { status: "unavailable", reason: "unsupportedOrUnprovenTarget" },
        evidenceAvailable: true,
      },
      false,
      aggregate,
      { ...report, categories: [check] },
    )
    const row = await screen.findByRole("button", { name: /Ignored instructions, 1 failed/ })
    fireEvent.click(row)
    expect(
      screen.getByText("Some sessions didn't follow your agent instruction files properly."),
    ).toBeInTheDocument()
    expect(screen.getByRole("button", { name: /Copy fix prompt/ })).toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "Show evidence" })).not.toBeInTheDocument()
    expect(await screen.findByText("Run tests before release.")).toBeInTheDocument()
    expect(screen.getByText("Released without tests")).toBeInTheDocument()
    expect(commands.evidence).toHaveBeenCalledWith("action-fresh")
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
      const check = result.target.check
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
      expect(
        searchApp(result.label).some(
          (entry) => entry.target.kind === "check" && entry.target.check === check,
        ),
      ).toBe(true)
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
    expect(screen.getByRole("button", { name: "Not assessed (1)" })).toHaveAttribute(
      "aria-expanded",
      "true",
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
})
