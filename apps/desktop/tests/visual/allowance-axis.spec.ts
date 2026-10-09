import { expect, test } from "@playwright/test"

for (const theme of ["light", "dark"] as const) {
  for (const width of [1280, 640]) {
    test(`allowance axis stays visible in ${theme} at ${width}`, async ({ page }, testInfo) => {
      await page.setViewportSize({ width, height: width === 640 ? 480 : 860 })
      await page.clock.setFixedTime(new Date("2026-09-15T00:00:00.000Z"))
      await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" })
      await page.goto(`/tests/visual/?surface=main&onboarding=complete&theme=${theme}`)
      const usage = page.getByRole("region", { name: "Usage", exact: true })
      await expect(usage).toBeVisible({ timeout: 30_000 })
      await usage.getByRole("radio", { name: "Subscription" }).click()
      const tabs = await usage.getByRole("radiogroup", { name: "Usage unit" }).boundingBox()
      expect(tabs).not.toBeNull()
      for (const label of ["100%", "75%", "50%", "25%", "0%"]) {
        const tick = usage.getByText(label, { exact: true })
        await expect(tick).toBeVisible()
        const box = await tick.boundingBox()
        expect(box).not.toBeNull()
        expect(box!.x + box!.width).toBeLessThanOrEqual(width)
        expect(tabs!.x + tabs!.width).toBeLessThanOrEqual(box!.x)
      }
      await page.screenshot({
        path: testInfo.outputPath("allowance-axis.png"),
        animations: "disabled",
      })
    })
  }
}
