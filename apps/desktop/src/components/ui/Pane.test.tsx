// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { render, screen } from "@testing-library/react"
import { describe, expect, it } from "vitest"

import { Pane, PaneHeader } from "./Pane"

describe("PaneHeader", () => {
  it("renders the title as the pane’s only level-1 heading, with leading and trailing slots", () => {
    render(
      <PaneHeader
        title="Widgets"
        leading={<button type="button">Back</button>}
        trailing={<button type="button">Refresh</button>}
      />,
    )

    const heading = screen.getByRole("heading", { level: 1, name: "Widgets" })
    expect(heading.className).toContain("type-title-2")
    // Both slots live in the header row alongside the title, which is what lets
    // a caller identify the back control by walking up from the heading.
    expect(heading.parentElement?.contains(screen.getByRole("button", { name: "Back" }))).toBe(
      true,
    )
    expect(
      heading.parentElement?.contains(screen.getByRole("button", { name: "Refresh" })),
    ).toBe(true)
  })

  it("omits the optional slots when they are not supplied", () => {
    render(<PaneHeader title="Bare" />)

    expect(screen.queryByRole("button")).toBeNull()
  })
})

describe("Pane", () => {
  it("stacks its children under the header at the shared group spacing", () => {
    render(
      <Pane title="Widgets">
        <section>first</section>
        <section>second</section>
      </Pane>,
    )

    const stack = screen.getByText("first").parentElement!
    // One spacing definition for every pane: 24px (--space-2xl) between groups.
    expect(stack.className).toContain("space-y-6")
    expect(stack.children).toHaveLength(2)
    expect(screen.getByRole("heading", { level: 1, name: "Widgets" })).toBeTruthy()
  })

  it("keeps fragment children as direct stack children so the spacing applies", () => {
    render(
      <Pane title="Widgets">
        <>
          <section>first</section>
          <section>second</section>
        </>
      </Pane>,
    )

    expect(screen.getByText("first").parentElement?.children).toHaveLength(2)
  })
})
