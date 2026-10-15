import { expect, test, type Page } from "@playwright/test"

async function openChecks(page: Page, compact: boolean) {
  await expect(page.getByRole("main", { name: "antiburn main window" })).toBeVisible({
    timeout: 30_000,
  })
  if (compact) {
    const navigation = page.getByRole("button", { name: "Open Main navigation" })
    await expect(navigation).toBeVisible()
    await navigation.press("Enter")
    await expect(page.getByRole("dialog", { name: "Main navigation" })).toBeVisible()
  }
  await page.getByRole("tab", { name: "Checks", exact: true }).press("Enter")
  await expect(page.getByRole("tabpanel", { name: "Checks", exact: true })).toBeVisible()
}

for (const theme of ["light", "dark"] as const) {
  for (const layout of [
    { name: "wide", width: 1280, height: 860, scale: 100 },
    { name: "narrow", width: 640, height: 640, scale: 100 },
    { name: "large-scale", width: 1000, height: 860, scale: 200 },
  ]) {
    test.describe(`${theme} ${layout.name}`, () => {
      test.use({ deviceScaleFactor: layout.scale / 100 })
      test(`smart-check evidence in ${theme} ${layout.name}`, async ({ page }, testInfo) => {
        test.setTimeout(60_000)
        const errors: string[] = []
        page.on("pageerror", (error) => errors.push(error.message))
        await page.setViewportSize({
          width: layout.width / (layout.scale / 100),
          height: layout.height / (layout.scale / 100),
        })
        await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" })
        await page.goto(
          `/tests/visual/?surface=main&checks=smart&onboarding=complete&theme=${theme}&scale=${layout.scale}`,
        )
        await openChecks(page, layout.width / (layout.scale / 100) < 720)
        for (const [label, detector] of [
          ["Over-exploring", "overExploring"],
          ["Ignored instructions", "ignoredInstructions"],
          ["Scope creep", "scopeCreep"],
          ["Skill opportunities", "skillOpportunities"],
        ] as const) {
          await page.getByRole("button", { name: new RegExp(`^${label},`) }).focus()
          await page.keyboard.press("Enter")
          const target = page.locator(`[data-detector="${detector}"]`)
          await expect(target).toBeVisible()
          await expect(target.locator("[data-snapshot-action-id]")).toBeVisible()
          await expect(target.getByRole("button", { name: "Hide details" })).toHaveCount(0)
          await expect(target.getByRole("button", { name: "Show evidence" })).toHaveCount(0)
          if (detector === "overExploring") {
            await expect(target.getByRole("heading", { name: "Files read" })).toHaveCount(1)
            const contents = target.getByRole("button", { name: "Show file contents" }).first()
            await page.keyboard.press("Tab")
            await contents.focus()
            await page.keyboard.press("Enter")
            await expect(
              target.getByRole("button", { name: "Hide file contents" }),
            ).toHaveAttribute("aria-expanded", "true")
            await page.keyboard.press("Enter")
            await expect(contents).toHaveAttribute("aria-expanded", "false")
          }
          if (detector === "scopeCreep")
            await expect(target.getByText("Proposed work", { exact: true })).toBeVisible()
          if (detector === "skillOpportunities")
            await expect(
              target.getByText("Explain the parser fix.", { exact: true }),
            ).toBeVisible()
          await expect
            .poll(() => target.evaluate((node) => node.scrollWidth - node.clientWidth))
            .toBeLessThanOrEqual(1)
          const overflow = await target.locator("pre, p, time, button").evaluateAll((nodes) =>
            nodes
              .filter((node) => {
                if (!node.getBoundingClientRect().width) return false
                const box = node.getBoundingClientRect()
                const viewport = document.documentElement.clientWidth
                return box.left < -1 || box.right > viewport + 1
              })
              .map((node) => node.textContent),
          )
          expect(overflow).toEqual([])
          await page.screenshot({
            path: testInfo.outputPath(`${detector}-${theme}-${layout.name}.png`),
            animations: "disabled",
          })
          const expansion = target.locator("summary").first()
          if (await expansion.isVisible()) {
            await expansion.focus()
            await page.keyboard.press("Enter")
            await expect(expansion).toHaveAttribute("aria-expanded", "true")
            await expect
              .poll(() => target.evaluate((node) => node.scrollWidth - node.clientWidth))
              .toBeLessThanOrEqual(1)
          }
          await target.locator("pre:visible").last().scrollIntoViewIfNeeded()
          await page.screenshot({
            path: testInfo.outputPath(`${detector}-${theme}-${layout.name}-excerpts.png`),
            animations: "disabled",
          })
        }
        for (const review of ["empty", "blocked"] as const) {
          await page.goto(
            `/tests/visual/?surface=main&checks=smart&review=${review}&onboarding=complete&theme=${theme}&scale=${layout.scale}`,
          )
          await openChecks(page, layout.width / (layout.scale / 100) < 720)
          await page.getByRole("button", { name: "Not assessed (1)" }).focus()
          await page.keyboard.press("Enter")
          const status = review === "empty" ? "Nothing to assess" : "Context blocked"
          await page.getByRole("button", { name: `Skill opportunities, ${status}` }).focus()
          await page.keyboard.press("Enter")
          const detail = page.locator("#burn-check-skillOpportunities-detail")
          await expect(detail.getByText(status, { exact: true }).last()).toBeVisible()
          await expect(page.locator("[data-detector]")).toHaveCount(0)
          await expect(detail.getByRole("button", { name: "Retry" })).toHaveCount(0)
          await expect(page.getByText("Checking", { exact: true })).toHaveCount(0)
          await expect(page.getByText("Passed", { exact: true })).toHaveCount(0)
          await page.screenshot({
            path: testInfo.outputPath(`${review}-${theme}-${layout.name}.png`),
            animations: "disabled",
          })
        }
        expect(errors).toEqual([])
      })
    })
  }
}
