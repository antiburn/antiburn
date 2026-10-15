import { expect, test } from "@playwright/test"

for (const theme of ["light", "dark"] as const) {
  test(`sidebar summaries and Agents page in ${theme}`, async ({ page }, testInfo) => {
    await page.setViewportSize({ width: 1100, height: 800 })
    await page.emulateMedia({ reducedMotion: "reduce", colorScheme: theme })
    await page.goto(`/tests/visual/?surface=main&onboarding=complete&theme=${theme}`)
    const nav = page.getByRole("tablist", { name: "Main sections" })
    await expect(nav.getByRole("tab")).toHaveCount(6, { timeout: 30_000 })
    await expect(nav.getByRole("tab", { name: "Limits", exact: true })).toContainText("%")
    await expect(nav.getByRole("tab", { name: "Memories", exact: true })).toContainText("12")
    await nav.getByRole("tab", { name: "Agents", exact: true }).click()
    await expect(page.getByRole("heading", { name: "Agents", exact: true })).toBeVisible()
    await expect(page.getByRole("button", { name: "Agent settings" })).toBeVisible()
    await page.screenshot({ path: testInfo.outputPath("agents.png") })
    const link = page.getByRole("button", { name: /^View .+ sessions$/ }).first()
    await expect(link).toBeVisible()
    await link.click()
    await expect(nav.getByRole("tab", { name: "Sessions", exact: true })).toHaveAttribute(
      "aria-selected",
      "true",
    )
  })
}

test("onboarding docks into Agents and Back restores the card", async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 1100, height: 800 })
  await page.emulateMedia({ reducedMotion: "no-preference" })
  await page.goto("/tests/visual/?surface=main")
  await expect(page.getByRole("button", { name: "Settings", exact: true })).toBeDisabled()
  await expect(page.getByRole("tab", { name: "Memories", exact: true })).toHaveText("Memories")
  await page.getByRole("button", { name: "Get Started" }).click()
  await expect(
    page.locator('[style*="view-transition-name: progress-step-agents"]'),
  ).toBeVisible()
  await page.getByRole("button", { name: "Next", exact: true }).click()
  await expect(page.getByRole("heading", { name: "Plan limits" })).toBeVisible()
  await expect(page.getByRole("tab", { name: "Agents", exact: true })).toHaveCSS(
    "view-transition-name",
    "progress-step-agents",
  )
  await page.screenshot({ path: testInfo.outputPath("limits-step.png") })
  await page.locator("fieldset").getByRole("button", { name: "Back", exact: true }).click()
  await expect(page.getByRole("button", { name: "Next", exact: true })).toBeVisible()
  await expect(page.getByRole("tab", { name: "Agents", exact: true })).toHaveCSS(
    "view-transition-name",
    "none",
  )
})
