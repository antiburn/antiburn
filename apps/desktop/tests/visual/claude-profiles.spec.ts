import { expect, test, type Page } from "@playwright/test"

const FIXED_TIME = new Date("2026-09-15T00:00:00Z")

async function openSources(page: Page, theme: "light" | "dark") {
  await page.clock.setFixedTime(FIXED_TIME)
  await page.emulateMedia({ colorScheme: theme })
  await page.goto(`/tests/visual/?surface=settings&profiles=1&theme=${theme}`)
  await page.getByRole("tab", { name: "Sources", exact: true }).click()
  await expect(page.getByRole("heading", { name: "Claude profiles" })).toBeVisible()
}

function profilesSection(page: Page) {
  return page.locator("section").filter({
    has: page.getByRole("heading", { name: "Claude profiles" }),
  })
}

for (const theme of ["light", "dark"] as const) {
  test(`Claude profiles list, suggest, and mark the built-in profile in ${theme}`, async ({
    page,
  }, testInfo) => {
    await openSources(page, theme)
    const section = profilesSection(page)
    await expect(section.getByText("Default", { exact: true })).toBeVisible()
    await expect(section.getByText("Claude Work", { exact: true })).toBeVisible()
    await expect(section.getByText("Found on this computer")).toBeVisible()
    await expect(section.getByText("2 profiles")).toBeVisible()
    await section.screenshot({ path: testInfo.outputPath(`claude-profiles-${theme}.png`) })
  })
}

test("a suggested folder is added with its suggested name", async ({ page }) => {
  await openSources(page, "light")
  const section = profilesSection(page)
  await section
    .getByRole("button", { name: "Add /Users/fixture/.claude-side as a Claude profile" })
    .click()
  const dialog = page.getByRole("dialog", { name: "Add Claude profile" })
  await expect(dialog.getByRole("textbox", { name: "Name" })).toHaveValue("Claude Side")
  await dialog.getByRole("button", { name: "Add profile" }).click()
  await expect(dialog).toBeHidden()
  await expect(section.getByText("Claude Side", { exact: true })).toBeVisible()
  await expect(section.getByText("Found on this computer")).toBeHidden()
  await expect(section.getByText("3 profiles")).toBeVisible()
})

test("a duplicate name is refused and the dialog keeps the reader's input", async ({
  page,
}, testInfo) => {
  await openSources(page, "light")
  const section = profilesSection(page)
  await section.getByRole("button", { name: /Add profile/ }).click()
  const dialog = page.getByRole("dialog", { name: "Add Claude profile" })
  await dialog.getByRole("textbox", { name: "Name" }).fill("claude work")
  await dialog.getByRole("button", { name: /Choose/ }).click()
  await expect(dialog.getByText("/Users/fixture/.claude-extra")).toBeVisible()
  await dialog.getByRole("button", { name: "Add profile" }).click()
  await expect(dialog.getByRole("alert")).toHaveText("Another profile already uses this name.")
  await expect(dialog.getByRole("textbox", { name: "Name" })).toHaveValue("claude work")
  await dialog.screenshot({ path: testInfo.outputPath("claude-profile-duplicate.png") })
  await dialog.getByRole("textbox", { name: "Name" }).fill("Claude Extra")
  await dialog.getByRole("button", { name: "Add profile" }).click()
  await expect(dialog).toBeHidden()
  await expect(section.getByText("Claude Extra", { exact: true })).toBeVisible()
})

test("the built-in profile renames but offers no removal", async ({ page }) => {
  await openSources(page, "light")
  const section = profilesSection(page)
  await section.getByRole("button", { name: "More actions for Claude", exact: true }).click()
  await expect(page.getByRole("menuitem", { name: "Remove" })).toHaveCount(0)
  await page.getByRole("menuitem", { name: "Rename" }).click()
  const dialog = page.getByRole("dialog", { name: "Rename Claude profile" })
  await dialog.getByRole("textbox", { name: "Name" }).fill("Personal")
  await dialog.getByRole("button", { name: "Save changes" }).click()
  await expect(dialog).toBeHidden()
  await expect(section.getByText("Personal", { exact: true })).toBeVisible()
})

test("an added profile is removed after confirmation", async ({ page }) => {
  await openSources(page, "light")
  const section = profilesSection(page)
  await section.getByRole("button", { name: "More actions for Claude Work" }).click()
  await page.getByRole("menuitem", { name: "Remove" }).click()
  const confirm = page.getByRole("alertdialog", { name: "Remove Claude Work?" })
  await confirm.getByRole("button", { name: "Remove" }).click()
  await expect(confirm).toBeHidden()
  await expect(section.getByText("Claude Work", { exact: true })).toBeHidden()
  await expect(section.getByText("Default only")).toBeVisible()
})

for (const theme of ["light", "dark"] as const) {
  test(`Overview names every account and holds Retry for a rate limit in ${theme}`, async ({
    page,
  }, testInfo) => {
    await page.clock.setFixedTime(FIXED_TIME)
    await page.emulateMedia({ colorScheme: theme })
    await page.goto(`/tests/visual/?surface=main&profiles=1&theme=${theme}`)
    const limits = page.getByRole("region", { name: "Provider limits", exact: true })
    await expect(limits.getByRole("group", { name: "Claude Work, Max 20x plan" })).toBeVisible()
    const names = await limits
      .getByRole("group")
      .evaluateAll((groups) =>
        groups
          .filter((group) => group.parentElement?.closest('[role="group"]') == null)
          .map((group) => group.getAttribute("aria-label")?.split(",")[0]),
      )
    expect(names).toEqual(["Claude", "Claude Work", "Codex", "Claude Side"])
    await expect(limits.getByText(/account \d/i)).toHaveCount(0)
    await expect(limits.locator(".bg-separator")).toHaveCount(3)
    await expect(limits.getByRole("status")).toHaveText(/^Try again after .+\.$/)
    await expect(limits.getByRole("button", { name: "Retry Claude Side limits" })).toBeDisabled()
    await limits.screenshot({ path: testInfo.outputPath(`overview-limits-${theme}.png`) })
  })
}

test("the popover names profile accounts instead of numbering them", async ({ page }) => {
  await page.clock.setFixedTime(FIXED_TIME)
  await page.goto("/tests/visual/?surface=popover&profiles=1")
  await expect(page.getByText("Claude Work").first()).toBeVisible()
  await expect(page.getByText(/Claude account \d/)).toHaveCount(0)
})
