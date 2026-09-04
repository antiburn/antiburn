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
  it("inks a fill-free line green with the full pass count when every check passes", () => {
    render(<SessionStatusBar checks={ALL_PASSED} />)
    const verdict = screen.getByLabelText("All checks passed")
    expect(verdict.textContent).toBe("6/6 burn checks")
    expect(verdict.style.color).toBe("var(--color-system-green)")
    expect(verdict.className).not.toContain("bg-")
    expect(verdict.parentElement?.className).not.toContain("bg-")
  })

  it("inks a minority failure toward the orange end of the ramp", () => {
    render(<SessionStatusBar checks={CHECKS} />)
    const verdict = screen.getByLabelText("5 of 6 burn checks passed")
    expect(verdict.textContent).toBe("5/6 burn checks")
    expect(verdict.className).not.toContain("rounded-full")
    expect(verdict.style.backgroundColor).toBe("")
    expect(verdict.style.color).toContain("--color-system-red-text) 17%")
    expect(verdict.style.color).toContain("--color-system-orange")
    expect(verdict.parentElement?.className).not.toContain("bg-")
  })

  it("excludes unavailable checks from the result", () => {
    render(<SessionStatusBar checks={WITH_NOT_ASSESSED} />)
    const verdict = screen.getByLabelText("4 of 5 burn checks passed")
    expect(verdict.textContent).toBe("4/5 burn checks")
    expect(verdict.style.color).toContain("--color-system-red-text) 20%")
    expect(verdict.style.color).toContain("--color-system-orange")
  })

  it("qualifies an all-pass result when unavailable checks are hidden", () => {
    const checks = WITH_NOT_ASSESSED.map((check) =>
      check.status === "finding"
        ? { ...check, status: "clean" as const, ink: "system-green" as const }
        : check,
    )
    render(<SessionStatusBar checks={checks} />)
    const verdict = screen.getByLabelText("All assessed checks passed")
    expect(verdict.textContent).toBe("5/5 assessed burn checks")
    expect(screen.queryByLabelText("All checks passed")).toBeNull()
  })

  it("reaches full red ink only when every check fails", () => {
    const allFailed = CHECKS.map((check) => ({
      ...check,
      status: "finding" as const,
      ink: "system-red-text" as const,
    }))
    render(<SessionStatusBar checks={allFailed} />)
    const verdict = screen.getByLabelText("0 of 6 burn checks passed")
    expect(verdict.style.color).toContain("--color-system-red-text) 100%")
  })

  it("shows a computing state instead of claiming a clean result", () => {
    const notAssessed = WITH_NOT_ASSESSED.map((check) => ({
      ...check,
      status: "notAssessed" as const,
      notAssessedReason: "incompleteEvidence" as const,
      ink: "label-tertiary" as const,
    }))
    render(<SessionStatusBar checks={notAssessed} evidenceState="processing" />)
    const verdict = screen.getByLabelText("Computing session hygiene checks")
    expect(verdict.textContent).toBe("Computing checks…")
    expect(verdict.style.color).toBe("var(--color-label-tertiary)")
  })

  it("keeps the ellipsis off settled evidence states", () => {
    render(<SessionStatusBar checks={[]} evidenceState="unsupported" />)
    const verdict = screen.getByLabelText("Unsupported session hygiene checks")
    expect(verdict.textContent).toBe("Unsupported checks")
  })

  it("shows the verdict, not the state text, once at least one check is assessed", () => {
    render(<SessionStatusBar checks={CHECKS} evidenceState="processing" />)
    const verdict = screen.getByLabelText("Computing — 5 of 6 burn checks passed")
    expect(verdict.textContent).toBe("5/6 burn checks")
  })

  it("prefixes the transient state onto an assessed but stale verdict", () => {
    render(<SessionStatusBar checks={CHECKS} evidenceState="stale" />)
    const verdict = screen.getByLabelText("Refreshing — 5 of 6 burn checks passed")
    expect(verdict.textContent).toBe("5/6 burn checks")
    expect(verdict.style.color).toContain("--color-system-orange")
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
    const verdict = screen.getByLabelText("0 of 1 burn check passed")
    expect(verdict.textContent).toBe("0/1 burn check")
    expect(verdict.style.color).toContain("--color-system-red-text) 100%")
  })

  it("omits a settled result when no checks were assessed", () => {
    const noneAssessed = CHECKS.map((check) => ({
      ...check,
      status: "notAssessed" as const,
      notAssessedReason: "capabilityMissing" as const,
      ink: "label-tertiary" as const,
    }))
    render(<SessionStatusBar checks={noneAssessed} evidenceState="ready" />)
    expect(screen.queryByText(/burn check/i)).toBeNull()
    expect(screen.queryByText("0/0")).toBeNull()
    expect(screen.queryByLabelText(/assessed/i)).toBeNull()
  })

  it("omits unavailable checks from the tooltip", async () => {
    render(<SessionStatusBar checks={WITH_NOT_ASSESSED} />)
    fireEvent.focus(screen.getByLabelText("4 of 5 burn checks passed"))

    expect(await screen.findByText("Session overdepth detected")).toBeTruthy()
    expect(screen.queryByText("Overpowered subagents detected")).toBeNull()
    expect(screen.queryByText("couldn't read the whole session log")).toBeNull()
  })

  it("marks each assessed status with a named icon, not a text glyph", async () => {
    render(<SessionStatusBar checks={WITH_NOT_ASSESSED} />)
    fireEvent.focus(screen.getByLabelText("4 of 5 burn checks passed"))

    for (const label of ["Finding", "Passed"]) {
      const marks = await screen.findAllByLabelText(label)
      expect(marks.every((mark) => mark.tagName.toLowerCase() === "svg")).toBe(true)
    }
    expect(screen.queryByLabelText("Not assessed")).toBeNull()
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

  it("keeps a smaller limit share as plain text", () => {
    render(
      <SessionStatusBar
        checks={ALL_PASSED}
        limitBadge={{ label: "Estimated weekly share", percent: 4.99 }}
      />,
    )

    const figure = screen.getByLabelText("Estimated weekly share")
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
