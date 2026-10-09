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
      await page.goto(`/tests/visual/?surface=main&theme=${theme}&onboarding=complete`)
      const navigation = page.getByRole("button", { name: "Open Main navigation" })
      if (width === 640) await navigation.click()
      const openChecks = page.getByRole("button", { name: /^Checks\s+9$/ })
      await expect(openChecks).toBeVisible({ timeout: 30_000 })
      await openChecks.click()
      const modal = page.getByRole("dialog", { name: "Checks", exact: true })
      await expect(modal).toBeVisible()
      const overdepth = modal.getByRole("switch", { name: "Session overdepth", exact: true })
      await expect(overdepth).toBeChecked()
      await overdepth.click()
      await expect(overdepth).not.toBeChecked()
      await expect(
        modal.getByRole("switch", { name: "Model overthinking", exact: true }),
      ).toBeChecked()
      const smart = modal.getByRole("switch", { name: "Ignored instructions", exact: true })
      await smart.scrollIntoViewIfNeeded()
      await expect(smart).toBeEnabled()
      await expect(smart).toBeChecked()
      await smart.evaluate((node) => node.scrollIntoView({ block: "start" }))
      await settleTransitions(page)
      await page.screenshot({
        path: testInfo.outputPath(`checks-${theme}-smart.png`),
        animations: "disabled",
      })
      await modal.getByRole("button", { name: "Close", exact: true }).click()
      if (width === 640 && (await navigation.isVisible())) await navigation.click()
      await page.getByRole("button", { name: /^Checks\s+8$/ }).click()
      await expect(overdepth).not.toBeChecked()
      await overdepth.scrollIntoViewIfNeeded()
      await settleTransitions(page)
      await page.screenshot({
        path: testInfo.outputPath(`checks-${theme}-local.png`),
        animations: "disabled",
      })
      expect(
        await modal.evaluate((node) => node.scrollWidth - node.clientWidth),
      ).toBeLessThanOrEqual(1)
    })
  }
}
