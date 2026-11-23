import { expect, test } from "@playwright/test"

for (const theme of ["light", "dark"] as const) {
  test(`Sessions has no SSH source controls in ${theme}`, async ({ page }) => {
    await page.goto(`/tests/visual/?surface=main&onboarding=complete&theme=${theme}`)
    await page.getByRole("button", { name: "Filters", exact: true }).click()
    await expect(page.getByText("Source · Select one or more")).toHaveCount(0)
    await expect(
      page.getByRole("menuitemcheckbox", { name: /^Remote,|^Local,|^All sources,/ }),
    ).toHaveCount(0)
    await expect(page.getByRole("img", { name: /Remote session from/ })).toHaveCount(0)
  })

  test(`Settings has no host management in ${theme}`, async ({ page }) => {
    await page.goto(`/tests/visual/?surface=settings&theme=${theme}`)
    await page.getByRole("tab", { name: "Sessions", exact: true }).click()
    await expect(page.getByRole("heading", { name: "Remote hosts" })).toHaveCount(0)
    await expect(page.getByRole("button", { name: "Add host" })).toHaveCount(0)
    await expect(page.getByRole("combobox", { name: "Sync frequency" })).toHaveCount(0)
  })
}
