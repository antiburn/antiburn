import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it } from "vitest"

import type { SessionHygienePayload } from "../../../lib/insightsIpc"
import {
  INITIAL_SESSION_HYGIENE,
  sessionHygieneChecks,
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
  it("omits unavailable checks from the summary and detail rows", () => {
    view()

    const summary = screen.getByRole("button", {
      name: "4/5 passed",
    })
    expect(summary.getAttribute("aria-expanded")).toBe("false")
    expect(screen.getByLabelText("Session hygiene checks").children).toHaveLength(2)
    expect(screen.getByText("Session overdepth")).toBeTruthy()
    expect(screen.getByText("failing")).toBeTruthy()
    expect(screen.queryByText("Model overthinking")).toBeNull()

    fireEvent.click(summary)

    expect(summary.getAttribute("aria-expanded")).toBe("true")
    expect(screen.getByText("Model overthinking")).toBeTruthy()
    expect(screen.queryByText("Overpowered subagents")).toBeNull()
    expect(screen.getByText("Obsolete model")).toBeTruthy()
    expect(screen.getByText("Fast mode overuse")).toBeTruthy()
    expect(screen.getByText("Excess cache rehydration")).toBeTruthy()
    expect(screen.getAllByText("passed")).toHaveLength(4)
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
