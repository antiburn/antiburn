// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it } from "vitest"

import { SessionCostBadge } from "./SessionCostBadge"

afterEach(cleanup)

describe("SessionCostBadge", () => {
  it("renders the figure and names it accessibly", () => {
    render(<SessionCostBadge totalUsd={2.4} figureLabel="Estimated cost" />)
    expect(screen.getByLabelText("Estimated cost $2.40")).toBeTruthy()
  })

  it("carries the outlier meaning in the accessible name, not just in color", () => {
    render(<SessionCostBadge totalUsd={19.5} figureLabel="Estimated cost" isHighCost />)
    const pill = screen.getByLabelText("Estimated cost $19.50, higher than usual")
    expect(pill.className).toContain("bg-brand-tint")
  })

  it("uses the calm palette when the session is not an outlier", () => {
    render(<SessionCostBadge totalUsd={0.4} figureLabel="Projected cost" />)
    expect(screen.getByLabelText("Projected cost $0.40").className).toContain(
      "bg-label-tertiary/15",
    )
  })

  it("shows the component rows and models in the tooltip", () => {
    render(
      <SessionCostBadge
        totalUsd={2.4}
        figureLabel="Estimated cost"
        models={["model-a", "model-b"]}
        breakdownRows={[
          { label: "Input", usd: 0.3 },
          { label: "Output", usd: 0.8 },
        ]}
      />,
    )
    fireEvent.focus(screen.getByLabelText("Estimated cost $2.40"))
    expect(screen.getAllByText("Input").length).toBeGreaterThan(0)
    expect(screen.getAllByText("$0.30").length).toBeGreaterThan(0)
    expect(screen.getAllByText("model-a").length).toBeGreaterThan(0)
    expect(screen.getAllByText("model-b").length).toBeGreaterThan(0)
  })

  it("explains the outlier band in the tooltip", () => {
    render(<SessionCostBadge totalUsd={19.5} figureLabel="Estimated cost" isHighCost />)
    fireEvent.focus(screen.getByLabelText(/higher than usual/))
    expect(screen.getAllByText(/over 3× your typical session/).length).toBeGreaterThan(0)
  })

  it("appends caller classes to the pill geometry", () => {
    render(
      <SessionCostBadge
        totalUsd={1}
        figureLabel="Estimated cost"
        className="relative top-px"
      />,
    )
    const pill = screen.getByLabelText("Estimated cost $1.00")
    expect(pill.className).toContain("relative top-px")
  })
})
