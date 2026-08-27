import { describe, expect, it } from "vitest"

import { isSettingsPane } from "./settingsPanes"

describe("isSettingsPane", () => {
  it("recognizes every registered pane, including insights", () => {
    for (const pane of [
      "general",
      "appearance",
      "sources",
      "privacy",
      "notifications",
      "usage",
      "insights",
      "about",
    ]) {
      expect(isSettingsPane(pane)).toBe(true)
    }
  })

  it("rejects unknown values and non-strings", () => {
    expect(isSettingsPane("reports")).toBe(false)
    expect(isSettingsPane(undefined)).toBe(false)
    expect(isSettingsPane(7)).toBe(false)
  })
})
