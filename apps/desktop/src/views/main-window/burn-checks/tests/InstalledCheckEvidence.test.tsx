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

const publicEventInputs = [
  {
    event: "user_message",
    value: {
      message: {
        id: "user-1",
        source: "codex",
        kind: "user_message",
        role: "user",
        content: "Review the parser plan.",
        quality: { source_complete: false },
      },
    },
    expected: ["message", "user_message", "Review the parser plan.", "false"],
  },
  {
    event: "assistant_message",
    value: {
      message: {
        id: "assistant-1",
        source: "codex",
        kind: "assistant_message",
        role: "assistant",
        content: "The parser review is complete.",
        quality: { source_complete: true },
      },
    },
    expected: ["message", "assistant_message", "The parser review is complete."],
  },
  {
    event: "tool_call",
    value: {
      message: {
        id: "call-1",
        source: "codex",
        kind: "tool_call",
        role: "assistant",
        tool: { call_id: "native-call-1", name: "custom", arguments: "review parser.rs" },
        quality: { source_complete: true, arguments_available: true },
      },
    },
    expected: ["message", "tool_call", "native-call-1", "custom", "review parser.rs"],
  },
  {
    event: "turn_end",
    value: {
      turn: {
        id: "cycle-1",
        source: "codex",
        status: "cancelled",
        messages: [
          {
            id: "cycle-user-1",
            source: "codex",
            kind: "user_message",
            role: "user",
            content: "Review the completed changes.",
            quality: { source_complete: true },
          },
        ],
      },
    },
    expected: ["turn", "cancelled", "messages", "Review the completed changes."],
  },
  {
    event: "session_start",
    value: { session: { id: "session-1", start_reason: "attached" } },
    expected: ["session", "start_reason", "attached"],
  },
  {
    event: "session_idle",
    value: { session: { id: "session-1", idle_seconds: 120 } },
    expected: ["session", "idle_seconds", "120"],
  },
]

