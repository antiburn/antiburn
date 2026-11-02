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
    "input-too-large",
    "provider-context-rejected",
    "fit-config-invalid",
    "input-invalid",
    "provider-invalid-request",
    "action-failed",
    "yaml-syntax",
    "config-invalid-value",
    "compile-invalid-expression",
    "discovery-read-failed",
    "discovery-directory-unavailable",
    "discovery-duplicate-id",
    "compile-unsupported-feature",
    "input-unavailable",
    "source-unavailable",
    "provider-authentication",
    "provider-unavailable",
    "provider-not-configured",
    "provider-delivery-unknown",
    "storage-failed",
    "opencode-v2-service-unavailable",
    "opencode-v2-authentication-rejected",
    "opencode-v2-source-mismatch",
    "opencode-v2-version-unsupported",
    "opencode-v2-capture-gap",
    "opencode-v2-source-limit",
  ])("keeps %s not assessed and gives the cause in detail", async (code) => {
    setup(
      null,
      false,
      { wins: [] },
      diagnosticReport({
        finding: 0,
        clean: 0,
        hasStarted: false,
        targets: [],
        waitingForEvents: null,
        diagnostic: "raw diagnostic must not become UI copy",
        diagnostics: [{ ...diagnostic, code }],
      }),
    )
    fireEvent.click(await screen.findByRole("button", { name: "Not assessed (1)" }))
    const row = screen.getByRole("button", { name: "Failed retry loop, Not assessed" })
    expect(row.querySelector(".activity-row-title-shimmer, .animate-pulse")).toBeNull()
    row.focus()
    fireEvent.keyDown(row, { key: "Enter" })
    const detail = document.getElementById(`burn-check-${installed.effectiveId}-detail`)!
    await waitFor(() => expect(detail).toHaveFocus())
    expect(screen.getByRole("status").textContent).toBe(
      `Failed retry loop: Not assessed. ${diagnostic.title}`,
    )
    const panel = within(screen.getByRole("region", { name: diagnostic.title }))
    expect(panel.getByText(diagnostic.message)).toBeVisible()
    expect(panel.queryByText("3 affected sessions")).toBeNull()
    expect(panel.getByText(/retries.yaml · Line 18, column 5/)).toBeVisible()
    expect(panel.getByRole("button", { name: "Open check file" })).toBeEnabled()
    expect(screen.queryByText("This check has no result in the current report.")).toBeNull()
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
          explanation: "The saved check found no repeated failures.",
          targets: result === "failure" ? installed.targets : [],
        }),
      )
      const row = await screen.findByRole("button", {
        name: `Failed retry loop, ${result === "failure" ? "1 failed" : "Passed"}`,
      })
      expect(
        screen.getByRole("region", {
          name: result === "failure" ? "Failed checks 1" : "Passed checks 1",
        }),
      ).toContainElement(row)
      const content = document.querySelector(".burn-checks-detail-content")!
      expect(content.querySelector("section")).toBe(
        screen.getByRole("region", { name: "Input too large" }),
      )
      expect(
        screen.getByText(
          result === "failure"
            ? "Review the recorded work."
            : "The saved check found no repeated failures.",
        ),
      ).toBeVisible()
    },
  )

  it("announces a selected check's error and recovery without changing its saved result", async () => {
    const saved = {
      finding: 0,
      clean: 1,
      checkingCount: 0,
      targets: [],
      diagnostics: [],
      explanation: "The saved check found no concern.",
    }
    const error = {
      ...diagnostic,
      code: "provider-authentication",
      title: "Reconnect the decision model",
      message: "Review the decision connection in Settings.",
      source: null,
      details: [{ label: "Diagnostic code", value: "provider-authentication" }],
    }
    const notice = {
      ...diagnostic,
      code: "input-shortened",
      severity: "info" as const,
      title: "Input shortened",
    }
    const { adapter, session } = setup(null, false, { wins: [] }, diagnosticReport(saved))
    const row = await screen.findByRole("button", { name: "Failed retry loop, Passed" })
    fireEvent.click(row)
    const announcement = screen.getByRole("status")
    expect(announcement).toHaveAttribute("aria-live", "polite")
    expect(announcement).toHaveAttribute("aria-atomic", "true")
    expect(announcement.textContent).toBe("Failed retry loop: Passed")

    vi.mocked(adapter.getReport).mockResolvedValue(
      diagnosticReport({
        ...saved,
        diagnostics: [notice, { ...error, severity: "info" }, error, error],
      }),
    )
    await act(async () => session.refresh())
    expect(screen.getByRole("button", { name: "Failed retry loop, Passed" })).toBe(row)
    expect(announcement.textContent).toBe(
      "Failed retry loop: Passed. Reconnect the decision model",
    )
    expect(announcement).not.toHaveTextContent(
      /provider-authentication|Diagnostic code|Input shortened/,
    )
    expect(screen.getAllByRole("region", { name: error.title })).toHaveLength(1)

    vi.mocked(adapter.getReport).mockResolvedValue(
      diagnosticReport({
        ...saved,
        diagnostics: [notice],
      }),
    )
    await act(async () => session.refresh())
    expect(screen.getByRole("button", { name: "Failed retry loop, Passed" })).toBe(row)
    expect(announcement.textContent).toBe("Failed retry loop: Passed")
    expect(screen.queryByRole("region", { name: error.title })).toBeNull()
  })

  it.each([
    "opencode-v2-service-unavailable",
    "opencode-v2-authentication-rejected",
    "opencode-v2-source-mismatch",
    "opencode-v2-version-unsupported",
    "opencode-v2-capture-gap",
    "opencode-v2-source-limit",
  ])("keeps the backend's blocked %s state not assessed", async (code) => {
    setup(
      null,
      false,
      { wins: [] },
      diagnosticReport({
        finding: 0,
        clean: 0,
        hasStarted: false,
        checkingCount: 0,
        unavailable: 1,
        targets: [],
        waitingForDataCount: 0,
        waitingForResultsCount: 0,
        waitingForEvents: null,
        diagnostics: [{ ...diagnostic, code, source: null }],
      }),
    )
    fireEvent.click(await screen.findByRole("button", { name: "Not assessed (1)" }))
    expect(
      screen.getByRole("button", { name: "Failed retry loop, Not assessed" }),
    ).toBeVisible()
    expect(screen.queryByRole("region", { name: /Passed checks/ })).toBeNull()
    expect(screen.queryByText("Passed")).toBeNull()
  })

  it.each([0, 1])(
    "keeps healthy checking and %i accepted passes beside a blocked source",
    async (clean) => {
      setup(
        null,
        false,
        { wins: [] },
        diagnosticReport({
          finding: 0,
          clean,
          hasStarted: true,
          checkingCount: 1,
          unavailable: 1,
          targets: [],
          waitingForEvents: null,
          explanation: "The saved check found no concern.",
          diagnostics: [
            { ...diagnostic, code: "opencode-v2-service-unavailable", source: null },
          ],
        }),
      )
      if (clean === 0)
        fireEvent.click(await screen.findByRole("button", { name: "Not assessed (1)" }))
      const row = await screen.findByRole("button", {
        name: `Failed retry loop, ${clean > 0 ? "Passed" : "Not assessed"} · Checking`,
      })
      expect(
        screen.getByRole("region", {
          name: clean > 0 ? "Passed checks 1" : "Not assessed (1)",
        }),
      ).toContainElement(row)
      if (clean === 0) expect(row).not.toHaveTextContent("Passed")
      fireEvent.click(row)
      const detail = document.getElementById(`burn-check-${installed.effectiveId}-detail`)!
      const error = within(detail).getByRole("region", { name: diagnostic.title })
      const status = within(detail).getByRole("region", { name: "Check status" })
      expect(
        error.compareDocumentPosition(status) & Node.DOCUMENT_POSITION_FOLLOWING,
      ).toBeTruthy()
      expect(within(status).getByRole("heading", { name: "Checking" })).toBeVisible()
      expect(status.querySelector(".text-burn-check-pass-fill")).toBeNull()
      if (clean > 0) {
        expect(within(status).getByText("Saved check result")).toBeVisible()
        expect(within(detail).getAllByText("The saved check found no concern.")).toHaveLength(1)
      } else {
        expect(within(status).getByText("No check result yet.")).toBeVisible()
        expect(within(detail).queryByText("The saved check found no concern.")).toBeNull()
      }
    },
  )

  it("keeps informational source details out of the result label", async () => {
    const message = "Stored-session analysis does not require a live connection."
    setup(
      null,
      false,
      { wins: [] },
      diagnosticReport({
        finding: 0,
        clean: 0,
        hasStarted: false,
        targets: [],
        waitingForDataCount: 0,
        waitingForResultsCount: 1,
        diagnostics: [
          {
            code: "opencode-v2-capture-gap",
            severity: "info",
            title: "Stored-session analysis",
            message,
            source: null,
            details: [],
            affectedSessionCount: 1,
          },
        ],
      }),
    )
    fireEvent.click(await screen.findByRole("button", { name: "Not assessed (1)" }))
    const row = screen.getByRole("button", {
      name: "Failed retry loop, Not assessed",
    })
    fireEvent.click(row)
    expect(row).not.toHaveTextContent(/Passed|Missing information/)
    expect(row.querySelector(".activity-row-title-shimmer, .animate-pulse")).toBeNull()
    const notice = screen.getByRole("region", {
      name: "Stored-session analysis",
    })
    expect(within(notice).getByText(message)).toBeVisible()
    expect(within(notice).queryByRole("button", { name: "Open check file" })).toBeNull()
  })

  it("keeps distinct files and decisions without repeating session counts", async () => {
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
    const primary = panels[0]!
    const technical = within(primary).getByText("Technical details")
    expect(technical.tagName).toBe("SUMMARY")
    expect(technical.closest("details")).not.toHaveAttribute("open")
    expect(screen.getByText("recovery")).not.toBeVisible()
    expect(panels[1]!).not.toBeVisible()
    expect(panels[2]!).not.toBeVisible()
    fireEvent.click(technical)
    expect(panels).toHaveLength(3)
    expect(within(panels[0]!).getByText(diagnostic.code)).toBeVisible()
    expect(panels[1]!).not.toBeVisible()
    expect(panels[2]!).not.toBeVisible()
    fireEvent.click(technical)
    const moreIssues = screen.getByText("2 more issues")
    expect(moreIssues).toBeVisible()
    expect(moreIssues.tagName).toBe("SUMMARY")
    expect(moreIssues.closest("details")).not.toHaveAttribute("open")
    expect(technical.closest("details")).not.toHaveAttribute("open")
    fireEvent.click(moreIssues)
    expect(panels[1]!).toBeVisible()
    expect(panels[2]!).toBeVisible()
    expect(technical.closest("details")).not.toHaveAttribute("open")
    expect(screen.queryByText(/affected sessions?/)).toBeNull()
    expect(
      within(panels[1]!).getByText("/workspace/other/.antiburn/checks/retries.yaml"),
    ).toBeVisible()
    fireEvent.click(within(panels[2]!).getByText("Technical details"))
    expect(within(panels[2]!).getByText(diagnostic.code)).toBeVisible()
    expect(within(panels[2]!).getByText("recovery")).toBeVisible()
  })

  it("shows compact identifiers and readable limits without repeated copy", async () => {
    const limit = "8192 estimated tokens; local limit 4096 estimated tokens"
    setup(
      null,
      false,
      { wins: [] },
      diagnosticReport({
        diagnostics: [
          {
            ...diagnostic,
            details: [
              ...diagnostic.details,
              { label: "Field", value: "decisions.retry.input" },
              { label: "Estimated request size", value: limit },
              { label: "Reason", value: diagnostic.message },
              { label: "Stored analysis", value: "Unrelated stored-analysis explanation." },
              { label: "Diagnostic code", value: diagnostic.code },
            ],
          },
        ],
      }),
    )
    const panel = await screen.findByRole("region", { name: diagnostic.title })
    const technical = within(panel).getByText("Technical details")
    expect(technical.closest("details")).not.toHaveAttribute("open")
    fireEvent.click(technical)
    expect(within(panel).getByText("decisions.retry.input").tagName).toBe("CODE")
    const prose = within(panel).getByText(limit)
    expect(prose.tagName).toBe("DD")
    expect(prose.closest("code, .font-mono")).toBeNull()
    expect(within(panel).getAllByText(diagnostic.message)).toHaveLength(1)
    expect(within(panel).getAllByText(diagnostic.code)).toHaveLength(1)
    expect(within(panel).queryByText("Diagnostic code")).toBeNull()
    expect(within(panel).queryByText("Stored analysis")).toBeNull()
    expect(within(panel).queryByText(/affected sessions?/)).toBeNull()
  })

  it.each([false, true])(
    "shows one primary alert for the same source issue with error first=%s",
    async (errorFirst) => {
      const error = {
        ...diagnostic,
        code: "opencode-v2-capture-gap",
        title: "OpenCode live checks are not running",
        message:
          "Live activity is not available for this session. Keep OpenCode open, then rescan sessions. Live checks cannot recover missed activity. Saved results cover only their recorded input.",
        source: null,
        details: [{ label: "Diagnostic code", value: "opencode-v2-capture-gap" }],
      }
      const notice = {
        ...error,
        severity: "info" as const,
      }
      setup(
        null,
        false,
        { wins: [] },
        diagnosticReport({
          checkingCount: 1,
          diagnostics: errorFirst ? [error, notice] : [notice, error],
        }),
      )
      const row = await screen.findByRole("button", {
        name: "Failed retry loop, 1 failed · 1 passed · Checking",
      })
      expect(screen.getByRole("region", { name: "Failed checks 1" })).toContainElement(row)
      const panel = screen.getByRole("region", { name: error.title })
      expect(within(panel).getByText(error.message)).toBeVisible()
      expect(
        within(panel).getByText(/Live checks cannot recover missed activity\./),
      ).toBeVisible()
      expect(screen.getAllByText(error.message)).toHaveLength(1)
      expect(screen.queryByText(/OpenCode V2/)).toBeNull()
      expect(screen.getByText("Review the recorded work.")).toBeVisible()
      expect(panel.querySelector("svg")).toHaveAttribute("aria-hidden", "true")
      const technical = within(panel).getByText("Technical details")
      expect(technical.closest("details")).not.toHaveAttribute("open")
      expect(within(panel).getByText(error.code)).not.toBeVisible()
      fireEvent.click(technical)
      expect(within(panel).getByText(error.code)).toBeVisible()
      expect(screen.getAllByRole("region", { name: error.title })).toHaveLength(1)
      expect(screen.queryByText(/more issues?/)).toBeNull()
    },
  )

  it("keeps distinct source errors behind a counted disclosure outside technical details", async () => {
    const journal = {
      ...diagnostic,
      code: "source-unavailable",
      title: "Session journal unavailable",
      message: "Restore access to the session journal.",
      source: null,
      details: [],
    }
    const source = {
      ...journal,
      title: "Session source unavailable",
      message: "Restore access to the session source.",
    }
    setup(
      null,
      false,
      { wins: [] },
      diagnosticReport({
        finding: 0,
        clean: 0,
        targets: [],
        diagnostics: [journal, source],
      }),
    )
    fireEvent.click(await screen.findByRole("button", { name: "Not assessed (1)" }))
    const row = screen.getByRole("button", { name: "Failed retry loop, Not assessed" })
    fireEvent.click(row)
    const primary = screen.getByRole("region", { name: journal.title })
    const related = screen.getByRole("region", { name: source.title })
    const metadata = within(primary).getByText("Technical details").closest("details")!
    const moreIssues = screen.getByText("1 more issue")
    expect(moreIssues).toBeVisible()
    expect(moreIssues.tagName).toBe("SUMMARY")
    expect(metadata).not.toHaveAttribute("open")
    expect(moreIssues.closest("details")).not.toHaveAttribute("open")
    expect(related).not.toBeVisible()
    fireEvent.click(moreIssues)
    expect(within(related).getByText(source.message)).toBeVisible()
    expect(within(primary).getByText(journal.message)).toBeVisible()
    expect(metadata).not.toHaveAttribute("open")
    fireEvent.click(moreIssues)
    expect(related).not.toBeVisible()
    expect(screen.getByRole("status").textContent).toBe(
      `Failed retry loop: Not assessed. ${journal.title}`,
    )
  })

  it("shows a malformed definition by its backend filename and clears it after repair", async () => {
    const { adapter, session } = setup(
      null,
      false,
      { wins: [] },
      diagnosticReport({
        name: "broken.yaml",
        hasStarted: false,
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
    const row = screen.getByRole("button", { name: "broken.yaml, Not assessed" })
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
      name: "Repaired check, Passed",
    })
    await waitFor(() => expect(repaired).toHaveFocus())
    expect(repaired).toHaveAttribute("aria-pressed", "true")
    expect(screen.queryByRole("region", { name: "Check needs fixing" })).toBeNull()
    expect(screen.queryByRole("button", { name: "Open check file" })).toBeNull()
    expect(screen.getByText("The checked results passed.")).toBeVisible()
  })

  it("keeps active work animated without removing its saved finding or diagnostic", async () => {
    setup(null, false, { wins: [] }, diagnosticReport({ checkingCount: 1 }))
    const row = await screen.findByRole("button", {
      name: "Failed retry loop, 1 failed · 1 passed · Checking",
    })
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

  it("shows the saved pass before a collapsed input-shortening notice", async () => {
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
      name: "Failed retry loop, Passed",
    })
    expect(row).not.toHaveTextContent("Input shortened")
    expect(row.querySelector(".activity-row-title-shimmer")).toBeNull()
    const info = screen.getByRole("region", { name: "Input shortened" })
    expect(info.querySelector("svg")).toHaveClass("text-label-tertiary")
    const pass = screen.getByText("No repeated failure found.")
    expect(pass).toBeVisible()
    expect(pass.compareDocumentPosition(info) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    const disclosure = within(info).getByText("Input shortened")
    expect(disclosure.tagName).toBe("SUMMARY")
    expect(disclosure.closest("details")).not.toHaveAttribute("open")
    expect(within(info).getByText(/This check shortened selected input/)).not.toBeVisible()
    expect(within(info).queryByText("3 affected sessions")).toBeNull()
    expect(within(info).getByRole("button", { name: "Open check file" })).not.toBeVisible()
    expect(within(info).getByText("8192 bytes")).not.toBeVisible()
    fireEvent.click(disclosure)
    expect(disclosure.closest("details")).toHaveAttribute("open")
    expect(within(info).getByText(/This check shortened selected input/)).toBeVisible()
    expect(within(info).getByRole("button", { name: "Open check file" })).toBeEnabled()
    fireEvent.click(within(info).getByText("Technical details"))
    expect(within(info).getByText("8192 bytes")).toBeVisible()
    expect(within(info).getByText("1024 bytes")).toBeVisible()
    fireEvent.click(disclosure)
    expect(within(info).getByText("8192 bytes")).not.toBeVisible()
    expect(pass).toBeVisible()
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
        hasStarted: false,
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
    fireEvent.click(screen.getByRole("button", { name: "Failed retry loop, Not assessed" }))
    expect(screen.getByRole("region", { name: "Cannot read check folder" })).toBeVisible()
    expect(screen.queryByRole("button", { name: "Open check file" })).toBeNull()
    expect(screen.queryByRole("region", { name: "Failed sessions" })).toBeNull()
    expect(screen.queryByText(/0 affected/)).toBeNull()
  })
})
