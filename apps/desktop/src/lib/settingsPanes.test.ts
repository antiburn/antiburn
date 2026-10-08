import { describe, expect, it } from "vitest"

import { isSettingsPane } from "./settingsPanes"

describe("isSettingsPane", () => {
  it("recognizes every registered pane", () => {
    for (const pane of [
      "general",
      "notifications",
      "usage",
      "appearance",
      "privacy",
      "about",
    ]) {
      expect(isSettingsPane(pane)).toBe(true)
    }
  })

  it("rejects unknown values and non-strings", () => {
    expect(isSettingsPane("insights")).toBe(false)
    expect(isSettingsPane("reports")).toBe(false)
    expect(isSettingsPane(undefined)).toBe(false)
    expect(isSettingsPane(7)).toBe(false)
  })

  it("rejects the Sources and Checks panes, retired to Overview step modals", () => {
    expect(isSettingsPane("sources")).toBe(false)
    expect(isSettingsPane("checks")).toBe(false)
  })
})
