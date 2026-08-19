// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { render, screen } from "@testing-library/react"
import { describe, expect, it } from "vitest"

import { Card } from "./Card"
import { Row } from "./Row"

describe("Card", () => {
  it("renders its children inside one bordered, hairline-divided container", () => {
    const { container } = render(
      <Card>
        <Row label="First" />
        <Row label="Second" />
      </Card>,
    )

    const card = container.firstElementChild!
    // The divider is the card's job, not each row's: rows stay edge-to-edge
    // and the container draws the hairline between them.
    expect(card.className).toContain("divide-y")
    expect(card.className).toContain("divide-separator")
    expect(card.className).toContain("border-separator")
    expect(card.className).toContain("rounded-popover")
    // Clipped, so the first and last rows take the container's corner radius.
    expect(card.className).toContain("overflow-hidden")
    expect(card.children).toHaveLength(2)
    expect(screen.getByText("First")).toBeInTheDocument()
    expect(screen.getByText("Second")).toBeInTheDocument()
  })

  it("appends a caller class without dropping its own", () => {
    const { container } = render(
      <Card className="mt-4">
        <Row label="Only" />
      </Card>,
    )

    const card = container.firstElementChild!
    expect(card.className).toContain("mt-4")
    expect(card.className).toContain("rounded-popover")
  })

  it("adds no trailing whitespace when no class is passed", () => {
    const { container } = render(
      <Card>
        <Row label="Only" />
      </Card>,
    )

    const className = container.firstElementChild!.className
    expect(className).toBe(className.trimEnd())
  })
})
