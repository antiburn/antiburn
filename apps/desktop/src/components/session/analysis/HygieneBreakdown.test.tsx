import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it } from "vitest"

import type { SessionHygienePayload } from "../../../lib/insightsIpc"
import { sessionHygieneChecks } from "../../../lib/presentation/sessionHygiene"
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
  it("rolls passing and unassessed checks into a summary", () => {
    view()

    const summary = screen.getByRole("button", {
      name: "4/5 passing, 1 not assessed",
    })
    expect(summary.getAttribute("aria-expanded")).toBe("false")
    expect(screen.getByLabelText("Session hygiene checks").children).toHaveLength(2)
    expect(screen.getByText("Session overdepth")).toBeTruthy()
    expect(screen.getByText("failing")).toBeTruthy()
    expect(screen.queryByText("Model overthinking")).toBeNull()

    fireEvent.click(summary)

    expect(summary.getAttribute("aria-expanded")).toBe("true")
    expect(screen.getByText("Model overthinking")).toBeTruthy()
    expect(screen.getByText("Overpowered subagents")).toBeTruthy()
    expect(screen.getByText("Obsolete model")).toBeTruthy()
    expect(screen.getByText("Fast mode overuse")).toBeTruthy()
    expect(screen.getByText("Excess cache rehydration")).toBeTruthy()
    expect(screen.getAllByText("passing")).toHaveLength(4)
    expect(screen.getByText("not assessed")).toBeTruthy()
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

    fireEvent.click(screen.getByRole("button", { name: "4/5 passing, 1 not assessed" }))
    fireEvent.click(screen.getByRole("button", { name: "Model overthinking details" }))
    expect(screen.queryByRole("region", { name: "Session overdepth guidance" })).toBeNull()
    expect(
      screen.getByRole("region", { name: "Model overthinking guidance" }),
    ).toHaveTextContent("Keep thinking/reasoning/effort below xhigh")
  })

  it("explains why a check was not assessed", () => {
    view()

    fireEvent.click(screen.getByRole("button", { name: "4/5 passing, 1 not assessed" }))
    fireEvent.click(screen.getByRole("button", { name: "Overpowered subagents details" }))
    expect(
      screen.getByRole("region", { name: "Overpowered subagents guidance" }),
    ).toHaveTextContent("couldn't read the whole session log")
  })

  it("keeps failing checks visible when the rollup closes", () => {
    view()

    const summary = screen.getByRole("button", { name: "4/5 passing, 1 not assessed" })
    fireEvent.click(summary)
    fireEvent.click(screen.getByRole("button", { name: "Model overthinking details" }))
    fireEvent.click(summary)

    expect(screen.queryByRole("region", { name: "Model overthinking guidance" })).toBeNull()
    expect(screen.getByRole("button", { name: "Session overdepth details" })).toBeTruthy()
  })
})
