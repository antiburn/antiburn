import { act, fireEvent, screen, waitFor, within } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { createElement } from "react"
import * as snoozes from "../../../../lib/snoozedBurnChecks"
import * as ipc from "../../../../lib/insightsIpc"
import { BurnChecksView } from "../../BurnChecksView"
import {
  installedCheckDiagnosticFixture,
  installedCheckFixture as installed,
} from "./installedCheckFixture"
import { aggregate, report, setup, target } from "./burnChecksTestSupport"

beforeEach(() => {
  HTMLElement.prototype.scrollIntoView = vi.fn()
  vi.spyOn(snoozes, "useSnoozedBurnChecks").mockReturnValue({ status: "ready", records: [] })
})
afterEach(() => vi.restoreAllMocks())

describe("installed checks in the existing UI", () => {
  it.each([1, 2])(
    "shows Checking for %i running sessions without extra statuses",
    async (checkingCount) => {
      const { session, adapter } = setup(
        null,
        false,
        { wins: [] },
        {
          ...report,
          categories: [],
          installedChecks: [{ ...installed, finding: 0, clean: 0, checkingCount, targets: [] }],
        },
      )
      const checkingRow = await screen.findByRole("button", {
        name: "Failed Retry Loop, Checking",
      })
      expect(screen.getByRole("region", { name: "Checking 1" })).toContainElement(checkingRow)
      expect(checkingRow).toHaveAttribute("data-outcome", "checking")
      expect(screen.queryByRole("button", { name: /Not assessed/ })).not.toBeInTheDocument()
      expect(screen.queryByRole("button", { name: /Passed checks/ })).not.toBeInTheDocument()
      checkingRow.focus()
      fireEvent.click(checkingRow)
      expect(screen.getByRole("heading", { name: "Failed Retry Loop" })).toBeVisible()
      expect(within(checkingRow).getByText("Checking")).toHaveClass(
        "activity-row-title-shimmer",
      )
      const detail = document.getElementById(`burn-check-${installed.effectiveId}-detail`)!
      expect(within(detail).getByText("Checking")).toBeVisible()
      expect(screen.getByRole("status")).toHaveTextContent("Failed Retry Loop: Checking")
      expect(screen.getByText("This check has not checked any sessions yet.")).toBeVisible()
      expect(screen.queryByText(/Scanning sessions/)).not.toBeInTheDocument()
      vi.mocked(adapter.getReport).mockResolvedValue({
        ...report,
        categories: [],
        installedChecks: [
          {
            ...installed,
            finding: 0,
            clean: 1,
            checkingCount: 0,
            targets: [],
            explanation: "No repeated failure found.",
          },
        ],
      })
      await act(async () => session.refresh())
      const passedRow = await screen.findByRole("button", {
        name: "Failed Retry Loop, Waiting for activity",
      })
      expect(passedRow).toBeVisible()
      expect(passedRow).toHaveTextContent("Waiting for activity")
      await waitFor(() => expect(passedRow).toHaveFocus())
      expect(screen.getByText("No repeated failure found.")).toBeVisible()
      expect(screen.queryByRole("region", { name: "Checking 1" })).not.toBeInTheDocument()
    },
  )

  it("shows when a check failed or completed without changing status", async () => {
    const { adapter, session } = setup(
      null,
      false,
      { wins: [] },
      {
        ...report,
        categories: [],
        installedChecks: [
          {
            ...installed,
            finding: 0,
            clean: 0,
            reviewFailed: 1,
            checkingCount: 0,
            targets: [],
          },
        ],
      },
    )
    fireEvent.click(await screen.findByRole("button", { name: /Not assessed/ }))
    const row = await screen.findByRole("button", {
      name: "Failed Retry Loop, Could not check",
    })
    fireEvent.click(row)
    expect(screen.getByText("This check has not checked any sessions yet.")).toBeVisible()

    vi.mocked(adapter.getReport).mockResolvedValue({
      ...report,
      categories: [],
      installedChecks: [
        {
          ...installed,
          finding: 0,
          clean: 0,
          reviewFailed: 0,
          reviewedWithoutStatus: 1,
          checkingCount: 0,
          targets: [],
        },
      ],
    })
    await act(async () => session.refresh())
    expect(
      screen.getByRole("button", {
        name: "Failed Retry Loop, Waiting for activity",
      }),
    ).toHaveTextContent("Waiting for activity")
    expect(
      await screen.findByText("This check has not checked any sessions yet."),
    ).toBeVisible()
  })

  it.each(["failure", "pass"] as const)(
    "retains a saved %s with independent work and review errors",
    async (result) => {
      setup(
        null,
        false,
        { wins: [] },
        {
          ...report,
          categories: [],
          installedChecks: [
            {
              ...installed,
              finding: result === "failure" ? 1 : 0,
              clean: result === "pass" ? 1 : 0,
              checkingCount: 1,
              reviewFailed: 1,
              reviewedWithoutStatus: 1,
            },
          ],
        },
      )
      const text = "Checking"
      const row = await screen.findByRole("button", { name: `Failed Retry Loop, ${text}` })
      expect(row).toHaveTextContent(text)
      expect(
        screen.getByRole("region", {
          name: result === "failure" ? "Failed checks 1" : "Passed checks 1",
        }),
      ).toContainElement(row)
      const detail = document.getElementById(`burn-check-${installed.effectiveId}-detail`)!
      expect(detail.querySelector("header")).toHaveTextContent(text)
      expect(screen.getByRole("status")).toHaveTextContent(`Failed Retry Loop: ${text}`)
    },
  )

  it("distinguishes waiting for data and results from running work and keeps selection reachable", async () => {
    const { adapter, session } = setup(
      null,
      false,
      { wins: [] },
      {
        ...report,
        categories: [],
        installedChecks: [
          { ...installed, finding: 0, clean: 0, checkingCount: 1, targets: [] },
        ],
      },
    )
    const row = await screen.findByRole("button", {
      name: "Failed Retry Loop, Checking",
    })
    row.focus()
    vi.mocked(adapter.getReport).mockResolvedValue({
      ...report,
      categories: [],
      installedChecks: [
        {
          ...installed,
          finding: 0,
          clean: 0,
          checkingCount: 0,
          waitingForDataCount: 0,
          waitingForResultsCount: 1,
          waitingForEvents: { event: "toolResult", count: 3, catchingUp: false },
          targets: [],
        },
      ],
    })
    await act(async () => session.refresh())
    const waiting = screen.getByRole("button", {
      name: "Failed Retry Loop, Waiting for activity",
    })
    await waitFor(() => expect(waiting).toHaveFocus())
    expect(waiting).toHaveAttribute("aria-pressed", "true")
    expect(waiting.querySelector(".activity-row-title-shimmer, .animate-pulse")).toBeNull()
    expect(screen.getByText("Needs 3 more tool results.")).toBeVisible()
    expect(
      within(waiting).queryByText(/action results|session data|No status change/),
    ).not.toBeInTheDocument()
    expect(screen.getByText("This check has not checked any sessions yet.")).toBeVisible()
    expect(screen.getByRole("button", { name: "Not assessed (1)" })).toHaveAttribute(
      "aria-expanded",
      "true",
    )
    expect(screen.queryByRole("region", { name: "Checking 1" })).not.toBeInTheDocument()

    vi.mocked(adapter.getReport).mockResolvedValue({
      ...report,
      categories: [],
      installedChecks: [
        {
          ...installed,
          finding: 0,
          clean: 0,
          checkingCount: 0,
          waitingForDataCount: 1,
          waitingForResultsCount: 1,
          waitingForEvents: { event: "toolResult", count: 3, catchingUp: true },
          targets: [],
        },
      ],
    })
    await act(async () => session.refresh())
    expect(screen.getByRole("button", { name: "Failed Retry Loop, Catching up" })).toBeVisible()
    expect(screen.queryByText("Needs 3 more tool results.")).not.toBeInTheDocument()
  })

  it("keeps the report after a failed snooze and retries the same duration", async () => {
    const save = vi
      .spyOn(snoozes, "snoozeBurnCheck")
      .mockRejectedValueOnce(new Error("write failed"))
      .mockResolvedValue()
    setup(
      null,
      false,
      { wins: [] },
      { ...report, categories: [], installedChecks: [installed] },
    )
    fireEvent.keyDown(await screen.findByRole("button", { name: "Snooze" }), {
      key: "ArrowDown",
    })
    fireEvent.click(await screen.findByRole("menuitem", { name: "For 1 month" }))
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not save the snooze")
    expect(screen.getByRole("button", { name: /Failed Retry Loop, Waiting for/ })).toBeVisible()
    fireEvent.click(screen.getByRole("button", { name: "Retry snooze" }))
    await waitFor(() => expect(save).toHaveBeenCalledTimes(2))
    expect(save).toHaveBeenLastCalledWith(installed.effectiveId, "month")
    await waitFor(() => expect(screen.queryByRole("alert")).not.toBeInTheDocument())
  })

  it("renders readable source fields, keeps raw JSON optional, and shows the observation once", async () => {
    vi.spyOn(ipc, "getInstalledCheckEvidence").mockResolvedValue({
      status: "available",
      items: [
        {
          label: "context",
          reference: "task",
          sourceLabel: "Requested task",
          excerpt: JSON.stringify({
            source: "pi",
            kind: "user_message",
            text: "Fix the parser.\nRun its tests.",
          }),
          observedAtMs: null,
          startLine: null,
          endLine: null,
          explanation: "",
          limitation: null,
        },
        {
          label: "observedAction",
          reference: "tool-1",
          sourceLabel: "Action result",
          excerpt: JSON.stringify({
            tool: {
              name: "bash",
              arguments: { command: "cargo test parser", cwd: "/synthetic" },
              result: {
                content: "Parser test failed.\nExpected two records.",
                status: "error",
              },
            },
            quality: { source_complete: false },
          }),
          observedAtMs: null,
          startLine: null,
          endLine: null,
          explanation: "",
          limitation: "Partial action result.",
        },
      ],
    })
    const observation = "The same failed command ran again."
    setup(
      null,
      false,
      { wins: [] },
      {
        ...report,
        categories: [],
        installedChecks: [
          {
            ...installed,
            explanation: observation,
            targets: [{ ...installed.targets[0]!, observation }],
          },
        ],
      },
    )
    await screen.findByRole("heading", { name: "Failed Retry Loop" })
    expect(screen.getAllByText(observation)).toHaveLength(1)
    fireEvent.click(screen.getByRole("button", { name: "Show source details" }))
    expect(await screen.findByText("Fix the parser. Run its tests.")).toBeVisible()
    const argumentsSection = screen.getByRole("region", { name: "Action arguments" })
    expect(within(argumentsSection).getByText("cargo test parser")).toBeVisible()
    const resultSection = screen.getByRole("region", { name: "Action result" })
    expect(resultSection).toHaveTextContent("Parser test failed. Expected two records.")
    const raw = screen.getAllByText("Raw source details")[1]!.closest("details")!
    expect(raw).not.toHaveAttribute("open")
    expect(raw.querySelector("pre")).not.toBeVisible()
    expect(screen.getByText("Partial action result.")).toBeVisible()
  })

  it("uses ordinary counts, keyboard selection and the right detail without native actions", async () => {
    const { adapter } = setup(null, false, aggregate, {
      ...report,
      installedChecks: [installed],
    })
    const native = await screen.findByRole("button", { name: /Old model usage, / })
    expect(
      screen.getByLabelText("4 check types in total, including snoozed checks"),
    ).toBeVisible()
    expect(screen.getByRole("region", { name: "Failed checks 2" })).toBeVisible()
    fireEvent.keyDown(native, { key: "ArrowDown" })
    const status = "Waiting for activity"
    const row = screen.getByRole("button", { name: `Failed Retry Loop, ${status}` })
    expect(row).toHaveTextContent(status)
    expect(row).toHaveFocus()
    expect(row).toHaveAttribute("aria-pressed", "true")
    fireEvent.keyDown(row, { key: "Enter" })
    await waitFor(() =>
      expect(
        document.getElementById(`burn-check-${installed.effectiveId}-detail`),
      ).toHaveFocus(),
    )
    const detail = within(screen.getByRole("region", { name: "Burn check details" }))
    expect(detail.getByRole("heading", { name: installed.name, level: 2 })).toBeVisible()
    expect(
      document
        .getElementById(`burn-check-${installed.effectiveId}-detail`)!
        .querySelector("header"),
    ).toHaveTextContent(status)
    expect(detail.getByRole("button", { name: "Snooze" })).toBeVisible()
    expect(detail.queryByRole("button", { name: "Fix" })).not.toBeInTheDocument()
    expect(detail.queryByRole("button", { name: "Copy fix prompt" })).not.toBeInTheDocument()
    expect(adapter.getTargets).not.toHaveBeenCalledWith(installed.effectiveId)
    const selectedDetail = within(
      document.getElementById(`burn-check-${installed.effectiveId}-detail`)!,
    )
    expect(
      selectedDetail.queryByText(/estimated burn|wasted|Savings|Verifying/),
    ).not.toBeInTheDocument()
    expect(
      screen.queryByRole("button", { name: /Reload|Pause|Resume/ }),
    ).not.toBeInTheDocument()
    expect(
      screen.queryByText(/Installed checks|Assessment history|Generation:|Probabilities:/),
    ).not.toBeInTheDocument()
  })

  it("uses existing source excerpts and session cards through read-only routes", async () => {
    const evidence = vi.spyOn(ipc, "getInstalledCheckEvidence").mockResolvedValue({
      status: "available",
      items: [
        {
          label: "context",
          sourceLabel: "Requested task",
          reference: "task",
          observedAtMs: null,
          startLine: null,
          endLine: null,
          excerpt: "Run the tests",
          explanation: "",
          limitation: null,
        },
      ],
    })
    const open = vi.spyOn(ipc, "openBurnCheckSample").mockResolvedValue({ outcome: "opened" })
    const samples = target.samples
    setup(
      null,
      false,
      { wins: [] },
      {
        ...report,
        categories: [],
        installedChecks: [{ ...installed, targets: [{ ...installed.targets[0]!, samples }] }],
      },
    )
    fireEvent.click(await screen.findByRole("button", { name: "Show source details" }))
    expect(await screen.findByText("Run the tests")).toBeVisible()
    expect(evidence).toHaveBeenCalledWith(installed.targets[0]!.id)
    expect(screen.getByRole("button", { name: "Hide source details" })).toHaveAttribute(
      "aria-expanded",
      "true",
    )
    const card = within(screen.getByRole("region", { name: "Failed sessions" })).getByRole(
      "button",
    )
    fireEvent.click(card)
    await waitFor(() => expect(open).toHaveBeenCalledWith(samples[0]!.navigationHandle))
  })

  it("preserves selection through a pass transition and treats invalid results as unassessed", async () => {
    const { session, adapter, view } = setup(
      null,
      false,
      { wins: [] },
      { ...report, categories: [], installedChecks: [installed] },
    )
    view.rerender(
      createElement(BurnChecksView, {
        active: true,
        session,
        focusedCheck: installed.effectiveId,
        focusRevision: 1,
      }),
    )
    await screen.findByRole("button", { name: /Failed Retry Loop, Waiting for/ })
    vi.mocked(adapter.getReport).mockResolvedValue({
      ...report,
      categories: [],
      installedChecks: [
        {
          ...installed,
          finding: 0,
          clean: 2,
          targets: [],
          explanation: "No concern in the checked work.",
        },
      ],
    })
    await act(async () => session.refresh())
    expect(
      await screen.findByRole("button", {
        name: "Failed Retry Loop, Waiting for activity",
      }),
    ).toHaveAttribute("aria-pressed", "true")
    expect(screen.getByText("No concern in the checked work.")).toBeVisible()
    expect(screen.queryByRole("region", { name: "Savings" })).not.toBeInTheDocument()
    vi.mocked(adapter.getReport).mockResolvedValue({
      ...report,
      categories: [],
      installedChecks: [
        {
          ...installed,
          finding: 0,
          clean: 0,
          targets: [],
          availability: { status: "invalid", reason: "Invalid YAML." },
          diagnostic: "Invalid YAML.",
          diagnostics: [
            {
              ...installedCheckDiagnosticFixture,
              code: "yaml-syntax",
              title: "Check needs fixing",
              message: "Fix the YAML syntax at the marked line, then save the check file.",
            },
          ],
        },
      ],
    })
    await act(async () => session.refresh())
    expect(
      await screen.findByRole("button", {
        name: "Failed Retry Loop, Check needs fixing",
      }),
    ).toBeVisible()
    expect(screen.getByRole("button", { name: "Not assessed (1)" })).toHaveAttribute(
      "aria-expanded",
      "true",
    )
    expect(
      screen.getByText("Fix the YAML syntax at the marked line, then save the check file."),
    ).toBeVisible()
    expect(screen.queryByText("Invalid YAML.")).not.toBeInTheDocument()
  })

  it("uses logical installed snoozes, preserves totals and restores the ordinary group", async () => {
    const snooze = vi.spyOn(snoozes, "snoozeBurnCheck").mockResolvedValue()
    const unsnooze = vi.spyOn(snoozes, "unsnoozeBurnCheck").mockResolvedValue()
    const { session, view } = setup(
      null,
      false,
      { wins: [] },
      { ...report, categories: [], installedChecks: [installed] },
    )
    fireEvent.keyDown(await screen.findByRole("button", { name: "Snooze" }), {
      key: "ArrowDown",
    })
    fireEvent.click(await screen.findByRole("menuitem", { name: "For 1 week" }))
    expect(snooze).toHaveBeenCalledWith(installed.effectiveId, "week")
    vi.mocked(snoozes.useSnoozedBurnChecks).mockReturnValue({
      status: "ready",
      records: [{ detector: installed.effectiveId, scope: "check", until: null }],
    })
    view.rerender(
      createElement(BurnChecksView, {
        active: true,
        session,
        focusedCheck: installed.effectiveId,
        focusRevision: 1,
      }),
    )
    expect(
      screen.getByLabelText("1 check type in total, including snoozed checks"),
    ).toBeVisible()
    expect(screen.queryByRole("heading", { name: /Failed checks/ })).not.toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Snoozed 1" })).toHaveAttribute(
      "aria-expanded",
      "true",
    )
    const snoozedStatus = "Snoozed"
    expect(
      screen.getByRole("button", { name: `Failed Retry Loop, ${snoozedStatus}` }),
    ).toHaveTextContent(snoozedStatus)
    await waitFor(() => expect(screen.getByRole("button", { name: "Unsnooze" })).toBeEnabled())
    fireEvent.click(screen.getByRole("button", { name: "Unsnooze" }))
    expect(unsnooze).toHaveBeenCalledWith(installed.effectiveId)
    vi.mocked(snoozes.useSnoozedBurnChecks).mockReturnValue({ status: "ready", records: [] })
    view.rerender(
      createElement(BurnChecksView, {
        active: true,
        session,
        focusedCheck: installed.effectiveId,
        focusRevision: 2,
      }),
    )
    expect(screen.getByRole("region", { name: "Failed checks 1" })).toBeVisible()
  })
})
