import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it } from "vitest"

import type { SessionHygienePayload } from "../../../lib/insightsIpc"
import {
  INITIAL_SESSION_HYGIENE,
  sessionHygieneChecks,
  sessionHygieneDocumentation,
} from "../../../lib/presentation/sessionHygiene"
import { HygieneBreakdown } from "./HygieneBreakdown"

afterEach(cleanup)

const PAYLOAD: SessionHygienePayload = {
  badges: [
    {
      id: "sessionOverdepth",
      status: "finding",
      notAssessedReason: null,
      findingEvidence: {
        kind: "sessionOverdepth",
        maxRequestContextTokens: 475_000,
        depthCapTokens: 400_000,
      },
    },
    { id: "modelOverthinking", status: "clean", notAssessedReason: null },
    {
      id: "overpoweredSubagents",
      status: "notAssessed",
      notAssessedReason: "incompleteEvidence",
    },
    { id: "obsoleteModel", status: "clean", notAssessedReason: null },
    { id: "fastModeOveruse", status: "clean", notAssessedReason: null },
    { id: "excessCacheRehydration", status: "clean", notAssessedReason: null },
  ],
  evidenceState: "ready",
}

function view() {
  return render(<HygieneBreakdown checks={sessionHygieneChecks(PAYLOAD)} />)
}

