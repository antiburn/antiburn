// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { render, screen } from "@testing-library/react"
import { describe, expect, it } from "vitest"

import { Skeleton, SkeletonCard } from "./Skeleton"

describe("Skeleton", () => {
  it("renders children untouched once loading is over", () => {
    const { container } = render(
      <Skeleton loading={false}>
        <button type="button">Rescan</button>
      </Skeleton>,
    )

    const button = screen.getByRole("button", { name: "Rescan" })
    expect(button).toBeInTheDocument()
    // No wrapper at all: the placeholder must not survive as an extra node
    // that could change layout after the real content arrives.
    expect(container.firstElementChild).toBe(button)
  })

  it("hides the content it wraps from assistive tech and from interaction", () => {
    const { container } = render(
      <Skeleton>
        <button type="button">Rescan</button>
      </Skeleton>,
    )

    const placeholder = container.firstElementChild!
    expect(placeholder.getAttribute("aria-hidden")).toBe("true")
    expect(placeholder.getAttribute("tabindex")).toBe("-1")
    expect(placeholder.className).toContain("pointer-events-none")
    expect(placeholder.className).toContain("select-none")
    // Nothing in the subtree is announced, so the button is no longer a button
    // as far as the accessibility tree is concerned.
    expect(screen.queryByRole("button", { name: "Rescan" })).toBeNull()
  })

  it("keeps the wrapped content in the DOM as a sizer, painted invisible", () => {
    const { container } = render(
      <Skeleton>
        <span>A fairly long label</span>
      </Skeleton>,
    )

    const placeholder = container.firstElementChild!
    // Present for its size…
    expect(placeholder.textContent).toBe("A fairly long label")
    // …but nothing inside it paints.
    expect(placeholder.className).toContain("inline-block")
    expect(placeholder.className).toContain("text-transparent")
    expect(placeholder.className).toContain("[&_*]:invisible")
  })

  it("renders a standalone block sized by the caller", () => {
    const { container } = render(<Skeleton className="h-3 w-32" />)

    const placeholder = container.firstElementChild!
    expect(placeholder.textContent).toBe("")
    expect(placeholder.className).toContain("block")
    expect(placeholder.className).not.toContain("inline-block")
    expect(placeholder.className).toContain("h-3")
    expect(placeholder.className).toContain("w-32")
  })

  it("pulses and carries the placeholder fill in both shapes", () => {
    const { container } = render(
      <>
        <Skeleton className="h-3 w-32" />
        <Skeleton>
          <span>Content</span>
        </Skeleton>
      </>,
    )

    for (const placeholder of Array.from(container.children)) {
      expect(placeholder.className).toContain("animate-pulse")
      expect(placeholder.className).toContain("bg-surface-tertiary")
    }
  })

  it("passes extra attributes through to the placeholder element", () => {
    const { container } = render(<Skeleton data-testid="pending" className="h-3" />)

    expect(container.firstElementChild?.getAttribute("data-testid")).toBe("pending")
  })
})

describe("SkeletonCard", () => {
  it("renders one placeholder line per requested width, in the card shell", () => {
    const { container } = render(<SkeletonCard lines={["w-24", "w-40", "w-16"]} />)

    const card = container.firstElementChild!
    expect(card.className).toContain("rounded-popover")
    expect(card.className).toContain("border-separator")
    expect(card.querySelectorAll("[data-placeholder]")).toHaveLength(3)
    expect(card.querySelector(".w-40")).not.toBeNull()
  })

  it("omits the leading square unless it is asked for", () => {
    const { container: without } = render(<SkeletonCard />)
    const { container: with_ } = render(<SkeletonCard leading />)

    // Two default lines, plus the leading square only in the second case.
    expect(without.querySelectorAll("[data-placeholder]")).toHaveLength(2)
    expect(with_.querySelectorAll("[data-placeholder]")).toHaveLength(3)
  })

  it("announces nothing: a run of placeholders is not a live region", () => {
    render(<SkeletonCard leading />)

    expect(screen.queryByRole("status")).toBeNull()
    expect(document.querySelectorAll("[aria-live]")).toHaveLength(0)
  })
})