describe("installed check assessed evidence", () => {
  it.each(publicEventInputs)(
    "keeps exact $event input separate for each decision without tool-result fields",
    async ({ event, value, expected }) => {
      const secondInput = { note: "Only this value went to the second decision." }
      const limitation = "This check covers only these saved decision input values."
      setupEvidence({
        status: "available",
        items: [
          {
            ...evidence.items[0]!,
            reference: "assessed-input:event-review",
            sourceLabel: "Assessed input: event-review",
            excerpt: JSON.stringify(value),
            limitation,
          },
          {
            ...evidence.items[0]!,
            reference: "assessed-input:summary",
            sourceLabel: "Assessed input: summary",
            excerpt: JSON.stringify(secondInput),
            limitation: null,
          },
        ],
      })
      fireEvent.click(await screen.findByRole("button", { name: "Show source details" }))
      const firstItem = (
        await screen.findByRole("heading", {
          name: "Assessed input: event-review",
        })
      ).closest("li")!
      const first = within(firstItem)
      for (const text of expected)
        expect(first.getAllByText(text).find((node) => !node.closest("details"))).toBeVisible()
      expect(first.getByText(limitation)).toBeVisible()
      expect(first.queryByText(secondInput.note)).not.toBeInTheDocument()
      const raw = first.getByText("Raw assessed input").closest("details")!
      expect(raw).not.toHaveAttribute("open")
      expect(JSON.parse(raw.querySelector("pre")!.textContent!)).toEqual(value)
      const second = within(
        screen
          .getByRole("heading", {
            name: "Assessed input: summary",
          })
          .closest("li")!,
      )
      expect(second.getByText(secondInput.note)).toBeVisible()
      expect(second.queryByText(expected[0]!)).not.toBeInTheDocument()
      expect(first.queryByText("result")).not.toBeInTheDocument()
      if (event !== "tool_call") expect(first.queryByText("tool")).not.toBeInTheDocument()
      expect(screen.queryByRole("region", { name: "Action arguments" })).toBeNull()
      expect(screen.queryByRole("region", { name: "Action result" })).toBeNull()
      expect(screen.queryByText(/Readable source details are unavailable/)).toBeNull()
    },
  )

  it("shows frozen current-file scope, descriptions, and mode-hint limits without opening raw JSON", () => {
    render(
      <InstalledCheckEvidence
        item={{
          ...evidence.items[0]!,
          reference: "assessed-input:review",
          excerpt: JSON.stringify({
            observed_files: {
              global: {
                instructions_complete: false,
                instructions_limits: ["managed-policy-unavailable"],
              },
              repo: {
                snapshot_revision: "frozen-snapshot",
                instructions: [
                  {
                    path: "/repo/AGENTS.md",
                    content: "Run the focused parser tests.",
                    scope: { root: "/repo", globs: ["src/**"] },
                    observed_at: "2026-10-23T12:00:00Z",
                  },
                ],
                skills: [
                  {
                    name: "review",
                    description: "Review parser changes.",
                    path: "/repo/a/SKILL.md",
                  },
                  {
                    name: "review",
                    description: "Review test coverage.",
                    path: "/repo/b/SKILL.md",
                  },
                ],
                skills_complete: false,
                skills_limits: ["configured-definitions-only"],
              },
            },
            activity: [
              { fast_mode: true, quality: { fast_mode_evidence: "model-id-suffix-hint" } },
            ],
          }),
        }}
      />,
    )
    for (const value of [
      "managed-policy-unavailable",
      "frozen-snapshot",
      "/repo/AGENTS.md",
      "Run the focused parser tests.",
      "src/**",
      "2026-10-23T12:00:00Z",
      "Review parser changes.",
      "Review test coverage.",
      "/repo/a/SKILL.md",
      "/repo/b/SKILL.md",
      "configured-definitions-only",
      "model-id-suffix-hint",
    ])
      expect(screen.getAllByText(value).find((node) => !node.closest("details"))).toBeVisible()
    expect(
      screen.getAllByText("review").filter((node) => !node.closest("details")),
    ).toHaveLength(2)
    const raw = screen.getByText("Raw assessed input").closest("details")!
    expect(raw).not.toHaveAttribute("open")
    expect(raw.querySelector("pre")).not.toBeVisible()
  })

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

  it.each([
    "assessed-input",
    "input-omissions",
    "assessed-input:review",
    "original-input:review",
    "input-omissions:review",
    "decision-connection",
  ])("renders %s as values even when it contains source-like keys", (reference) => {
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
  })

  it.each([
    ["assessed-input:first", "Raw assessed input"],
    ["original-input:first", "Raw original input"],
    ["input-omissions:first", "Raw input omissions"],
    ["decision-connection", "Raw decision connection"],
  ])("shows structured arrays and the correct raw label for %s", (reference, rawLabel) => {
    render(
      <InstalledCheckEvidence
        item={{
          ...evidence.items[0]!,
          reference,
          excerpt: JSON.stringify(["Saved input.", { flag: false }, [], null]),
        }}
      />,
    )
    for (const text of ["Saved input.", "flag", "false", "[]", "null"])
      expect(screen.getAllByText(text).find((node) => !node.closest("details"))).toBeVisible()
    const raw = screen.getByText(rawLabel!).closest("details")!
    expect(raw).not.toHaveAttribute("open")
    expect(raw.querySelector("pre")).not.toBeVisible()
    expect(screen.queryByText(/Readable source details are unavailable/)).toBeNull()
    expect(screen.queryByText("Raw source details")).toBeNull()
  })

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
      "Exact assessed input was not saved for this check. This is original source evidence; it does not show which values were sent to the model."
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

describe("installed local check evidence", () => {
  const explanation =
    "These frozen values were available to the local status actions. No model request was sent."
  const localItem: ipc.BurnCheckEvidenceItemPayload = {
    ...evidence.items[0]!,
    reference: "local-context",
    sourceLabel: "Frozen local context",
    explanation,
    limitation: null,
  }

  it.each(publicEventInputs)(
    "shows the real $event root for local status actions without model or tool-result claims",
    ({ event, value, expected }) => {
      render(<InstalledCheckEvidence item={{ ...localItem, excerpt: JSON.stringify(value) }} />)
      for (const text of expected)
        expect(screen.getAllByText(text).find((node) => !node.closest("details"))).toBeVisible()
      const raw = screen.getByText("Raw local context").closest("details")!
      expect(JSON.parse(raw.querySelector("pre")!.textContent!)).toEqual(value)
      expect(screen.getByText(explanation)).toBeVisible()
      expect(screen.queryByText("Raw assessed input")).not.toBeInTheDocument()
      expect(screen.queryByText("result")).not.toBeInTheDocument()
      if (event !== "tool_call") expect(screen.queryByText("tool")).not.toBeInTheDocument()
      expect(screen.queryByRole("region", { name: "Action arguments" })).toBeNull()
      expect(screen.queryByRole("region", { name: "Action result" })).toBeNull()
    },
  )

  it("shows the frozen local context without model input or connection claims", async () => {
    setupEvidence({
      status: "available",
      items: [{ ...localItem, excerpt: JSON.stringify({ session: { requests: [] } }) }],
    })
    fireEvent.click(await screen.findByRole("button", { name: "Show source details" }))
    const context = within(
      (await screen.findByRole("heading", { name: "Frozen local context" })).closest("li")!,
    )
    expect(context.getByText("session")).toBeVisible()
    expect(context.getByText("requests")).toBeVisible()
    expect(context.getByText("[]")).toBeVisible()
    expect(context.getByText(explanation)).toBeVisible()
    const raw = context.getByText("Raw local context").closest("details")!
    expect(raw).not.toHaveAttribute("open")
    expect(raw.querySelector("pre")).not.toBeVisible()
    expect(screen.queryByText("Raw assessed input")).toBeNull()
    expect(screen.queryByRole("heading", { name: "Decision connection" })).toBeNull()
    expect(screen.queryByText(/Exact assessed input was not saved/)).toBeNull()
  })

  it.each([
    {
      value: {
        text: "Frozen text.",
        tool: { name: "Frozen tool name." },
        quality: { source_complete: false },
      },
      expected: ["text", "tool", "quality", "Frozen text.", "Frozen tool name."],
    },
    {
      value: ["First frozen value.", { flag: true }],
      expected: ["First frozen value.", "flag", "true"],
    },
    { value: [], expected: ["[]"] },
    { value: {}, expected: ["{}"] },
    { value: "Frozen context text.", expected: ["Frozen context text."] },
    { value: null, expected: ["null"] },
  ])(
    "renders local JSON values without interpreting source-like fields: $value",
    ({ value, expected }) => {
      render(<InstalledCheckEvidence item={{ ...localItem, excerpt: JSON.stringify(value) }} />)
      for (const text of expected) {
        const readable = screen.getAllByText(text).find((node) => !node.closest("details"))
        expect(readable).toBeVisible()
      }
      expect(screen.queryByRole("region", { name: "Action arguments" })).toBeNull()
      expect(screen.queryByRole("region", { name: "Action result" })).toBeNull()
      expect(screen.queryByText("Source details are partial.")).toBeNull()
      expect(screen.queryByText("Earlier user requests may be missing.")).toBeNull()
      expect(screen.queryByText("Action unavailable")).toBeNull()
      expect(screen.getByText("Raw local context")).toBeVisible()
      expect(screen.getByText(explanation)).toBeVisible()
    },
  )

  it.each([true, false])(
    "keeps the long-text preview and limitation for local excerpts (JSON: %s)",
    (json) => {
      const excerpt = "Frozen value. ".repeat(100)
      const limitation = "This display contains only the saved excerpt."
      render(
        <InstalledCheckEvidence
          item={{
            ...localItem,
            excerpt: json ? JSON.stringify({ note: excerpt }) : excerpt,
            limitation,
          }}
        />,
      )
      expect(screen.getByRole("button", { name: "Show full details" })).toBeVisible()
      expect(screen.getByText(excerpt.slice(0, 360).trimEnd() + "…")).toBeVisible()
      fireEvent.click(screen.getByRole("button", { name: "Show full details" }))
      expect(screen.getByText(excerpt, { normalizer: (value) => value })).toBeVisible()
      expect(screen.getByText(limitation)).toBeVisible()
      expect(screen.getByText(explanation)).toBeVisible()
    },
  )
})
