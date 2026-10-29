import { expect, test, type Page } from "@playwright/test"

async function selectPane(page: Page, name: string) {
  const navigation = page.getByRole("button", { name: "Open Settings navigation" })
  if (await navigation.isVisible()) await navigation.click()
  await page.getByRole("tab", { name, exact: true }).click()
  return page.getByRole("tabpanel", { name, exact: true })
}

async function openChecks(page: Page) {
  await expect(page.getByRole("heading", { name: "General", exact: true })).toBeVisible({
    timeout: 30_000,
  })
  return selectPane(page, "Checks")
}

for (const theme of ["light", "dark"] as const) {
  for (const scale of [100, 200]) {
    test.describe(`${theme} at ${scale}%`, () => {
      const factor = scale / 100
      test.use({
        viewport: {
          width: Math.round(1100 / factor),
          height: Math.round(800 / factor),
        },
        deviceScaleFactor: factor,
        colorScheme: theme,
      })
      test("check files shows only confirmed files", async ({ page }, testInfo) => {
        await page.goto(
          `/tests/visual/?surface=settings&theme=${theme}&scale=${scale}&directories=mixed`,
        )
        const pane = await openChecks(page)
        await pane.getByRole("button", { name: "Check files", exact: true }).click()
        await expect(pane.getByText("6 check files", { exact: true })).toBeVisible()
        await expect(pane.getByText("2 check files", { exact: true })).toBeVisible()
        for (const label of [
          "New project",
          "Paused project",
          "Unavailable project",
          "Empty project",
          "Uncounted project",
          "Paused with files",
        ])
          await expect(pane.getByText(label, { exact: true })).toHaveCount(0)
        await expect(pane.getByRole("button", { name: /Refresh folders/ })).toHaveCount(0)
        await expect(pane.getByText(/unknown|file count unavailable/i)).toHaveCount(0)
        await expect(pane.getByRole("button", { name: /^Open in / })).toHaveCount(2)
        await pane
          .getByRole("button", { name: "Check files", exact: true })
          .evaluate((node) => node.scrollIntoView({ block: "start" }))
        await page.screenshot({
          path: testInfo.outputPath(`check-files-${theme}-${scale}.png`),
          animations: "disabled",
        })
        await pane.getByRole("button", { name: /^Open in .*This device/ }).click()
        expect(await page.evaluate(() => window.__ANTIBURN_VISUAL_DIRECTORY_OPENS__)).toEqual([
          "fixture-global-checks",
        ])
      })
    })
  }
}

test("revisiting Checks replaces the folder list without a refresh action", async ({
  page,
}) => {
  await page.goto("/tests/visual/?surface=settings&directories=mixed")
  let pane = await openChecks(page)
  await pane.getByRole("button", { name: "Check files", exact: true }).click()
  await expect(pane.getByText("6 check files", { exact: true })).toBeVisible()
  await selectPane(page, "General")
  await page.evaluate(() => {
    const url = new URL(window.location.href)
    url.searchParams.set("directories", "loading")
    history.replaceState(null, "", url)
  })
  pane = await selectPane(page, "Checks")
  await pane.getByRole("button", { name: "Check files", exact: true }).click()
  await expect(pane.getByText("Loading check folders…", { exact: true })).toBeVisible()
  await expect(pane.getByText("6 check files", { exact: true })).toHaveCount(0)
  await page.evaluate(() => {
    const url = new URL(window.location.href)
    url.searchParams.set("directories", "mixed")
    history.replaceState(null, "", url)
    window.__ANTIBURN_VISUAL_RELEASE_DIRECTORIES__?.()
  })
  await expect(pane.getByText("6 check files", { exact: true })).toBeVisible()
  await selectPane(page, "General")
  await page.evaluate(() => {
    const url = new URL(window.location.href)
    url.searchParams.set("directories", "empty")
    history.replaceState(null, "", url)
  })
  pane = await selectPane(page, "Checks")
  await pane.getByRole("button", { name: "Check files", exact: true }).click()
  await expect(pane.getByText("No check files found.", { exact: true })).toBeVisible()
  await expect(pane.getByText("6 check files", { exact: true })).toHaveCount(0)
  await expect(pane.getByRole("button", { name: /Refresh folders/ })).toHaveCount(0)
})

test("a failed folder open remains visible after Check files closes", async ({ page }) => {
  await page.goto("/tests/visual/?surface=settings&directoryOpen=error")
  const pane = await openChecks(page)
  const disclosure = pane.getByRole("button", { name: "Check files", exact: true })
  await disclosure.click()
  await pane.getByRole("button", { name: /^Open in .*This device/ }).click()
  await expect(pane.getByText(/^Could not open this check folder\./)).toBeVisible()
  await disclosure.click()
  await expect(disclosure).toHaveAttribute("aria-expanded", "false")
  await expect(pane.getByRole("alert")).toHaveText(
    "Could not open a check folder. Reopen Settings → Checks to try again.",
  )
})
