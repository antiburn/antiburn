import { cleanup, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it } from "vitest"

import type { SessionHygieneCheck } from "../../lib/presentation/sessionHygiene"
import { SessionStatusBar } from "./SessionStatusBar"

const CHECKS: SessionHygieneCheck[] = [
  {
    id: "bloatedInitialContext",
    status: "finding",
    notAssessedReason: null,
    title: "Bloated initial context detected",
    ink: "system-red-text",
  },
  {
    id: "reasoningOverkill",
    status: "clean",
    notAssessedReason: null,
    title: "No reasoning overkill detected",
    ink: "system-green",
  },
  {
    id: "excessCacheRehydration",
    status: "clean",
    notAssessedReason: null,
    title: "No excess cache rehydration detected",
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
    title: "Excess cache rehydration not assessed",
    ink: "label-tertiary",
  },
]

afterEach(cleanup)

describe("SessionStatusBar", () => {
  it("inks a fill-free line green with the full pass count when every check passes", () => {
    render(<SessionStatusBar checks={ALL_PASSED} />)
    const verdict = screen.getByLabelText("All checks pass")
    expect(verdict.textContent).toBe("3/3 checks pass")
    expect(verdict.style.color).toBe("var(--color-system-green)")
    expect(verdict.className).not.toContain("bg-")
    expect(verdict.parentElement?.className).not.toContain("bg-")
  })

  it("inks a minority failure toward the orange end of the ramp", () => {
    render(<SessionStatusBar checks={CHECKS} />)
    const verdict = screen.getByLabelText("1 of 3 assessed checks failed")
    expect(verdict.textContent).toBe("2/3 checks pass")
    expect(verdict.className).not.toContain("rounded-full")
    expect(verdict.style.backgroundColor).toBe("")
    expect(verdict.style.color).toContain("--color-system-red-text) 33%")
    expect(verdict.style.color).toContain("--color-system-orange")
    expect(verdict.parentElement?.className).not.toContain("bg-")
  })

  it("counts findings, clean checks, and unassessed checks separately", () => {
    render(<SessionStatusBar checks={WITH_NOT_ASSESSED} />)
    const verdict = screen.getByLabelText("1 of 2 assessed checks failed; 1 not assessed")
    expect(verdict.textContent).toBe("1/2 checks pass · 1 not assessed")
    expect(verdict.parentElement?.getAttribute("data-state")).toBeDefined()
  })

  it("reaches full red ink when every assessed check fails", () => {
    const allFailed = CHECKS.map((check) => ({
      ...check,
      status: "finding" as const,
      ink: "system-red-text" as const,
    }))
    render(<SessionStatusBar checks={allFailed} />)
    const verdict = screen.getByLabelText("3 of 3 assessed checks failed")
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
    expect(verdict.textContent).toBe("Computing checks")
    expect(verdict.style.color).toBe("var(--color-label-tertiary)")
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

  it("keeps the last-activity time hidden until the host row hover", () => {
    render(<SessionStatusBar checks={ALL_PASSED} timestamp={new Date().toISOString()} />)
    const time = screen.getByLabelText(/^Last activity /)
    expect(time.className).toContain("opacity-0")
    expect(time.className).toContain("group-hover:opacity-100")
  })

  it("omits the cost figure when nothing priced the session", () => {
    render(<SessionStatusBar checks={ALL_PASSED} cost={null} />)
    expect(screen.queryByLabelText(/cost/)).toBeNull()
  })
})
