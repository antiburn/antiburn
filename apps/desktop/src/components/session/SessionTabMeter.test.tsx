import { cleanup, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it } from "vitest"

import { SessionTabMeter } from "./SessionTabMeter"

afterEach(cleanup)

describe("SessionTabMeter", () => {
  it("states the label, the figure, and the reading", () => {
    render(
      <SessionTabMeter
        meter={{
          label: "Peak context",
          figure: "45%",
          percent: 45,
          meterLabel: "Peak context used of the window",
        }}
      />,
    )
    expect(screen.getByText("Peak context")).toBeTruthy()
    expect(screen.getByText("45%")).toBeTruthy()
    expect(screen.getByText("Peak context used of the window: 45 percent")).toBeTruthy()
  })

  it("says there is no figure rather than reading zero", () => {
    render(
      <SessionTabMeter
        meter={{
          label: "Cost",
          figure: "—",
          percent: null,
          meterLabel: "Share of the spend that was rewrite",
        }}
      />,
    )
    expect(
      screen.getByText("Share of the spend that was rewrite: no stated figure"),
    ).toBeTruthy()
  })
})
