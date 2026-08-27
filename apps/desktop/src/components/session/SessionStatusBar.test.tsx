import { cleanup, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it } from "vitest"

import type { MockSessionHygieneCheck } from "../../lib/presentation/mockSessionHygiene"
import { SessionStatusBar } from "./SessionStatusBar"

const CHECKS: MockSessionHygieneCheck[] = [
  { id: "sessionOverdepth", passed: false, title: "Session overdepth detected" },
  { id: "modelOverthinking", passed: true, title: "No model overthinking detected" },
  { id: "excessCacheRehydration", passed: true, title: "No excess cache rehydration detected" },
]

const ALL_PASSED = CHECKS.map((check) => ({ ...check, passed: true }))

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
    const verdict = screen.getByLabelText("1 of 3 checks failed")
    expect(verdict.textContent).toBe("2/3 checks pass")
    // Severity lives in the ink only — the verdict is never a badge.
    expect(verdict.className).not.toContain("rounded-full")
    expect(verdict.style.backgroundColor).toBe("")
    // One failure of three is a third of the way from orange to red.
    expect(verdict.style.color).toContain("--color-system-red-text) 33%")
    expect(verdict.style.color).toContain("--color-system-orange")
    // The line behind the verdict stays fill-free.
    expect(verdict.parentElement?.className).not.toContain("bg-")
  })

  it("reaches full red ink when every check fails", () => {
    const allFailed = CHECKS.map((check) => ({ ...check, passed: false }))
    render(<SessionStatusBar checks={allFailed} />)
    const verdict = screen.getByLabelText("3 of 3 checks failed")
    expect(verdict.style.color).toContain("--color-system-red-text) 100%")
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
