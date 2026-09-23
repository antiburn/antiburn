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

// This file holds the search tests for the Burn Checks view.
// The suite is split across files in this folder by theme. Each test renders
// the full view. CI runs this file next to the other heavy view suites, so
// one test can take five times its local run time. 15 s is the bound, not a
// target.
describe("BurnChecksView search", { timeout: 15_000 }, () => {
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
})
