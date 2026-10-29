import { expect, test, type Page } from "@playwright/test"

async function settleTransitions(page: Page) {
  await page.evaluate(async () => {
    await document.fonts.ready
    await Promise.all(
      document
        .getAnimations()
        .filter((animation) =>
          Number.isFinite(Number(animation.effect?.getComputedTiming().endTime)),
        )
        .map((animation) => animation.finished.catch(() => undefined)),
    )
  })
}

for (const theme of ["light", "dark"] as const) {
  for (const { width, height, scale } of [
    { width: 1280, height: 860, scale: 100 },
    { width: 640, height: 600, scale: 200 },
  ]) {
    test.describe(`${theme} at ${scale}%`, () => {
      const factor = scale / 100
      test.use({
        viewport: { width: Math.round(width / factor), height: Math.round(height / factor) },
        deviceScaleFactor: factor,
        colorScheme: theme,
      })
      test(`check switches remain reachable and retain choices in ${theme} at ${scale}%`, async ({
        page,
      }, testInfo) => {
        await page.goto(`/tests/visual/?surface=settings&theme=${theme}&scale=${scale}`)
        await expect(page.getByRole("tabpanel", { name: "General" })).toBeVisible({
          timeout: 30_000,
        })
        async function openPane(name: string) {
          const navigation = page.getByRole("button", { name: "Open Settings navigation" })
          if (await navigation.isVisible()) await navigation.click()
          await page.getByRole("tab", { name, exact: true }).click()
        }
        await openPane("Checks")
        const pane = page.getByRole("tabpanel", { name: "Checks" })
        await expect(pane.getByRole("heading", { level: 2 })).toHaveText([
          "Local checks",
          "Model-based checks",
        ])
        await expect(pane.getByRole("heading", { name: "Decision model" })).toHaveCount(0)
        const overdepth = pane.getByRole("switch", { name: "Session overdepth", exact: true })
        await expect(overdepth).toBeChecked()
        await overdepth.click()
        await expect(overdepth).not.toBeChecked()
        await expect(
          pane.getByRole("switch", { name: "Model overthinking", exact: true }),
        ).toBeChecked()
        const smart = pane.getByRole("switch", { name: "Run checks", exact: true })
        await smart.scrollIntoViewIfNeeded()
        await expect(smart).toBeEnabled()
        await expect(smart).not.toBeChecked()
        await expect(
          pane.getByRole("button", { name: "Add connection", exact: true }),
        ).toBeEnabled()
        await expect(pane.getByLabel("Provider", { exact: true })).toHaveCount(0)
        await expect(pane.getByLabel("Jev API key", { exact: true })).toHaveCount(0)
        const modelSection = pane.locator("section").filter({
          has: page.getByRole("heading", { name: "Model-based checks", exact: true }),
        })
        await expect(modelSection.locator(".rounded-popover")).toHaveCount(1)
        await expect(
          modelSection.getByRole("button", { name: "Check files", exact: true }),
        ).toHaveAttribute("aria-expanded", "false")
        await expect(
          modelSection.getByRole("button", { name: "Model usage", exact: true }),
        ).toHaveAttribute("aria-expanded", "false")
        await expect(pane.getByRole("switch")).toHaveCount(10)
        await pane
          .getByRole("heading", { name: "Model-based checks", exact: true })
          .evaluate((node) => node.scrollIntoView({ block: "start" }))
        await settleTransitions(page)
        await page.screenshot({
          path: testInfo.outputPath(`checks-${theme}-${scale}-model-based.png`),
          animations: "disabled",
        })
        await openPane("General")
        await openPane("Checks")
        await expect(overdepth).not.toBeChecked()
        await expect(smart).not.toBeChecked()
        await overdepth.scrollIntoViewIfNeeded()
        await settleTransitions(page)
        await page.screenshot({
          path: testInfo.outputPath(`checks-${theme}-${scale}-local.png`),
          animations: "disabled",
        })
        expect(
          await pane.evaluate((node) => node.scrollWidth - node.clientWidth),
        ).toBeLessThanOrEqual(1)
      })
    })
  }
}
