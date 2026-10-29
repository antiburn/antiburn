import { act, fireEvent, screen, waitFor, within } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { createElement } from "react"
import * as snoozes from "../../../../lib/snoozedBurnChecks"
import * as ipc from "../../../../lib/insightsIpc"
import { BurnChecksView } from "../../BurnChecksView"
import {
  installedCheckDiagnosticFixture,
  installedCheckEvidenceFixture,
  installedCheckFixture as installed,
} from "./installedCheckFixture"
import { aggregate, report, setup, target } from "./burnChecksTestSupport"

beforeEach(() => {
  HTMLElement.prototype.scrollIntoView = vi.fn()
  vi.spyOn(snoozes, "useSnoozedBurnChecks").mockReturnValue({ status: "ready", records: [] })
})
afterEach(() => vi.restoreAllMocks())

describe("installed checks in the existing UI", () => {
  it.each([
    ["paused", "provider-authentication", "Connection needed"],
    ["unsupported", "compile-unsupported-feature", "Not supported"],
  ] as const)(
    "keeps %s result counts and saved evidence accessible",
    async (availability, code, reason) => {
      const evidence = vi
        .spyOn(ipc, "getInstalledCheckEvidence")
        .mockResolvedValue(installedCheckEvidenceFixture)
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
              availability: { status: availability },
              checkingCount: 2,
              waitingForDataCount: 3,
              waitingForEvents: { event: "toolResult", count: 3, catchingUp: true },
              diagnostics: [
                {
                  ...installedCheckDiagnosticFixture,
                  code,
                  title: reason,
                  source: null,
                  message: "Review the decision connection in Settings.",
                },
              ],
            },
          ],
        },
      )
      const summary = "1 failed · 1 passed"
      const row = await screen.findByRole("button", { name: `Failed retry loop, ${summary}` })
      expect(screen.getByRole("region", { name: "Failed checks 1" })).toContainElement(row)
      expect(row).toHaveTextContent(summary)
      expect(row).not.toHaveTextContent(/Checking|Waiting|Catching up/)
      expect(row.querySelector(".activity-row-title-shimmer")).toBeNull()
      fireEvent.click(row)
      const detail = document.getElementById(`burn-check-${installed.effectiveId}-detail`)!
      expect(detail.querySelector("header")).toHaveTextContent(summary)
      expect(within(detail).getByText("Review the recorded work.")).toBeVisible()
      expect(
        within(detail).getByText("Review the decision connection in Settings."),
      ).toBeVisible()
      expect(
        within(detail).queryByText(/Checking|Waiting on|Catching up/),
      ).not.toBeInTheDocument()
      fireEvent.click(within(detail).getByRole("button", { name: "Show source details" }))
      expect(
        await within(detail).findByRole("heading", { name: "Assessed input" }),
      ).toBeVisible()
      expect(within(detail).getByText("Fix the parser and run its tests.")).toBeVisible()
      expect(evidence).toHaveBeenCalledWith(installed.targets[0]!.id)
    },
  )

  it.each(["paused", "unsupported"] as const)(
    "keeps %s clean-result counts in the row and detail",
    async (availability) => {
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
              availability: { status: availability },
              finding: 0,
              clean: 2,
              targets: [],
              checkingCount: 2,
              waitingForDataCount: 3,
              explanation: "No repeated failure found.",
            },
          ],
        },
      )
      const summary = "2 passed"
      const row = await screen.findByRole("button", { name: `Failed retry loop, ${summary}` })
      expect(screen.getByRole("region", { name: "Passed checks 1" })).toContainElement(row)
      expect(row).toHaveTextContent(summary)
      expect(row).not.toHaveTextContent(/Checking|Waiting|Catching up/)
      fireEvent.click(row)
      const detail = document.getElementById(`burn-check-${installed.effectiveId}-detail`)!
      expect(detail.querySelector("header")).toHaveTextContent(summary)
      expect(within(detail).getByText("No repeated failure found.")).toBeVisible()
      expect(
        within(detail).queryByRole("button", { name: "Show source details" }),
      ).not.toBeInTheDocument()
    },
  )

  it.each([
    { availability: "paused", checkingCount: 0 },
    { availability: "paused", checkingCount: 2 },
    { availability: "unsupported", checkingCount: 0 },
    { availability: "unsupported", checkingCount: 2 },
  ] as const)(
    "keeps a started check unassessed while $availability with $checkingCount queued and no accepted results",
    async ({ availability, checkingCount }) => {
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
              availability: { status: availability },
              finding: 0,
              clean: 0,
              hasStarted: true,
              checkingCount,
              targets: [],
            },
          ],
        },
      )

      const collection = within(
        await screen.findByRole("region", { name: "Burn check collection" }),
      )
      expect(
        collection.queryByRole("button", { name: /Passed checks/ }),
      ).not.toBeInTheDocument()
      expect(collection.queryByRole("region", { name: /Checking/ })).not.toBeInTheDocument()
      fireEvent.click(collection.getByRole("button", { name: "Not assessed (1)" }))
      const row = collection.getByRole("button", { name: "Failed retry loop, Not assessed" })
      expect(collection.getByRole("region", { name: "Not assessed (1)" })).toContainElement(row)
      expect(row).toHaveAttribute("data-outcome", "unassessed")
      expect(row).not.toHaveTextContent(/Passed|Checking|Waiting|Catching up/)
      expect(row.querySelector(".activity-row-title-shimmer")).toBeNull()
    },
  )

  it.each(["invalid", "paused"] as const)(
    "does not show result counts for an %s check without current results",
    async (availability) => {
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
              availability: {
                status: availability,
                reason:
                  availability === "paused"
                    ? "Model-based Checks are disabled."
                    : "Invalid definition.",
              },
              finding: 0,
              clean: 0,
              targets: [],
              hasStarted: false,
              checkingCount: 0,
              waitingForEvents: null,
            },
          ],
        },
      )
      fireEvent.click(await screen.findByRole("button", { name: "Not assessed (1)" }))
      const row = screen.getByRole("button", { name: "Failed retry loop, Not assessed" })
      expect(row).not.toHaveTextContent(/failed|passed|Checking|Waiting|Catching up/)
      fireEvent.click(row)
      const detail = document.getElementById(`burn-check-${installed.effectiveId}-detail`)!
      expect(detail.querySelector("header")).toHaveTextContent("Not assessed")
      expect(
        within(detail).getByRole("heading", {
          name: availability === "invalid" ? "Check needs fixing" : "This check is paused",
        }),
      ).toBeVisible()
      expect(within(detail).queryByText("Review the recorded work.")).not.toBeInTheDocument()
      expect(
        within(detail).queryByRole("button", { name: "Show source details" }),
      ).not.toBeInTheDocument()
    },
  )

  it.each([1, 3])("keeps an unstarted wait of %i in detail only", async (count) => {
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
            finding: 0,
            clean: 0,
            hasStarted: false,
            reviewedWithoutStatus: 0,
            waitingForEvents: { event: "toolResult", count, catchingUp: false },
            targets: [],
          },
        ],
      },
    )
    fireEvent.click(await screen.findByRole("button", { name: "Not assessed (1)" }))
    const text = "Not assessed"
    const row = screen.getByRole("button", { name: `Failed retry loop, ${text}` })
    fireEvent.click(row)
    expect(row).toHaveTextContent(text)
    const detail = document.getElementById(`burn-check-${installed.effectiveId}-detail`)!
    expect(detail.querySelector("header")).toHaveTextContent(text)
    expect(
      within(detail).getByText(
        `Needs ${count} more ${count === 1 ? "tool result" : "tool results"}.`,
      ),
    ).toBeVisible()
    expect(within(detail).queryByText(/Saved check result/)).not.toBeInTheDocument()
    expect(screen.queryByRole("region", { name: "Checking 1" })).not.toBeInTheDocument()
  })

  it.each([
    { catchingUp: true, waitingForDataCount: 0 },
    { catchingUp: false, waitingForDataCount: 1 },
  ])(
    "explains the first check wait during catch-up ($catchingUp, $waitingForDataCount)",
    async ({ catchingUp, waitingForDataCount }) => {
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
              finding: 0,
              clean: 0,
              hasStarted: false,
              reviewedWithoutStatus: 0,
              waitingForDataCount,
              waitingForEvents: { event: "toolResult", count: 3, catchingUp },
              targets: [],
            },
          ],
        },
      )
      fireEvent.click(await screen.findByRole("button", { name: "Not assessed (1)" }))
      fireEvent.click(screen.getByRole("button", { name: "Failed retry loop, Not assessed" }))
      const detail = document.getElementById(`burn-check-${installed.effectiveId}-detail`)!
      expect(within(detail).getByText("Processing session activity.")).toBeVisible()
      expect(within(detail).getByText("No check result yet.")).toBeVisible()
      expect(within(detail).queryByText(/Needs 3 more tool results/)).not.toBeInTheDocument()
    },
  )

  it.each([0, 1])(
    "shows the idle fallback only without saved check results (%i started)",
    async (reviewedWithoutStatus) => {
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
              finding: 0,
              clean: 0,
              hasStarted: reviewedWithoutStatus > 0,
              reviewedWithoutStatus: 0,
              waitingForResultsCount: 0,
              waitingForEvents: null,
              targets: [],
            },
          ],
        },
      )
      fireEvent.click(await screen.findByRole("button", { name: "Not assessed (1)" }))
      fireEvent.click(
        await screen.findByRole("button", {
          name: "Failed retry loop, Not assessed",
        }),
      )
      const fallback = screen.queryByText("This check has no result in the current report.")
      if (reviewedWithoutStatus === 0) {
        expect(fallback).toBeVisible()
      } else {
        expect(fallback).not.toBeInTheDocument()
        expect(screen.getByText("No check results in the last 30 days.")).toBeVisible()
      }
    },
  )

  it.each([1, 2])(
    "keeps the first queued work not assessed for %i running sessions",
    async (checkingCount) => {
      const { session, adapter } = setup(
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
              hasStarted: false,
              reviewedWithoutStatus: 0,
              explanation: null,
              waitingForResultsCount: 0,
              waitingForEvents: null,
              checkingCount,
              targets: [],
            },
          ],
        },
      )
      const disclosure = await screen.findByRole("button", { name: "Not assessed (1)" })
      disclosure.focus()
      fireEvent.keyDown(disclosure, { key: "ArrowDown" })
      const checkingRow = screen.getByRole("button", {
        name: "Failed retry loop, Not assessed · Checking",
      })
      await waitFor(() => expect(checkingRow).toHaveFocus())
      expect(screen.getByRole("region", { name: "Not assessed (1)" })).toContainElement(
        checkingRow,
      )
      expect(checkingRow).toHaveAttribute("data-outcome", "unassessed")
      expect(screen.queryByRole("button", { name: /Passed checks/ })).not.toBeInTheDocument()
      expect(within(checkingRow).queryByText("Passed")).not.toBeInTheDocument()
      expect(screen.queryByRole("region", { name: "Checking 1" })).not.toBeInTheDocument()
      checkingRow.focus()
      fireEvent.click(checkingRow)
      expect(screen.getByRole("heading", { name: "Failed retry loop" })).toBeVisible()
      expect(within(checkingRow).getByText("Checking")).toHaveClass(
        "activity-row-title-shimmer",
      )
      const detail = document.getElementById(`burn-check-${installed.effectiveId}-detail`)!
      expect(within(detail).getByRole("heading", { name: "Checking" })).toBeVisible()
      expect(within(detail).getByText("No check result yet.")).toBeVisible()
      expect(within(detail).queryByText(/Saved check result/)).not.toBeInTheDocument()
      expect(screen.getByRole("status")).toHaveTextContent(
        "Failed retry loop: Not assessed · Checking",
      )
      expect(
        screen.queryByText("This check has no result in the current report."),
      ).not.toBeInTheDocument()
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
        name: "Failed retry loop, Passed",
      })
      expect(passedRow).toBeVisible()
      expect(passedRow).toHaveTextContent("Passed")
      await waitFor(() => expect(passedRow).toHaveFocus())
      expect(screen.getByText("No repeated failure found.")).toBeVisible()
      expect(within(detail).queryByText("No check result yet.")).not.toBeInTheDocument()
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
            hasStarted: true,
            reviewFailed: 1,
            checkingCount: 0,
            targets: [],
          },
        ],
      },
    )
    fireEvent.click(await screen.findByRole("button", { name: "Not assessed (1)" }))
    const row = screen.getByRole("button", {
      name: "Failed retry loop, Not assessed",
    })
    fireEvent.click(row)
    const detail = document.getElementById(`burn-check-${installed.effectiveId}-detail`)!
    expect(detail.querySelector("header")).toHaveTextContent("Not assessed")
    expect(within(detail).getByText("A recent check could not finish.")).toBeVisible()
    expect(
      screen.queryByText("This check has no result in the current report."),
    ).not.toBeInTheDocument()

    vi.mocked(adapter.getReport).mockResolvedValue({
      ...report,
      categories: [],
      installedChecks: [
        {
          ...installed,
          finding: 0,
          clean: 0,
          hasStarted: true,
          reviewFailed: 0,
          reviewedWithoutStatus: 1,
          checkingCount: 0,
          waitingForEvents: null,
          targets: [],
        },
      ],
    })
    await act(async () => session.refresh())
    expect(
      screen.getByRole("button", {
        name: "Failed retry loop, Not assessed",
      }),
    ).toHaveTextContent("Not assessed")
    expect(detail.querySelector("header")).toHaveTextContent("Not assessed")
    expect(within(detail).getByText("The check finished without a new result.")).toBeVisible()
    expect(
      screen.queryByText("This check has no result in the current report."),
    ).not.toBeInTheDocument()
  })

  it.each([
    { result: "failure", checkingCount: 0 },
    { result: "failure", checkingCount: 1 },
    { result: "pass", checkingCount: 0 },
    { result: "pass", checkingCount: 1 },
  ])(
    "retains a saved $result after a recheck error with $checkingCount queued",
    async ({ result, checkingCount }) => {
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
              checkingCount,
              reviewFailed: 1,
              reviewedWithoutStatus: 1,
            },
          ],
        },
      )
      const text = `${result === "failure" ? "1 failed" : "Passed"}${checkingCount > 0 ? " · Checking" : ""}`
      const row = await screen.findByRole("button", { name: `Failed retry loop, ${text}` })
      expect(row).toHaveTextContent(text)
      expect(
        screen.getByRole("region", {
          name: result === "failure" ? "Failed checks 1" : "Passed checks 1",
        }),
      ).toContainElement(row)
      const detail = document.getElementById(`burn-check-${installed.effectiveId}-detail`)!
      expect(detail.querySelector("header")).toHaveTextContent(text)
      expect(screen.getByRole("status")).toHaveTextContent(`Failed retry loop: ${text}`)
      expect(
        screen.queryByText("This check has no result in the current report."),
      ).not.toBeInTheDocument()
      expect(
        within(detail).getByText(
          result === "failure" ? installed.targets[0]!.observation! : installed.explanation!,
        ),
      ).toBeVisible()
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
    fireEvent.click(await screen.findByRole("button", { name: "Not assessed (1)" }))
    const row = screen.getByRole("button", {
      name: "Failed retry loop, Not assessed · Checking",
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
          reviewedWithoutStatus: 0,
          waitingForEvents: { event: "toolResult", count: 3, catchingUp: false },
          targets: [],
        },
      ],
    })
    await act(async () => session.refresh())
    const waiting = screen.getByRole("button", {
      name: "Failed retry loop, Not assessed",
    })
    await waitFor(() => expect(waiting).toHaveFocus())
    expect(waiting).toHaveAttribute("aria-pressed", "true")
    expect(waiting.querySelector(".activity-row-title-shimmer, .animate-pulse")).toBeNull()
    expect(waiting).toHaveTextContent("Not assessed")
    const detail = document.getElementById(`burn-check-${installed.effectiveId}-detail`)!
    expect(detail.querySelector("header")).toHaveTextContent("Not assessed")
    expect(screen.getByText("Needs 3 more tool results.")).toBeVisible()
    expect(
      within(waiting).queryByText(/action results|session data|No status change/),
    ).not.toBeInTheDocument()
    expect(
      screen.queryByText("This check has no result in the current report."),
    ).not.toBeInTheDocument()
    expect(screen.getByRole("region", { name: "Not assessed (1)" })).toContainElement(waiting)
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
          reviewedWithoutStatus: 1,
          waitingForEvents: { event: "toolResult", count: 3, catchingUp: true },
          targets: [],
        },
      ],
    })
    await act(async () => session.refresh())
    expect(
      screen.getByRole("button", { name: "Failed retry loop, Not assessed" }),
    ).toBeVisible()
    expect(screen.queryByText("Needs 3 more tool results.")).not.toBeInTheDocument()
    expect(
      screen.queryByText("This check has no result in the current report."),
    ).not.toBeInTheDocument()

    vi.mocked(adapter.getReport).mockResolvedValue({
      ...report,
      categories: [],
      installedChecks: [
        { ...installed, hasStarted: true, finding: 2, clean: 0, checkingCount: 1 },
      ],
    })
    await act(async () => session.refresh())
    const failed = screen.getByRole("button", {
      name: "Failed retry loop, 2 failed · Checking",
    })
    expect(screen.getByRole("region", { name: "Failed checks 1" })).toContainElement(failed)
    expect(failed).toHaveAttribute("data-outcome", "failed")
    expect(detail.querySelector("header")).toHaveTextContent("2 failed · Checking")
    expect(screen.queryByRole("region", { name: "Checking 1" })).not.toBeInTheDocument()
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
    expect(screen.getByRole("button", { name: /Failed retry loop, 1 failed/ })).toBeVisible()
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
    await screen.findByRole("heading", { name: "Failed retry loop" })
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
    const status = "1 failed · 1 passed"
    const row = screen.getByRole("button", { name: `Failed retry loop, ${status}` })
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
    await screen.findByRole("button", { name: /Failed retry loop, 1 failed/ })
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
        name: "Failed retry loop, 2 passed",
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
          hasStarted: false,
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
        name: "Failed retry loop, Not assessed",
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
    expect(screen.queryByText("No concern in the checked work.")).not.toBeInTheDocument()
    expect(screen.queryByText(/Saved check result/)).not.toBeInTheDocument()
  })

  it("uses logical installed snoozes, preserves totals and restores the ordinary group", async () => {
    const snooze = vi.spyOn(snoozes, "snoozeBurnCheck").mockResolvedValue()
    const unsnooze = vi.spyOn(snoozes, "unsnoozeBurnCheck").mockResolvedValue()
    const sourceReport = { ...report, categories: [], installedChecks: [installed] }
    const { session, view, adapter } = setup(null, false, { wins: [] }, sourceReport)
    fireEvent.keyDown(await screen.findByRole("button", { name: "Snooze" }), {
      key: "ArrowDown",
    })
    fireEvent.click(await screen.findByRole("menuitem", { name: "For 1 week" }))
    expect(snooze).toHaveBeenCalledWith(installed.effectiveId, "week")
    vi.mocked(snoozes.useSnoozedBurnChecks).mockReturnValue({
      status: "ready",
      records: [{ detector: installed.effectiveId, scope: "check", until: null }],
    })
    vi.mocked(adapter.getReport).mockResolvedValue({ ...sourceReport })
    await act(async () => session.refresh())
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
      screen.getByRole("button", { name: `Failed retry loop, ${snoozedStatus}` }),
    ).toHaveTextContent(snoozedStatus)
    await waitFor(() => expect(screen.getByRole("button", { name: "Unsnooze" })).toBeEnabled())
    fireEvent.click(screen.getByRole("button", { name: "Unsnooze" }))
    expect(unsnooze).toHaveBeenCalledWith(installed.effectiveId)
    vi.mocked(snoozes.useSnoozedBurnChecks).mockReturnValue({ status: "ready", records: [] })
    vi.mocked(adapter.getReport).mockResolvedValue({ ...sourceReport })
    await act(async () => session.refresh())
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
