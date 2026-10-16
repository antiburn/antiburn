import { act, fireEvent, screen, waitFor, within } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import * as ipc from "../../../../lib/insightsIpc"
import * as snoozes from "../../../../lib/snoozedBurnChecks"
import { deferred, report, setup } from "./burnChecksTestSupport"
import {
  installedCheckDiagnosticFixture as diagnostic,
  installedCheckFixture as installed,
} from "./installedCheckFixture"

function diagnosticReport(overrides: Partial<ipc.InstalledCheckReportPayload> = {}) {
  return {
    ...report,
    categories: [],
    installedChecks: [{ ...installed, diagnostics: [diagnostic], ...overrides }],
  }
}

beforeEach(() => {
  HTMLElement.prototype.scrollIntoView = vi.fn()
  vi.spyOn(snoozes, "useSnoozedBurnChecks").mockReturnValue({ status: "ready", records: [] })
})

afterEach(() => vi.restoreAllMocks())

describe("installed check diagnostics", () => {
  it.each([
    ["input-too-large", "Input too large"],
    ["provider-context-rejected", "Input too large"],
    ["fit-config-invalid", "Check needs fixing"],
    ["input-invalid", "Check needs fixing"],
    ["provider-invalid-request", "Check needs fixing"],
    ["action-failed", "Check needs fixing"],
    ["yaml-syntax", "Check needs fixing"],
    ["config-invalid-value", "Check needs fixing"],
    ["compile-invalid-expression", "Check needs fixing"],
    ["discovery-read-failed", "Cannot read check"],
    ["discovery-directory-unavailable", "Cannot read check"],
    ["discovery-duplicate-id", "Duplicate check"],
    ["compile-unsupported-feature", "Not supported"],
    ["input-unavailable", "Missing information"],
    ["source-unavailable", "Missing information"],
    ["provider-authentication", "Connection needed"],
    ["provider-unavailable", "Connection needed"],
    ["provider-not-configured", "Connection needed"],
    ["provider-delivery-unknown", "Could not check"],
    ["storage-failed", "Could not check"],
  ])("shows a short, static %s status and friendly detail", async (code, status) => {
    setup(
      null,
      false,
      { wins: [] },
      diagnosticReport({
        finding: 0,
        clean: 0,
        targets: [],
        waitingForEvents: null,
        diagnostic: "raw diagnostic must not become UI copy",
        diagnostics: [{ ...diagnostic, code }],
      }),
    )
    fireEvent.click(await screen.findByRole("button", { name: "Not assessed (1)" }))
    const row = screen.getByRole("button", { name: `Failed Retry Loop, ${status}` })
    expect(row.querySelector(".activity-row-title-shimmer, .animate-pulse")).toBeNull()
    row.focus()
    fireEvent.keyDown(row, { key: "Enter" })
    const detail = document.getElementById(`burn-check-${installed.effectiveId}-detail`)!
    await waitFor(() => expect(detail).toHaveFocus())
    expect(screen.getByRole("status")).toHaveTextContent(`Failed Retry Loop: ${status}`)
    const panel = within(screen.getByRole("region", { name: diagnostic.title }))
    expect(panel.getByText(diagnostic.message)).toBeVisible()
    expect(panel.getByText("3 affected sessions")).toBeVisible()
    expect(panel.getByText(/retries.yaml · Line 18, column 5/)).toBeVisible()
    expect(panel.getByRole("button", { name: "Open check file" })).toBeEnabled()
    expect(screen.queryByText("This check has not checked any sessions yet.")).toBeNull()
    expect(screen.queryByText("raw diagnostic must not become UI copy")).toBeNull()
    expect(screen.queryByRole("region", { name: "Failed sessions" })).toBeNull()
    expect(screen.queryByRole("button", { name: /Copy|Open session/ })).toBeNull()
  })

  it.each(["failure", "pass"] as const)(
    "shows errors first and keeps the previous %s in its result group",
    async (result) => {
      setup(
        null,
        false,
        { wins: [] },
        diagnosticReport({
          finding: result === "failure" ? 1 : 0,
          clean: result === "pass" ? 1 : 0,
          explanation: "The saved assessment found no repeated failures.",
          targets: result === "failure" ? installed.targets : [],
        }),
      )
      const row = await screen.findByRole("button", {
        name: "Failed Retry Loop, Input too large",
      })
      expect(
        screen.getByRole("region", {
          name: result === "failure" ? "Failed checks 1" : "Passed checks 1",
        }),
      ).toContainElement(row)
      const content = document.querySelector(".burn-checks-detail-content")!
      expect(content.firstElementChild?.firstElementChild).toBe(
        screen.getByRole("region", { name: "Input too large" }),
      )
      expect(
        screen.getByText(
          result === "failure"
            ? "Review the recorded work."
            : "The saved assessment found no repeated failures.",
        ),
      ).toBeVisible()
    },
  )

  it("keeps grouped counts, distinct files, and distinct decisions readable", async () => {
    setup(
      null,
      false,
      { wins: [] },
      diagnosticReport({
        diagnostics: [
          diagnostic,
          {
            ...diagnostic,
            source: {
              reference: "other-file-reference",
              path: "/workspace/other/.antiburn/checks/retries.yaml",
              line: null,
              column: null,
            },
            affectedSessionCount: 1,
          },
          {
            ...diagnostic,
            details: [{ label: "Decision", value: "recovery" }],
            affectedSessionCount: 2,
          },
        ],
      }),
    )
    const panels = await screen.findAllByRole("region", { name: "Input too large" })
    expect(panels).toHaveLength(3)
    expect(within(panels[0]!).getByText("3 affected sessions")).toBeVisible()
    expect(within(panels[1]!).getByText("1 affected session")).toBeVisible()
    expect(
      within(panels[1]!).getByText("/workspace/other/.antiburn/checks/retries.yaml"),
    ).toBeVisible()
    expect(within(panels[2]!).getByText("2 affected sessions")).toBeVisible()
    const technical = within(panels[2]!).getByText("Technical details")
    expect(technical.tagName).toBe("SUMMARY")
    expect(technical.closest("details")).not.toHaveAttribute("open")
    expect(within(panels[2]!).getByText("recovery")).not.toBeVisible()
    fireEvent.click(technical)
    expect(technical.closest("details")).toHaveAttribute("open")
    expect(within(panels[2]!).getByText("recovery")).toBeVisible()
  })

  it("shows a malformed definition by its backend filename and clears it after repair", async () => {
    const { adapter, session } = setup(
      null,
      false,
      { wins: [] },
      diagnosticReport({
        name: "broken.yaml",
        aliases: [],
        description: null,
        finding: 0,
        clean: 0,
        targets: [],
        availability: { status: "invalid", reason: "internal parse error" },
        diagnostics: [{ ...diagnostic, code: "yaml-syntax", title: "Check needs fixing" }],
      }),
    )
    fireEvent.click(await screen.findByRole("button", { name: "Not assessed (1)" }))
    const row = screen.getByRole("button", { name: "broken.yaml, Check needs fixing" })
    row.focus()
    fireEvent.click(row)
    expect(screen.getByRole("heading", { name: "broken.yaml", level: 2 })).toBeVisible()
    expect(screen.queryByText("internal parse error")).toBeNull()
    vi.mocked(adapter.getReport).mockResolvedValue(
      diagnosticReport({
        name: "Repaired check",
        finding: 0,
        clean: 1,
        targets: [],
        diagnostics: [],
        explanation: "The checked results passed.",
      }),
    )
    await act(async () => session.refresh())
    const repaired = screen.getByRole("button", {
      name: "Repaired check, Waiting for activity",
    })
    await waitFor(() => expect(repaired).toHaveFocus())
    expect(repaired).toHaveAttribute("aria-pressed", "true")
    expect(screen.queryByRole("region", { name: "Check needs fixing" })).toBeNull()
    expect(screen.queryByRole("button", { name: "Open check file" })).toBeNull()
    expect(screen.getByText("The checked results passed.")).toBeVisible()
  })

  it("keeps active work animated without removing its saved finding or diagnostic", async () => {
    setup(null, false, { wins: [] }, diagnosticReport({ checkingCount: 1 }))
    const row = await screen.findByRole("button", { name: "Failed Retry Loop, Checking" })
    expect(within(row).getByText("Checking")).toHaveClass("activity-row-title-shimmer")
    expect(screen.getByRole("region", { name: "Failed checks 1" })).toContainElement(row)
    expect(screen.getByRole("region", { name: "Input too large" })).toBeVisible()
    expect(screen.getByText("Review the recorded work.")).toBeVisible()
  })

  it("does not transfer an open failure to a different decision after a report refresh", async () => {
    vi.spyOn(ipc, "openInstalledCheckDefinition").mockRejectedValue(
      "Refresh Checks and try again.",
    )
    const remaining = {
      ...diagnostic,
      details: [{ label: "Decision", value: "recovery" }],
    }
    const { adapter, session } = setup(
      null,
      false,
      { wins: [] },
      diagnosticReport({ diagnostics: [diagnostic, remaining] }),
    )
    const buttons = await screen.findAllByRole("button", { name: "Open check file" })
    fireEvent.click(buttons[0]!)
    expect(await screen.findByRole("alert")).toHaveTextContent("Refresh Checks and try again.")
    vi.mocked(adapter.getReport).mockResolvedValue(
      diagnosticReport({ diagnostics: [remaining] }),
    )
    await act(async () => session.refresh())
    expect(screen.getAllByRole("region", { name: "Input too large" })).toHaveLength(1)
    expect(screen.queryByRole("alert")).toBeNull()
    expect(screen.getByText("recovery")).toBeInTheDocument()
  })

  it("keeps successful input shortening quiet and its reduction details collapsed", async () => {
    setup(
      null,
      false,
      { wins: [] },
      diagnosticReport({
        finding: 0,
        clean: 1,
        targets: [],
        explanation: "No repeated failure found.",
        diagnostics: [
          {
            ...diagnostic,
            code: "input-shortened",
            severity: "info",
            title: "Input shortened",
            message: "This check shortened selected input as allowed by its definition.",
            details: [
              { label: "Input path", value: "activity[0].tool.result.text" },
              { label: "Original size", value: "8192 bytes" },
              { label: "Retained size", value: "1024 bytes" },
            ],
          },
        ],
      }),
    )
    const row = await screen.findByRole("button", {
      name: "Failed Retry Loop, Waiting for activity",
    })
    expect(row).not.toHaveTextContent("Input shortened")
    expect(row.querySelector(".activity-row-title-shimmer")).toBeNull()
    const info = screen.getByRole("region", { name: "Input shortened" })
    expect(info.querySelector("svg")).toHaveClass("text-label-tertiary")
    expect(within(info).getByText("8192 bytes")).not.toBeVisible()
    fireEvent.click(within(info).getByText("Technical details"))
    expect(within(info).getByText("8192 bytes")).toBeVisible()
    expect(within(info).getByText("1024 bytes")).toBeVisible()
    expect(screen.getByText("No repeated failure found.")).toBeVisible()
  })

  it("opens only the issued reference, blocks duplicate requests, and retains diagnostics on failure", async () => {
    const request = deferred<void>()
    const open = vi
      .spyOn(ipc, "openInstalledCheckDefinition")
      .mockReturnValueOnce(request.promise)
      .mockRejectedValueOnce(
        "Allow access to this folder in Settings → Sources, then try again.",
      )
      .mockResolvedValue(undefined)
    setup(null, false, { wins: [] }, diagnosticReport())
    const button = await screen.findByRole("button", { name: "Open check file" })
    button.focus()
    expect(button).toHaveFocus()
    expect(button.querySelector("svg")).toHaveAttribute("aria-hidden", "true")
    fireEvent.click(button, { detail: 0 })
    fireEvent.click(button)
    expect(open).toHaveBeenCalledExactlyOnceWith(diagnostic.source!.reference)
    expect(button).toBeDisabled()
    expect(button).toHaveAttribute("aria-busy", "true")
    await act(async () => request.reject(new Error("native exception with private data")))
    expect(screen.getByRole("alert")).toHaveTextContent("Could not open the check file.")
    expect(screen.queryByText(/native exception/)).toBeNull()
    expect(screen.getByRole("region", { name: "Input too large" })).toBeVisible()
    expect(screen.getByText("Review the recorded work.")).toBeVisible()
    expect(button).toBeEnabled()
    fireEvent.click(button)
    await waitFor(() => expect(open).toHaveBeenCalledTimes(2))
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Allow access to this folder in Settings → Sources, then try again.",
    )
    expect(screen.getByText(diagnostic.message)).toBeVisible()
    fireEvent.click(button)
    await waitFor(() => expect(open).toHaveBeenCalledTimes(3))
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull())
    expect(open).toHaveBeenLastCalledWith(diagnostic.source!.reference)
  })

  it("pauses file actions while the main window is inactive", async () => {
    const open = vi.spyOn(ipc, "openInstalledCheckDefinition").mockResolvedValue(undefined)
    const { hide } = setup(null, false, { wins: [] }, diagnosticReport())
    const button = await screen.findByRole("button", { name: "Open check file" })
    act(() => hide())
    expect(button).toBeDisabled()
    fireEvent.click(button)
    expect(open).not.toHaveBeenCalled()
  })

  it("shows directory errors without a file or session action", async () => {
    setup(
      null,
      false,
      { wins: [] },
      diagnosticReport({
        finding: 0,
        clean: 0,
        targets: [],
        diagnostics: [
          {
            ...diagnostic,
            code: "discovery-read-failed",
            title: "Cannot read check folder",
            message: "Check folder access, then scan again.",
            source: null,
            details: [{ label: "Directory", value: "/workspace/demo/.antiburn/checks" }],
            affectedSessionCount: 0,
          },
        ],
      }),
    )
    fireEvent.click(await screen.findByRole("button", { name: "Not assessed (1)" }))
    fireEvent.click(
      screen.getByRole("button", { name: "Failed Retry Loop, Cannot read check" }),
    )
    expect(screen.getByRole("region", { name: "Cannot read check folder" })).toBeVisible()
    expect(screen.queryByRole("button", { name: "Open check file" })).toBeNull()
    expect(screen.queryByRole("region", { name: "Failed sessions" })).toBeNull()
    expect(screen.queryByText(/0 affected/)).toBeNull()
  })
})