describe("HygieneBreakdown", () => {
  it.each([true, false])("uses Burn Check marks with inlineGuidance=%s", (inlineGuidance) => {
    render(
      <HygieneBreakdown
        checks={sessionHygieneChecks(PAYLOAD)}
        inlineGuidance={inlineGuidance}
        collapsePassing={false}
      />,
    )

    for (const { name, iconClass, colorClass, strokeWidth } of [
      {
        name: "Session overdepth",
        iconClass: "lucide-circle-alert",
        colorClass: "text-burn-check-failure-fill",
        strokeWidth: "2.5",
      },
      {
        name: "Model overthinking",
        iconClass: "lucide-circle-check",
        colorClass: "text-burn-check-pass-fill",
        strokeWidth: "2",
      },
    ]) {
      const row = inlineGuidance
        ? screen.getByRole("group", { name })
        : screen.getByRole("button", { name: `${name} details` }).parentElement!
      const icon = row.querySelector(`.${iconClass}`)
      expect(icon).toHaveClass(colorClass)
      expect(icon).toHaveAttribute("stroke-width", strokeWidth)
      expect(icon).toHaveAttribute("width", "14")
      expect(icon).toHaveAttribute("height", "14")
      expect(icon).toHaveAttribute("aria-hidden", "true")
    }

    if (!inlineGuidance) {
      expect(screen.getByText("Failed")).toHaveClass("text-burn-check-failure-text")
      for (const word of screen.getAllByText("Passed")) {
        expect(word).toHaveClass("text-label-secondary")
      }
    }
  })

  it("carries each check at three densities and opens its advice in a tooltip", () => {
    const checks = sessionHygieneChecks(PAYLOAD)
    render(<HygieneBreakdown checks={checks} inlineGuidance />)
    expect(screen.queryByRole("button")).toBeNull()
    expect(screen.getAllByRole("group")).toHaveLength(5)
    expect(screen.queryByText("Overpowered subagents")).toBeNull()

    for (const check of checks.filter((item) => item.status !== "notAssessed")) {
      const documentation = sessionHygieneDocumentation(check)
      const row = screen.getByRole("group", { name: check.name })
      // The name shows at every width. The verdict word and the summary
      // ride along in the markup, and the stylesheet reveals each of them
      // at the pane width that has the room for it.
      expect(row).toHaveAttribute("tabindex", "0")
      expect(row).toHaveClass("session-check-cell")
      expect(row.querySelector(".session-check-word")).toHaveTextContent(
        check.status === "finding" ? "Failed" : "Passed",
      )
      expect(row.querySelector(".session-check-summary")).toHaveTextContent(
        documentation.summary,
      )
      // The advice never sits in the row. It waits in the tooltip.
      for (const advice of documentation.guidance) {
        expect(screen.queryByText(advice)).toBeNull()
      }

      fireEvent.focus(row)
      expect(screen.getAllByText(documentation.summary).length).toBeGreaterThan(1)
      for (const advice of documentation.guidance) {
        expect(screen.getAllByText(advice).length).toBeGreaterThan(0)
      }
      fireEvent.blur(row)
      for (const advice of documentation.guidance) {
        expect(screen.queryByText(advice)).toBeNull()
      }
    }

    // A finding names the evidence that caused it before its advice.
    const finding = screen.getByRole("group", { name: "Session overdepth" })
    fireEvent.focus(finding)
    expect(screen.getAllByText(/475,000 tokens/).length).toBeGreaterThan(0)
    for (const evidence of screen.getAllByText(/475,000 tokens/)) {
      expect(evidence).toHaveClass("text-burn-check-failure-text")
    }
    fireEvent.blur(finding)
  })

  it("omits unavailable checks from the summary and detail rows", () => {
    view()

    const summary = screen.getByRole("button", {
      name: "4/5 passed",
    })
    expect(summary.getAttribute("aria-expanded")).toBe("false")
    expect(screen.getByLabelText("Session hygiene checks").children).toHaveLength(2)
    expect(screen.getByText("Session overdepth")).toBeTruthy()
    expect(screen.getByText("Failed")).toBeTruthy()
    expect(screen.queryByText("Model overthinking")).toBeNull()

    fireEvent.click(summary)

    expect(summary.getAttribute("aria-expanded")).toBe("true")
    expect(screen.getByText("Model overthinking")).toBeTruthy()
    expect(screen.queryByText("Overpowered subagents")).toBeNull()
    expect(screen.getByText("Obsolete model")).toBeTruthy()
    expect(screen.getByText("Fast mode overuse")).toBeTruthy()
    expect(screen.getByText("Excess cache rehydration")).toBeTruthy()
    expect(screen.getAllByText("Passed")).toHaveLength(4)
    expect(screen.queryByText(/not assessed/i)).toBeNull()
  })

  it("opens the documentation for one check at a time", () => {
    view()

    fireEvent.click(screen.getByRole("button", { name: "Session overdepth details" }))
    expect(
      screen.getByRole("region", { name: "Session overdepth guidance" }),
    ).toHaveTextContent("Deep context burns more tokens with each request")
    expect(
      screen.getByRole("region", { name: "Session overdepth guidance" }),
    ).toHaveTextContent("475,000 tokens")
    expect(
      screen.getByRole("region", { name: "Session overdepth guidance" }),
    ).toHaveTextContent("reviewed limit is 400,000")
    const guidanceRegion = screen.getByRole("region", {
      name: "Session overdepth guidance",
    })
    expect(guidanceRegion).toHaveClass("-mx-1")
    expect(guidanceRegion).not.toHaveClass("px-1")
    expect(guidanceRegion.firstElementChild).toHaveClass(
      "mt-1",
      "border",
      "border-separator",
      "p-3",
    )
    expect(guidanceRegion.firstElementChild).not.toHaveClass("px-3", "pb-3")
    expect(guidanceRegion.querySelector('[class*="text-sm"]')).toBeNull()
    expect(screen.getByText(/475,000 tokens/).parentElement).toHaveClass(
      "text-burn-check-failure-text",
    )

    fireEvent.click(screen.getByRole("button", { name: "4/5 passed" }))
    fireEvent.click(screen.getByRole("button", { name: "Model overthinking details" }))
    expect(screen.queryByRole("region", { name: "Session overdepth guidance" })).toBeNull()
    expect(
      screen.getByRole("region", { name: "Model overthinking guidance" }),
    ).toHaveTextContent("Keep thinking/reasoning/effort below xhigh")
  })

  it("does not expose unavailable check details", () => {
    view()

    fireEvent.click(screen.getByRole("button", { name: "4/5 passed" }))
    expect(screen.queryByRole("button", { name: "Overpowered subagents details" })).toBeNull()
    expect(screen.queryByText("couldn't read the whole session log")).toBeNull()
  })

  it("keeps failing checks visible when the rollup closes", () => {
    view()

    const summary = screen.getByRole("button", { name: "4/5 passed" })
    fireEvent.click(summary)
    fireEvent.click(screen.getByRole("button", { name: "Model overthinking details" }))
    fireEvent.click(summary)

    expect(screen.queryByRole("region", { name: "Model overthinking guidance" })).toBeNull()
    expect(screen.getByRole("button", { name: "Session overdepth details" })).toBeTruthy()
  })

  it("does not overclaim when every available check passes", () => {
    const payload: SessionHygienePayload = {
      ...PAYLOAD,
      badges: PAYLOAD.badges.map((badge) =>
        badge.status === "finding"
          ? { id: badge.id, status: "clean", notAssessedReason: null }
          : badge,
      ),
    }
    render(<HygieneBreakdown checks={sessionHygieneChecks(payload)} />)

    expect(screen.getByRole("button", { name: "All assessed checks passed" })).toBeTruthy()
    expect(screen.queryByRole("button", { name: "5/5 passed" })).toBeNull()
  })

  it("renders no result when no checks were assessed", () => {
    const { container } = render(
      <HygieneBreakdown checks={sessionHygieneChecks(INITIAL_SESSION_HYGIENE)} />,
    )
    expect(container).toBeEmptyDOMElement()
  })
})
