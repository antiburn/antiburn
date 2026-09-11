import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it } from "vitest"

import type { SessionHygieneCheck } from "../../lib/presentation/sessionHygiene"
import { SessionStatusBar } from "./SessionStatusBar"

const CHECKS: SessionHygieneCheck[] = [
  {
    id: "sessionOverdepth",
    status: "finding",
    notAssessedReason: null,
    detail: null,
    title: "Session overdepth detected",
    name: "Session overdepth detected",
    ink: "system-red-text",
  },
  {
    id: "modelOverthinking",
    status: "clean",
    notAssessedReason: null,
    detail: null,
    title: "No model overthinking detected",
    name: "Model overthinking detected",
    ink: "system-green",
  },
  {
    id: "overpoweredSubagents",
    status: "clean",
    notAssessedReason: null,
    detail: null,
    title: "No overpowered subagents detected",
    name: "Overpowered subagents detected",
    ink: "system-green",
  },
  {
    id: "obsoleteModel",
    status: "clean",
    notAssessedReason: null,
    detail: null,
    title: "No obsolete model detected",
    name: "Obsolete model detected",
    ink: "system-green",
  },
  {
    id: "fastModeOveruse",
    status: "clean",
    notAssessedReason: null,
    detail: null,
    title: "No fast mode overuse detected",
    name: "Fast mode overuse detected",
    ink: "system-green",
  },
  {
    id: "excessCacheRehydration",
    status: "clean",
    notAssessedReason: null,
    detail: null,
    title: "No excess cache rehydration detected",
    name: "Excess cache rehydration detected",
    ink: "system-green",
  },
]

const ALL_PASSED = CHECKS.map((check) => ({
  ...check,
  status: "clean" as const,
  ink: "system-green" as const,
}))

const WITH_NOT_ASSESSED: SessionHygieneCheck[] = [
  CHECKS[0]!,
  CHECKS[1]!,
  {
    ...CHECKS[2]!,
    status: "notAssessed",
    notAssessedReason: "incompleteEvidence",
    title: "Overpowered subagents not assessed",
    ink: "label-tertiary",
  },
  CHECKS[3]!,
  CHECKS[4]!,
  CHECKS[5]!,
]

afterEach(cleanup)

