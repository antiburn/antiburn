// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { beforeEach, describe, expect, it } from "vitest"

import { applyTheme, currentTheme } from "./appearance"

describe("applyTheme", () => {
  beforeEach(() => {
    delete document.documentElement.dataset["theme"]
  })

  it("writes an explicit override for light and dark", () => {
    applyTheme("dark")
    expect(document.documentElement.dataset["theme"]).toBe("dark")
    applyTheme("light")
    expect(document.documentElement.dataset["theme"]).toBe("light")
  })

  it('removes the attribute for "system" rather than writing the word', () => {
    // The token layer keys off the *presence* of `data-theme`; a literal
    // `data-theme="system"` matches no rule and would strand the app in the
    // light palette.
    applyTheme("dark")
    applyTheme("system")
    expect(document.documentElement.hasAttribute("data-theme")).toBe(false)
  })

  it('reads back what it wrote, and calls an unknown value "system"', () => {
    applyTheme("light")
    expect(currentTheme()).toBe("light")

    applyTheme("system")
    expect(currentTheme()).toBe("system")

    document.documentElement.dataset["theme"] = "sepia"
    expect(currentTheme()).toBe("system")
  })
})
