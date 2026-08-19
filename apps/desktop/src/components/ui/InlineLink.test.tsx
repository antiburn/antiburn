// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { fireEvent, render, screen } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

import { InlineLink } from "./InlineLink"

describe("InlineLink", () => {
  it("renders as a button, not an anchor", () => {
    render(<InlineLink onClick={() => {}}>Privacy Policy</InlineLink>)

    // A real href would navigate the desktop webview instead of opening the
    // user's browser, so this must stay a button.
    const link = screen.getByRole("button", { name: "Privacy Policy" })
    expect(link.tagName).toBe("BUTTON")
    expect(link.getAttribute("href")).toBeNull()
  })

  it("calls onClick when activated", () => {
    const onClick = vi.fn()
    render(<InlineLink onClick={onClick}>Terms of Service</InlineLink>)

    fireEvent.click(screen.getByRole("button", { name: "Terms of Service" }))
    expect(onClick).toHaveBeenCalledTimes(1)
  })

  it("sits inline within surrounding prose", () => {
    render(
      <p>
        See our <InlineLink onClick={() => {}}>Privacy Policy</InlineLink> for detail.
      </p>,
    )

    const paragraph = screen.getByText(/See our/)
    expect(paragraph.textContent).toBe("See our Privacy Policy for detail.")
  })
})