describe("SessionStatusBar", () => {
  it("shows the complete pass wording with a cyan terminal tick", () => {
    render(<SessionStatusBar checks={ALL_PASSED} />)
    const verdict = screen.getByLabelText(/All Burn Checks passed/)
    expect(verdict).toHaveTextContent("All 6 passed")
    expect(screen.getByText("All 6 passed")).toHaveClass(
      "font-semibold!",
      "text-burn-check-pass-fill",
    )
    expect(verdict).toHaveClass("font-mono", "type-footnote", "tabular-nums")
    expect(verdict).not.toHaveClass("type-callout")
    expect(verdict).not.toHaveClass("tracking-tight!")
    expect(verdict.querySelector('[data-burn-check-indicator="pass"]')).not.toBeNull()
  })

  it("uses result-first mixed wording and failure ink", () => {
    render(<SessionStatusBar checks={CHECKS} />)
    const verdict = screen.getByLabelText(/Some Burn Checks failed/)
    expect(verdict).toHaveTextContent("1 failed·5 passed")
    expect(screen.getByText("1 failed")).toHaveClass("text-burn-check-failure-text")
    expect(screen.getByText("5 passed")).toHaveClass("text-burn-check-pass-fill")
    expect(screen.getByText("5 passed")).not.toHaveClass("font-semibold!")
    expect(verdict.querySelectorAll("circle")).toHaveLength(2)
  })

  it("omits unavailable checks from compact wording", () => {
    render(<SessionStatusBar checks={WITH_NOT_ASSESSED} />)
    const verdict = screen.getByLabelText(/6 session checks/)
    expect(verdict).toHaveTextContent("1 failed·4 passed")
    expect(verdict).not.toHaveTextContent("not assessed")
  })

  it("uses a pass mark while disclosing partial all-pass coverage", () => {
    const checks = WITH_NOT_ASSESSED.map((check) =>
      check.status === "finding"
        ? { ...check, status: "clean" as const, ink: "system-green" as const }
        : check,
    )
    render(<SessionStatusBar checks={checks} />)
    const verdict = screen.getByLabelText(/All assessed Burn Checks passed/)
    expect(verdict).toHaveTextContent("All 5 passed")
    expect(screen.getByText("All 5 passed")).toHaveClass("text-burn-check-pass-fill")
    expect(verdict.querySelector('[data-burn-check-indicator="pass"]')).not.toBeNull()
  })

  it("uses the terminal failure mark only when every check fails", () => {
    const allFailed = CHECKS.map((check) => ({
      ...check,
      status: "finding" as const,
      ink: "system-red-text" as const,
    }))
    render(<SessionStatusBar checks={allFailed} />)
    const verdict = screen.getByLabelText(/All Burn Checks failed/)
    expect(verdict).toHaveTextContent("6 failed")
    expect(verdict.querySelector('[data-burn-check-indicator="fail"]')).not.toBeNull()
  })

  it("shows a computing state instead of claiming a clean result", () => {
    const notAssessed = WITH_NOT_ASSESSED.map((check) => ({
      ...check,
      status: "notAssessed" as const,
      notAssessedReason: "incompleteEvidence" as const,
      ink: "label-tertiary" as const,
    }))
    render(<SessionStatusBar checks={notAssessed} evidenceState="processing" />)
    const verdict = screen.getByLabelText(/Running Burn Checks/)
    expect(verdict).toHaveTextContent("Running Burn Checks…")
    expect(verdict.querySelector('[data-burn-check-indicator="running"]')).not.toBeNull()
  })

  it("keeps the ellipsis off settled evidence states", () => {
    render(<SessionStatusBar checks={[]} evidenceState="unsupported" />)
    const verdict = screen.getByLabelText("Burn Checks not supported")
    expect(verdict).toHaveTextContent("Burn Checks not supported")
  })

  it("shows the verdict, not the state text, once at least one check is assessed", () => {
    render(<SessionStatusBar checks={CHECKS} evidenceState="processing" />)
    const verdict = screen.getByLabelText(/Burn Checks incomplete/)
    expect(verdict).toHaveTextContent("1 failed·5 passed")
  })

  it("prefixes the transient state onto an assessed but stale verdict", () => {
    render(<SessionStatusBar checks={CHECKS} evidenceState="stale" />)
    const verdict = screen.getByLabelText(/Refreshing/)
    expect(verdict).toHaveTextContent("1 failed·5 passed")
  })

  it("uses only the assessed checks for a singular result", () => {
    const oneAssessed: SessionHygieneCheck[] = [
      CHECKS[0]!,
      { ...WITH_NOT_ASSESSED[2]! },
      {
        ...CHECKS[1]!,
        status: "notAssessed",
        notAssessedReason: "incompleteEvidence",
        title: "Model overthinking not assessed",
        ink: "label-tertiary",
      },
    ]
    render(<SessionStatusBar checks={oneAssessed} />)
    const verdict = screen.getByLabelText(/3 session checks/)
    expect(verdict).toHaveTextContent("1 failed")
  })

  it("names a settled result when no checks were assessed", () => {
    const noneAssessed = CHECKS.map((check) => ({
      ...check,
      status: "notAssessed" as const,
      notAssessedReason: "capabilityMissing" as const,
      ink: "label-tertiary" as const,
    }))
    render(<SessionStatusBar checks={noneAssessed} evidenceState="ready" />)
    expect(screen.getByText("Burn Checks not assessed")).toBeInTheDocument()
    expect(screen.getByLabelText(/6 not assessed/)).toBeInTheDocument()
  })

  it("keeps the cost at the right edge when no checks were assessed", () => {
    render(
      <SessionStatusBar checks={[]} cost={{ totalUsd: 2.4, figureLabel: "Estimated cost" }} />,
    )

    expect(screen.getByLabelText("Estimated cost $2.40").parentElement).toHaveClass("ml-auto")
  })

  it("keeps a limit share at the right edge when no checks were assessed", () => {
    render(
      <SessionStatusBar
        checks={[]}
        limitBadge={{ label: "Estimated weekly share", percent: 2.4 }}
      />,
    )

    expect(screen.getByLabelText("Estimated weekly share").parentElement).toHaveClass("ml-auto")
  })

  it("shows no limit for a badge with no percent and no unknown flag", () => {
    render(
      <SessionStatusBar
        checks={[]}
        limitBadge={{ label: "No weekly limit for this session.", percent: null }}
      />,
    )

    expect(screen.getByLabelText("No weekly limit for this session.")).toHaveTextContent(
      "no limit",
    )
  })

  it("shows unknown for a badge with no percent that is flagged unknown", () => {
    render(
      <SessionStatusBar
        checks={[]}
        limitBadge={{
          label: "Share of your weekly limit is not known for this session.",
          percent: null,
          unknown: true,
        }}
      />,
    )

    expect(
      screen.getByLabelText("Share of your weekly limit is not known for this session."),
    ).toHaveTextContent("unknown")
  })

  it("keeps unavailable checks in the tooltip detail", async () => {
    render(<SessionStatusBar checks={WITH_NOT_ASSESSED} />)
    fireEvent.focus(screen.getByLabelText(/6 session checks/))

    expect(await screen.findByText("Burn Checks")).toHaveClass(
      "type-callout",
      "font-semibold!",
      "text-label",
    )
    expect(await screen.findByText("Session overdepth detected")).toBeTruthy()
    expect(screen.getByText("Overpowered subagents")).toBeInTheDocument()
    expect(screen.queryByText("Overpowered subagents not assessed")).toBeNull()
    expect(screen.getByText("Failed · 1")).toHaveClass("text-label-tertiary")
    expect(screen.getByText("Failed · 1")).not.toHaveClass("text-burn-check-failure-text")
    expect(screen.getByText("Session overdepth detected")).toHaveClass(
      "text-burn-check-failure-text",
    )
    const groups = screen
      .getByText("Burn Checks")
      .closest("[data-burn-check-tooltip]")!
      .querySelectorAll("[data-burn-check-tooltip-group]")
    expect([...groups].map((group) => group.textContent)).toEqual([
      "Failed · 1",
      "Passed · 4",
      "Not assessed · 1",
    ])
    expect(screen.queryByText("Open the session for details")).toBeNull()
  })

  it("uses the Burn Check status icons without repeating their labels", async () => {
    render(<SessionStatusBar checks={WITH_NOT_ASSESSED} />)
    fireEvent.focus(screen.getByLabelText(/6 session checks/))

    await screen.findByText("Burn Checks")
    const marks = document.querySelectorAll("[data-burn-check-tooltip-mark]")
    expect(marks).toHaveLength(6)
    for (const mark of marks) {
      expect(mark.tagName.toLowerCase()).toBe("svg")
      expect(mark).toHaveAttribute("aria-hidden", "true")
      expect(mark).toHaveAttribute("width", "14")
      expect(mark).toHaveAttribute("height", "14")
      expect(mark).toHaveAttribute(
        "stroke-width",
        mark.getAttribute("data-burn-check-tooltip-mark") === "finding" ? "2.5" : "2",
      )
    }
    expect(document.querySelectorAll('[data-burn-check-tooltip-mark="finding"]')).toHaveLength(
      1,
    )
    expect(document.querySelectorAll('[data-burn-check-tooltip-mark="clean"]')).toHaveLength(4)
    expect(
      document.querySelectorAll('[data-burn-check-tooltip-mark="notAssessed"]'),
    ).toHaveLength(1)
    expect(document.querySelector('[data-burn-check-tooltip-mark="finding"]')).toHaveClass(
      "lucide-circle-alert",
      "text-burn-check-failure-fill",
    )
    expect(document.querySelector('[data-burn-check-tooltip-mark="clean"]')).toHaveClass(
      "lucide-circle-check",
      "text-burn-check-pass-fill",
    )
    expect(document.querySelector('[data-burn-check-tooltip-mark="notAssessed"]')).toHaveClass(
      "lucide-circle-minus",
      "text-burn-check-neutral",
    )
  })

  it("shows a usual cost figure without pill chrome", () => {
    render(
      <SessionStatusBar
        checks={ALL_PASSED}
        cost={{ totalUsd: 2.4, figureLabel: "Estimated cost" }}
      />,
    )
    const figure = screen.getByLabelText("Estimated cost $2.40")
    expect(figure.className).not.toContain("rounded-full")
    expect(figure.className).not.toContain("bg-label-tertiary/15")
  })

  it("wraps an unusual cost in the hot pill, at the usual cost's size", () => {
    render(
      <SessionStatusBar
        checks={ALL_PASSED}
        cost={{ totalUsd: 24, figureLabel: "Estimated cost", isHighCost: true }}
      />,
    )
    const figure = screen.getByLabelText("Estimated cost $24.00, higher than usual")
    expect(figure.className).toContain("type-footnote")
    expect(figure.className).toContain("font-mono")
    expect(figure.className).toContain("font-medium")
    expect(figure.className).toContain("rounded-full")
    expect(figure.className).toContain("bg-brand-tint")
  })

  it("keeps the hot cost pill in pass and fail rows alike", () => {
    const cost = { totalUsd: 24, figureLabel: "Estimated cost", isHighCost: true }
    const figureClasses = () =>
      screen.getByLabelText("Estimated cost $24.00, higher than usual").className

    const passing = render(<SessionStatusBar checks={ALL_PASSED} cost={cost} />)
    expect(figureClasses()).toContain("bg-brand-tint")

    passing.unmount()
    render(<SessionStatusBar checks={CHECKS} cost={cost} />)
    expect(figureClasses()).toContain("bg-brand-tint")
  })

  it("uses the hot pill for a session that consumes at least five percent", () => {
    render(
      <SessionStatusBar
        checks={ALL_PASSED}
        limitBadge={{ label: "Estimated weekly share", percent: 5 }}
      />,
    )

    const figure = screen.getByLabelText(
      "Estimated weekly share This session uses 5% or more of your limit.",
    )
    expect(figure.className).toContain("rounded-full")
    expect(figure.className).toContain("bg-brand-tint")
    expect(figure.className).toContain("text-white")
    expect(figure.querySelector("svg")).not.toBeNull()
  })

  it("uses the hot pill when a limit share rounds to five percent", () => {
    render(
      <SessionStatusBar
        checks={ALL_PASSED}
        limitBadge={{ label: "Estimated weekly share", percent: 4.99 }}
      />,
    )

    const figure = screen.getByLabelText(
      "Estimated weekly share This session uses 5% or more of your limit.",
    )
    expect(figure).toHaveTextContent("5%")
    expect(figure.className).toContain("rounded-full")
    expect(figure.className).toContain("bg-brand-tint")
    expect(figure.querySelector("svg")).not.toBeNull()
  })

  it("keeps a displayed value below five percent as plain text", () => {
    render(
      <SessionStatusBar
        checks={ALL_PASSED}
        limitBadge={{ label: "Estimated weekly share", percent: 4.94 }}
      />,
    )

    const figure = screen.getByLabelText("Estimated weekly share")
    expect(figure).toHaveTextContent("4.9%")
    expect(figure.className).not.toContain("rounded-full")
    expect(figure.className).not.toContain("bg-brand-tint")
    expect(figure.querySelector("svg")).toBeNull()
  })

  it.each([
    [1.46, "1.5%"],
    [2.44, "2.4%"],
    [7.76, "7.8%"],
    [1.04, "1%"],
  ])("formats a %s limit share as %s", (percent, expected) => {
    render(
      <SessionStatusBar
        checks={ALL_PASSED}
        limitBadge={{ label: "Estimated weekly share", percent }}
      />,
    )

    expect(screen.getByText(expected)).toBeInTheDocument()
  })

  it("omits the cost figure when nothing priced the session", () => {
    render(<SessionStatusBar checks={ALL_PASSED} cost={null} />)
    expect(screen.queryByLabelText(/cost/)).toBeNull()
  })
})
