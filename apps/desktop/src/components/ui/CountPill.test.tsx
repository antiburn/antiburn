import { render, screen } from "@testing-library/react"
import { describe, expect, it } from "vitest"

import { CountPill } from "./CountPill"

describe("CountPill", () => {
  it("renders the count with tabular mono styling", () => {
    render(<CountPill count={12} />)

    const pill = screen.getByText("12")
    expect(pill).toHaveClass(
      "h-4",
      "min-w-4",
      "rounded-full",
      "bg-surface-tertiary/40",
      "font-mono",
      "type-metadata",
      "font-medium!",
      "tabular-nums",
      "text-label-tertiary",
    )
  })

  it("renders zero", () => {
    render(<CountPill count={0} />)

    expect(screen.getByText("0")).toBeInTheDocument()
  })

  it("merges an extra className", () => {
    render(<CountPill count={3} className="ml-auto" />)

    expect(screen.getByText("3")).toHaveClass("ml-auto")
  })
})
