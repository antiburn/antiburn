import { fireEvent, render, screen, within } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import * as ipc from "../../../../lib/insightsIpc"
import * as snoozes from "../../../../lib/snoozedBurnChecks"
import { InstalledCheckEvidence } from "../InstalledCheckEvidence"
import { report, setup } from "./burnChecksTestSupport"
import {
  installedCheckAssessedInputFixture as input,
  installedCheckEvidenceFixture as evidence,
  installedCheckFitReportFixture as fitReport,
  installedCheckFixture as installed,
} from "./installedCheckFixture"

beforeEach(() => {
  HTMLElement.prototype.scrollIntoView = vi.fn()
  vi.spyOn(snoozes, "useSnoozedBurnChecks").mockReturnValue({ status: "ready", records: [] })
})

afterEach(() => vi.restoreAllMocks())

function setupEvidence(payload: ipc.BurnCheckTargetEvidencePayload) {
  vi.spyOn(ipc, "getInstalledCheckEvidence").mockResolvedValue(payload)
  return setup(
    null,
    false,
    { wins: [] },
    { ...report, categories: [], installedChecks: [installed] },
  )
}

describe("installed check assessed evidence", () => {
  it("renders the saved input and FitReport with exact omission markers and backend limits", async () => {
    setupEvidence(evidence)
    fireEvent.click(await screen.findByRole("button", { name: "Show source details" }))

    const assessed = within(
      (await screen.findByRole("heading", { name: "Assessed input" })).closest("li")!,
    )
    for (const field of ["requests", "request_history_complete", "source_coverage", "activity"])
      expect(assessed.getByText(field)).toBeVisible()
    expect(assessed.getByText(input.requests[0]!.text)).toBeVisible()
    expect(assessed.getByText("cargo test parser")).toBeVisible()
    expect(assessed.getByText("partial")).toBeVisible()
    for (const value of assessed.getAllByText("false")) expect(value).toBeVisible()
    const retained = assessed.getByText(input.activity[0]!.tool.result.content, {
      normalizer: (value) => value,
    })
    expect(retained).toBeVisible()
    expect(retained).toHaveClass("whitespace-pre-wrap")
    expect(retained.textContent).toBe(input.activity[0]!.tool.result.content)
    expect(retained.textContent?.match(/\[\.\.\. omitted \.\.\.\]/g)).toHaveLength(1)
    expect(assessed.getByText(evidence.items[0]!.explanation)).toBeVisible()
    expect(assessed.getByText(evidence.items[0]!.limitation!)).toBeVisible()

    const omissions = within(
      screen.getByRole("heading", { name: "Input omissions" }).closest("li")!,
    )
    expect(omissions.getByText(fitReport.omissions[0]!.path)).toBeVisible()
    expect(omissions.getByText("truncate")).toBeVisible()
    expect(omissions.getByText(String(fitReport.omissions[0]!.original_count))).toBeVisible()
    expect(omissions.getByText(String(fitReport.omissions[0]!.retained_count))).toBeVisible()
    expect(omissions.getByText(evidence.items[1]!.explanation)).toBeVisible()
    for (const rawLabel of ["Raw assessed input", "Raw input omissions"]) {
      const raw = screen.getByText(rawLabel).closest("details")!
      expect(raw).not.toHaveAttribute("open")
      expect(raw.querySelector("pre")).not.toBeVisible()
    }
    expect(screen.queryByText(/Readable source details are unavailable/)).toBeNull()
    expect(screen.queryByRole("region", { name: "Action arguments" })).toBeNull()
    expect(screen.queryByRole("region", { name: "Action result" })).toBeNull()
    expect(screen.queryByText("Source details are partial.")).toBeNull()
    expect(screen.queryByText(/Original source:/)).toBeNull()
  })

  it.each(["assessed-input", "input-omissions"])(
    "renders %s as values even when it contains source-like keys",
    (reference) => {
      render(
        <InstalledCheckEvidence
          item={{
            ...evidence.items[0]!,
            reference,
            excerpt: JSON.stringify({
              text: "Selected expression text.",
              tool: { name: "Selected expression tool name." },
              quality: { source_complete: false, request_history_complete: false },
            }),
          }}
        />,
      )
      for (const field of ["text", "tool", "quality", "request_history_complete"])
        expect(screen.getByText(field)).toBeVisible()
      expect(screen.getByText("Selected expression text.")).toBeVisible()
      expect(screen.getByText("Selected expression tool name.")).toBeVisible()
      expect(screen.queryByText("Action unavailable")).toBeNull()
      expect(screen.queryByRole("region", { name: "Action arguments" })).toBeNull()
      expect(screen.queryByText("Source details are partial.")).toBeNull()
      expect(screen.queryByText("Earlier user requests may be missing.")).toBeNull()
    },
  )

  it.each([
    { value: null, text: "null" },
    { value: false, text: "false" },
    { value: 42, text: "42" },
    { value: [], text: "[]" },
    { value: {}, text: "{}" },
    { value: "Selected text.", text: "Selected text." },
  ])("shows the saved JSON value $text without marking it unavailable", ({ value, text }) => {
    render(
      <InstalledCheckEvidence
        item={{ ...evidence.items[0]!, excerpt: JSON.stringify(value) }}
      />,
    )
    const readableValue = screen.getAllByText(text).find((node) => !node.closest("details"))
    expect(readableValue).toBeVisible()
    expect(screen.queryByText(/Unavailable|Readable source details are unavailable/)).toBeNull()
  })

  it("retains original-source headings and the unsaved-input limit for legacy evidence", async () => {
    const limitation =
      "Exact assessed input was not saved for this assessment. This is original source evidence; it does not show which values were sent to the model."
    setupEvidence({
      status: "available",
      items: [
        {
          ...evidence.items[0]!,
          reference: "request:0",
          sourceLabel: "Original source: requested task",
          excerpt: JSON.stringify({ text: "Run the original parser tests." }),
          explanation: "",
          limitation,
        },
        {
          ...evidence.items[0]!,
          label: "observedAction",
          reference: "tool-result-1",
          sourceLabel: "Original source: action result",
          excerpt: JSON.stringify({
            tool: {
              name: "bash",
              arguments: { command: "cargo test original_parser" },
              result: { status: "error", content: "Original parser failure." },
            },
          }),
          explanation: "",
          limitation,
        },
      ],
    })
    fireEvent.click(await screen.findByRole("button", { name: "Show source details" }))
    const requested = within(
      (await screen.findByRole("heading", { name: "Original source: requested task" })).closest(
        "li",
      )!,
    )
    expect(requested.getByText("Run the original parser tests.")).toBeVisible()
    expect(requested.getByText(limitation)).toBeVisible()
    const action = within(
      screen.getByRole("heading", { name: "Original source: action result" }).closest("li")!,
    )
    expect(action.getByText("Original parser failure.")).toBeVisible()
    expect(action.getByText(limitation)).toBeVisible()
    expect(screen.queryByRole("heading", { name: "Assessed input" })).toBeNull()
  })
})
