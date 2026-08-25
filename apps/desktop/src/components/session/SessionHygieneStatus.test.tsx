// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { cleanup, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it } from "vitest"

import type { MockSessionHygieneCheck } from "../../lib/presentation/mockSessionHygiene"
import { SessionHygieneFailureLine, SessionHygieneStatus } from "./SessionHygieneStatus"

function check(
  id: MockSessionHygieneCheck["id"],
  shortTitle: string,
  passed: boolean,
): MockSessionHygieneCheck {
  return {
    id,
    passed,
    shortTitle,
    title: passed ? `No ${shortTitle.toLowerCase()} detected` : `${shortTitle} detected`,
  }
}

const ALL_PASSED: MockSessionHygieneCheck[] = [
  check("sessionOverdepth", "Session overdepth", true),
  check("modelOverthinking", "Model overthinking", true),
  check("excessCacheRehydration", "Excess cache rehydration", true),
]

const TWO_FAILED: MockSessionHygieneCheck[] = [
  check("sessionOverdepth", "Session overdepth", true),
  check("modelOverthinking", "Model overthinking", false),
  check("excessCacheRehydration", "Excess cache rehydration", false),
]

afterEach(cleanup)

describe("SessionHygieneStatus", () => {
  it("counts every check as passed when none failed", () => {
    render(<SessionHygieneStatus checks={ALL_PASSED} />)
    const status = screen.getByLabelText("3 of 3 checks passed")
    expect(status.className).toContain("text-system-green")
    expect(status.textContent).toBe("3/3 checks passed")
  })

  it("counts the failures, not the passes, when a check failed", () => {
    render(<SessionHygieneStatus checks={TWO_FAILED} />)
    const status = screen.getByLabelText(/^2 of 3 checks failed/)
    expect(status.className).toContain("text-system-red")
    expect(status.textContent).toBe("2/3checks failed")
  })

  it("names the failed checks in the accessible label", () => {
    render(<SessionHygieneStatus checks={TWO_FAILED} />)
    expect(
      screen.getByLabelText(
        "2 of 3 checks failed: Model overthinking, Excess cache rehydration",
      ),
    ).toBeTruthy()
  })

  it("hides the label after the check until the row is hovered", () => {
    const { container } = render(<SessionHygieneStatus checks={ALL_PASSED} />)
    // The reveal element carries the hover rule; a failing row has none.
    expect(container.querySelector(".session-hygiene-reveal")).not.toBeNull()
    cleanup()
    const failing = render(<SessionHygieneStatus checks={TWO_FAILED} />)
    expect(failing.container.querySelector(".session-hygiene-reveal")).toBeNull()
  })
})

describe("SessionHygieneFailureLine", () => {
  it("lists the failed check names, short form, in check order", () => {
    const { container } = render(<SessionHygieneFailureLine checks={TWO_FAILED} />)
    const line = container.querySelector(".session-hygiene-failures")
    expect(line?.textContent).toBe("Model overthinking, Excess cache rehydration")
  })

  it("renders nothing when every check passed", () => {
    const { container } = render(<SessionHygieneFailureLine checks={ALL_PASSED} />)
    expect(container.firstChild).toBeNull()
  })
})
