import { expect, test } from "@playwright/test"

for (const fixture of ["unknown-only", "loading", "unavailable"]) {
  test(`usage search focuses its group when usage is ${fixture}`, async ({ page }) => {
    const params = new URLSearchParams({ surface: "settings" })
    if (fixture === "unknown-only") params.set("checkUsage", fixture)
    else params.set("discovery", fixture)
    await page.goto(`/tests/visual/?${params}`)
    await expect(page.getByRole("heading", { name: "General", exact: true })).toBeVisible()
    await page.getByRole("tab", { name: "Checks", exact: true }).click()
    const pane = page.getByRole("tabpanel", { name: "Checks", exact: true })
    await expect(
      pane.getByRole("button", { name: "Add connection", exact: true }),
    ).toBeEnabled()
    const toggle = pane.getByRole("switch", { name: "Run checks", exact: true })
    const enabledBeforeSearch = await toggle.isChecked()
    await page.evaluate(() =>
      window.__ANTIBURN_VISUAL_EMIT__?.("settings:pane", "checks#checkModelUsage"),
    )
    const group = pane.locator("section[data-settings-focus-self]")
    await expect(group).toBeFocused()
    await expect(group.getByRole("heading", { name: "Model-based checks" })).toBeVisible()
    await expect(pane.getByRole("button", { name: "Model usage", exact: true })).toHaveCount(0)
    await expect(pane.getByText(/unknown usage|cost estimate unavailable/i)).toHaveCount(0)
    const calls = await page.evaluate(() => window.__ANTIBURN_VISUAL_PROVIDER_CALLS__ ?? [])
    expect(calls.length).toBeGreaterThan(0)
    expect(calls.every((command) => command === "get_system_one_settings")).toBe(true)
    expect(await page.evaluate(() => window.__ANTIBURN_VISUAL_DIRECTORY_OPENS__ ?? [])).toEqual(
      [],
    )
    expect(await toggle.isChecked()).toBe(enabledBeforeSearch)
  })
}
