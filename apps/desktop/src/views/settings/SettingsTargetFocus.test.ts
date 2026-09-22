import { afterEach, describe, expect, it, vi } from "vitest"
import { SettingsTargetFocus } from "./SettingsTargetFocus"
afterEach(() => document.body.replaceChildren())
describe("settings search focus", () => {
  it.each(["disabled", "aria-disabled"])("focuses the row when a switch is %s", (attribute) => {
    const panel = document.createElement("div")
    const row = document.createElement("div")
    row.dataset.settingsControl = "notificationSound"
    row.tabIndex = -1
    row.scrollIntoView = vi.fn()
    const button = document.createElement("button")
    button.setAttribute("role", "switch")
    button.setAttribute(attribute, "true")
    row.append(button)
    panel.append(row)
    document.body.append(panel)
    new SettingsTargetFocus().attach(panel, "notifications", "notificationSound", 1)()
    expect(row).toHaveFocus()
  })
  it("finds a delayed target once and does not steal focus on later renders", async () => {
    const focus = new SettingsTargetFocus()
    const panel = document.createElement("div")
    document.body.append(panel)
    const stop = focus.attach(panel, "privacy", "analytics", 1)
    const row = document.createElement("div")
    row.dataset.settingsControl = "analytics"
    row.scrollIntoView = vi.fn()
    const button = document.createElement("button")
    row.append(button)
    panel.append(row)
    await Promise.resolve()
    expect(button).toHaveFocus()
    expect(row.scrollIntoView).toHaveBeenCalledOnce()
    const other = document.createElement("button")
    panel.append(other)
    other.focus()
    stop()
    focus.attach(panel, "privacy", "analytics", 1)()
    expect(other).toHaveFocus()
    focus.attach(panel, "privacy", "analytics", 2)()
    expect(button).toHaveFocus()
  })
})
