import { afterEach, describe, expect, it, vi } from "vitest"
import { SettingsTargetFocus } from "./SettingsTargetFocus"
afterEach(() => document.body.replaceChildren())
describe("settings search focus", () => {
  it("focuses an explicit group target without choosing one of its controls", () => {
    const panel = document.createElement("div")
    const group = document.createElement("section")
    group.dataset.settingsControl = "checkModelUsage"
    group.dataset.settingsFocusSelf = "true"
    group.tabIndex = -1
    group.scrollIntoView = vi.fn()
    const button = document.createElement("button")
    group.append(button)
    panel.append(group)
    document.body.append(panel)
    new SettingsTargetFocus().attach(panel, "checks", "checkModelUsage", 1)()
    expect(group).toHaveFocus()
    expect(button).not.toHaveFocus()
  })
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
  it("waits for busy and disabled changes, then focuses the real input once", async () => {
    const panel = document.createElement("div")
    const region = document.createElement("div")
    region.setAttribute("aria-busy", "true")
    const row = document.createElement("div")
    row.dataset.settingsControl = "typeSafeApiKey"
    row.tabIndex = -1
    row.scrollIntoView = vi.fn()
    const input = document.createElement("input")
    input.disabled = true
    row.append(input)
    region.append(row)
    panel.append(region)
    document.body.append(panel)
    const focus = vi.spyOn(input, "focus")
    const stop = new SettingsTargetFocus().attach(panel, "checks", "typeSafeApiKey", 1)
    expect(row).not.toHaveFocus()
    input.disabled = false
    await Promise.resolve()
    expect(focus).not.toHaveBeenCalled()
    region.setAttribute("aria-busy", "false")
    await Promise.resolve()
    expect(input).toHaveFocus()
    input.disabled = true
    input.disabled = false
    await Promise.resolve()
    expect(focus).toHaveBeenCalledOnce()
    stop()
  })
  it("disconnects an unresolved busy target on teardown", async () => {
    const panel = document.createElement("div")
    panel.setAttribute("aria-busy", "true")
    panel.innerHTML =
      '<div data-settings-control="typeSafeApiKey" tabindex="-1"><input disabled /></div>'
    document.body.append(panel)
    const input = panel.querySelector("input")!
    const focus = vi.spyOn(input, "focus")
    new SettingsTargetFocus().attach(panel, "checks", "typeSafeApiKey", 1)()
    input.disabled = false
    panel.setAttribute("aria-busy", "false")
    await Promise.resolve()
    expect(focus).not.toHaveBeenCalled()
  })
  it("focuses temporary recovery once and keeps the real field pending through Retry", async () => {
    const panel = document.createElement("div")
    const recovery = document.createElement("div")
    recovery.dataset.settingsControl = "customResponseMode"
    recovery.dataset.settingsTargetPending = "true"
    recovery.scrollIntoView = vi.fn()
    const retry = document.createElement("button")
    recovery.append(retry)
    panel.append(recovery)
    document.body.append(panel)
    const focus = new SettingsTargetFocus()
    const stop = focus.attach(panel, "checks", "customResponseMode", 1)
    expect(retry).toHaveFocus()
    const other = document.createElement("button")
    panel.append(other)
    other.focus()
    focus.attach(panel, "checks", "customResponseMode", 1)()
    expect(other).toHaveFocus()
    panel.setAttribute("aria-busy", "true")
    const row = document.createElement("div")
    row.dataset.settingsControl = "customResponseMode"
    row.scrollIntoView = vi.fn()
    const select = document.createElement("select")
    row.append(select)
    recovery.replaceWith(row)
    await Promise.resolve()
    expect(select).not.toHaveFocus()
    panel.setAttribute("aria-busy", "false")
    await Promise.resolve()
    expect(select).toHaveFocus()
    other.focus()
    focus.attach(panel, "checks", "customResponseMode", 1)()
    expect(other).toHaveFocus()
    stop()
  })
  it("waits for the modal to close and applies only the latest search request", async () => {
    const panel = document.createElement("div")
    panel.innerHTML = `
      <div data-settings-control="checkDirectories"><button>Check files</button></div>
      <div data-settings-control="checkModelUsage"><button>Model usage</button></div>
      <dialog open><input aria-label="Credential" /></dialog>
    `
    document.body.append(panel)
    const files = panel.querySelector<HTMLButtonElement>("button")!
    const usage = panel.querySelectorAll<HTMLButtonElement>("button")[1]!
    for (const row of panel.querySelectorAll<HTMLElement>("[data-settings-control]"))
      row.scrollIntoView = vi.fn()
    const dialog = panel.querySelector("dialog")!
    const credential = panel.querySelector("input")!
    credential.focus()
    const focus = new SettingsTargetFocus()
    focus.attach(panel, "checks", "checkDirectories", 1)
    const stop = focus.attach(panel, "checks", "checkModelUsage", 2)
    expect(credential).toHaveFocus()
    expect(files).not.toHaveFocus()
    dialog.open = false
    await Promise.resolve()
    expect(usage).toHaveFocus()
    files.focus()
    focus.attach(panel, "checks", "checkModelUsage", 2)()
    expect(files).toHaveFocus()
    stop()
  })
  it.each(["close", "remove"])(
    "keeps the latest request after stale cleanup and a sibling modal %s",
    async (action) => {
      const panel = document.createElement("div")
      panel.innerHTML = `
        <div data-settings-control="checkDirectories"><button>Check files</button></div>
        <div data-settings-control="checkModelUsage"><button>Model usage</button></div>
      `
      const dialog = document.createElement("dialog")
      dialog.open = true
      const close = document.createElement("button")
      dialog.append(close)
      document.body.append(panel, dialog)
      for (const row of panel.querySelectorAll<HTMLElement>("[data-settings-control]"))
        row.scrollIntoView = vi.fn()
      close.focus()
      const focus = new SettingsTargetFocus()
      const staleCleanup = focus.attach(panel, "checks", "checkDirectories", 1)
      const stop = focus.attach(panel, "checks", "checkModelUsage", 2)
      staleCleanup()
      expect(close).toHaveFocus()
      if (action === "close") dialog.open = false
      else dialog.remove()
      await Promise.resolve()
      const usage = panel.querySelectorAll<HTMLButtonElement>("button")[1]!
      expect(usage).toHaveFocus()
      const files = panel.querySelector<HTMLButtonElement>("button")!
      files.focus()
      panel.setAttribute("aria-busy", "false")
      await Promise.resolve()
      expect(files).toHaveFocus()
      stop()
    },
  )
  it("does not consume a request when the target cannot receive focus", async () => {
    const panel = document.createElement("div")
    panel.innerHTML =
      '<div data-settings-control="checkDirectories"><button>Files</button></div>'
    document.body.append(panel)
    const row = panel.firstElementChild as HTMLElement
    row.scrollIntoView = vi.fn()
    const button = panel.querySelector("button")!
    const blockedFocus = vi.spyOn(button, "focus").mockImplementationOnce(() => undefined)
    const stop = new SettingsTargetFocus().attach(panel, "checks", "checkDirectories", 1)
    expect(button).not.toHaveFocus()
    button.setAttribute("aria-disabled", "false")
    await Promise.resolve()
    expect(button).toHaveFocus()
    expect(blockedFocus).toHaveBeenCalledTimes(2)
    stop()
  })
  it("keeps a later user focus choice after the modal closes before the observer runs", async () => {
    const panel = document.createElement("div")
    panel.innerHTML = `
      <div data-settings-control="checkDirectories"><button>Files</button></div>
      <button id="later">Later choice</button>
      <dialog open><input /></dialog>
    `
    document.body.append(panel)
    const row = panel.firstElementChild as HTMLElement
    row.scrollIntoView = vi.fn()
    const later = panel.querySelector<HTMLButtonElement>("#later")!
    const focus = new SettingsTargetFocus()
    focus.attach(panel, "checks", "checkDirectories", 1)
    panel.querySelector("dialog")!.open = false
    later.dispatchEvent(new Event("pointerdown", { bubbles: true }))
    later.focus()
    await Promise.resolve()
    expect(later).toHaveFocus()
    focus.attach(panel, "checks", "checkDirectories", 1)()
    expect(later).toHaveFocus()
  })
})
