import { render, screen, within } from "@testing-library/react"
import { describe, expect, it } from "vitest"

import type { InstalledCheckReportPayload } from "../../../../lib/insightsIpc"
import { checkRows, isInstalledCheck } from "../../../../lib/presentation/checkRows"
import { InstalledCheckDetailContent } from "../InstalledCheckDetailContent"
import {
  installedCheckDiagnosticFixture as diagnostic,
  installedCheckFixture as installed,
} from "./installedCheckFixture"

function content(overrides: Partial<InstalledCheckReportPayload>, snoozed = false) {
  const check = checkRows({
    evidenceSettled: true,
    windowSessions: 3,
    pendingEvidence: 0,
    deferredEvidence: 0,
    estimatedTokenBurnBasisPoints: null,
    categories: [],
    installedChecks: [
      {
        ...installed,
        finding: 0,
        clean: 0,
        explanation: null,
        reviewedWithoutStatus: 0,
        waitingForResultsCount: 0,
        waitingForEvents: null,
        targets: [],
        ...overrides,
      },
    ],
  }).find(isInstalledCheck)!
  return <InstalledCheckDetailContent check={check} active snoozed={snoozed} />
}

const savedResults = [
  { finding: 0, clean: 0, summary: "No check result yet.", explanation: null },
  { finding: 0, clean: 1, summary: "Saved check result: no concern found.", explanation: null },
  {
    finding: 0,
    clean: 1,
    summary: "Saved check result",
    explanation: "The saved check found no repeated failures.",
  },
  { finding: 0, clean: 2, summary: "Saved check results: 2 passed.", explanation: null },
  { finding: 1, clean: 0, summary: "Saved check results: 1 failed.", explanation: null },
  {
    finding: 1,
    clean: 2,
    summary: "Saved check results: 1 failed, 2 passed.",
    explanation: installed.targets[0]!.observation,
  },
]

describe("installed check detail status", () => {
  it.each([
    { hasStarted: true, message: "No check results in the last 30 days." },
    { hasStarted: false, message: "This check has no result in the current report." },
  ])(
    "bounds an empty result to the report when hasStarted=$hasStarted",
    ({ hasStarted, message }) => {
      render(content({ hasStarted }))
      expect(screen.getByRole("region", { name: "Check status" })).toHaveTextContent(message)
      expect(screen.queryByText("This check has not checked any sessions yet.")).toBeNull()
    },
  )

  it.each([
    {
      status: "paused",
      title: "This check is paused",
      message: "Review model-based checks and the selected connection in Settings → Checks.",
    },
    {
      status: "unsupported",
      title: "Check is not supported",
      message: "Review the check file and selected connection in Settings → Checks.",
    },
  ] as const)("gives Settings guidance for a $status check", ({ status, title, message }) => {
    render(content({ availability: { status } }))
    expect(screen.getByRole("heading", { name: title })).toBeVisible()
    expect(screen.getByText(message)).toBeVisible()
  })

  it.each(
    savedResults.flatMap((saved) =>
      ["checking", "catching up", "waiting"].map((progress) => ({ ...saved, progress })),
    ),
  )(
    "combines $progress with $summary in one neutral panel",
    ({ finding, clean, explanation, summary, progress }) => {
      const { container } = render(
        content({
          finding,
          clean,
          explanation,
          checkingCount: progress === "checking" ? 1 : 0,
          waitingForEvents: {
            event: "userMessage",
            count: 3,
            catchingUp: progress === "catching up",
          },
          targets: finding > 0 ? installed.targets : [],
        }),
      )
      const status = screen.getByRole("region", { name: "Check status" })
      expect(within(status).getByText(summary)).toBeVisible()
      expect(container.querySelectorAll(".bg-surface-card\\/75")).toHaveLength(1)
      expect(container.querySelector(".text-burn-check-pass-fill")).toBeNull()
      expect(within(status).getByRole("heading")).toHaveTextContent(
        progress === "checking"
          ? "Checking"
          : progress === "catching up"
            ? "Processing session activity."
            : "Waiting for new activity.",
      )
      if (progress === "waiting") {
        expect(within(status).getByText("Needs 3 more user messages.")).toBeVisible()
      } else {
        expect(status).not.toHaveTextContent(/Needs 3 more/)
      }
      if (explanation) {
        expect(screen.getAllByText(explanation)).toHaveLength(1)
        if (finding === 0) expect(within(status).getByText(explanation)).toBeVisible()
      }
      if (finding > 0) {
        const observation = screen.getByText(installed.targets[0]!.observation!)
        expect(observation).toBeVisible()
        expect(
          status.compareDocumentPosition(observation) & Node.DOCUMENT_POSITION_FOLLOWING,
        ).toBeTruthy()
        expect(screen.getByRole("button", { name: "Show source details" })).toBeVisible()
        expect(status).not.toHaveTextContent(/no concern/i)
      }
    },
  )

  it("explains completed no-write work without claiming a pass", () => {
    const { container } = render(content({ reviewedWithoutStatus: 1 }))
    expect(screen.getByText("The check finished without a new result.")).toBeVisible()
    expect(screen.queryByText(/no concern|passed/i)).toBeNull()
    expect(container.querySelector(".text-burn-check-pass-fill")).toBeNull()
  })

  it("explains no-write work while waiting for the next activity", () => {
    render(
      content({
        reviewedWithoutStatus: 1,
        waitingForEvents: { event: "userMessage", count: 1, catchingUp: false },
      }),
    )
    const status = screen.getByRole("region", { name: "Check status" })
    expect(within(status).getByText("Waiting for new activity.")).toBeVisible()
    expect(within(status).getByText("Needs 1 more user message.")).toBeVisible()
    expect(within(status).getByText("The check finished without a new result.")).toBeVisible()
    expect(status).not.toHaveTextContent(/no concern|passed/i)
  })

  it.each(["pass", "finding"] as const)(
    "keeps the saved %s after no-write work without claiming a new result",
    (result) => {
      render(
        content({
          finding: result === "finding" ? 1 : 0,
          clean: result === "pass" ? 1 : 0,
          reviewedWithoutStatus: 1,
          targets: result === "finding" ? installed.targets : [],
        }),
      )
      expect(
        screen.getByText(
          result === "pass"
            ? "No concern found in the saved check result."
            : installed.targets[0]!.observation!,
        ),
      ).toBeVisible()
      expect(screen.queryByText("The check finished without a new result.")).toBeNull()
    },
  )

  it("removes an invalidated pass and does not reuse its explanation", () => {
    const explanation = "The saved check found no concern."
    const view = render(content({ clean: 1, checkingCount: 1, explanation }))
    expect(screen.getByText(explanation)).toBeVisible()
    view.rerender(
      content({
        clean: 0,
        checkingCount: 1,
        explanation,
        diagnostics: [{ ...diagnostic, code: "source-unavailable" }],
      }),
    )
    expect(screen.getByRole("region", { name: diagnostic.title })).toBeVisible()
    expect(screen.getByText("No check result yet.")).toBeVisible()
    expect(screen.queryByText(explanation)).toBeNull()
    expect(screen.queryByText(/Saved check result/)).toBeNull()
  })

  it("does not show active progress for a snoozed saved result", () => {
    render(content({ clean: 1, checkingCount: 1 }, true))
    expect(screen.queryByRole("region", { name: "Check status" })).toBeNull()
    expect(screen.getByText("No concern found in the saved check result.")).toBeVisible()
  })
})
