// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { cleanup, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it } from "vitest"

import type { MockSessionHygieneCheck } from "../../lib/presentation/mockSessionHygiene"
import { SessionHygieneBadges } from "./SessionHygieneBadges"

const CHECKS: MockSessionHygieneCheck[] = [
  { id: "sessionOverdepth", passed: false, title: "Session overdepth detected" },
  { id: "modelOverthinking", passed: true, title: "No model overthinking detected" },
  { id: "excessCacheRehydration", passed: true, title: "No excess cache rehydration detected" },
]

afterEach(cleanup)

describe("SessionHygieneBadges", () => {
  it("shows failed checks in brand orange, outside the fan container", () => {
    render(<SessionHygieneBadges checks={CHECKS} />)
    const failed = screen.getByLabelText("Session overdepth detected")
    expect(failed.className).toContain("text-brand-tint")
    expect(failed.closest(".session-hygiene-pass")).toBeNull()
  })

  it("shows passed checks in tertiary, inside the fan container", () => {
    render(<SessionHygieneBadges checks={CHECKS} />)
    for (const title of [
      "No model overthinking detected",
      "No excess cache rehydration detected",
    ]) {
      const glyph = screen.getByLabelText(title)
      expect(glyph.className).toContain("text-label-tertiary")
      expect(glyph.closest(".session-hygiene-pass")).not.toBeNull()
    }
  })

  it("renders no fan container when every check fails", () => {
    const failing = CHECKS.map((check) => ({ ...check, passed: false }))
    const { container } = render(<SessionHygieneBadges checks={failing} />)
    expect(container.querySelector(".session-hygiene-pass")).toBeNull()
  })
})
