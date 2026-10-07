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
  for (const width of [1280, 640]) {
    test(`check switches remain reachable and retain choices in ${theme} at ${width}`, async ({
      page,
    }, testInfo) => {
      await page.setViewportSize({ width, height: width === 640 ? 480 : 860 })
      await page.emulateMedia({ colorScheme: theme })
      await page.goto(`/tests/visual/?surface=settings&theme=${theme}`)
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
      const overdepth = pane.getByRole("switch", { name: "Session overdepth", exact: true })
      await expect(overdepth).toBeChecked()
      await overdepth.click()
      await expect(overdepth).not.toBeChecked()
      await expect(
        pane.getByRole("switch", { name: "Model overthinking", exact: true }),
      ).toBeChecked()
      const smart = pane.getByRole("switch", { name: "Ignored Instructions", exact: true })
      await smart.scrollIntoViewIfNeeded()
      await expect(smart).toBeEnabled()
      await expect(smart).toBeChecked()
      await smart.evaluate((node) => node.scrollIntoView({ block: "start" }))
      await settleTransitions(page)
      await page.screenshot({
        path: testInfo.outputPath(`checks-${theme}-smart.png`),
        animations: "disabled",
      })
      await openPane("General")
      await openPane("Checks")
      await expect(overdepth).not.toBeChecked()
      await overdepth.scrollIntoViewIfNeeded()
      await settleTransitions(page)
      await page.screenshot({
        path: testInfo.outputPath(`checks-${theme}-local.png`),
        animations: "disabled",
      })
      expect(
        await pane.evaluate((node) => node.scrollWidth - node.clientWidth),
      ).toBeLessThanOrEqual(1)
    })
  }
}
