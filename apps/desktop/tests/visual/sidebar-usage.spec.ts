import { fileURLToPath } from "node:url"
import { expect, test } from "@playwright/test"

for (const theme of ["light", "dark"] as const) {
  test(`sidebar usage modal and empty pin control in ${theme}`, async ({ page }) => {
    await page.emulateMedia({ colorScheme: theme })
    await page.goto(`/tests/visual/?surface=main&theme=${theme}&onboarding=complete`)
    await page.getByRole("button", { name: "Close promotion" }).click()
    const trigger = page.locator("[data-usage-limits-trigger]").first()
    await expect(trigger).toBeVisible()
    await trigger.click()
    const dialog = page.getByRole("dialog", { name: "Usage limits" })
    await expect(dialog).toBeVisible()
    await expect(dialog).toBeFocused()
    await expect
      .poll(() =>
        dialog.evaluate((node) => getComputedStyle(node, "::backdrop").backgroundColor),
      )
      .toBe(theme === "light" ? "rgba(0, 0, 0, 0.1)" : "rgba(0, 0, 0, 0.24)")
    await page.screenshot({
      path: fileURLToPath(
        new URL(
          `../../../../.agent-artifacts/reviews/usage-modal-scrim-${theme}.png`,
          import.meta.url,
        ),
      ),
    })
    await expect(page.getByRole("tooltip")).toHaveCount(0)
    await page.keyboard.press("Tab")
    await expect(page.getByRole("button", { name: "Refresh usage limits" })).toBeFocused()
    await page.getByRole("radio", { name: "Bars" }).click()
    await expect(page.locator(".grid-cols-2")).toBeVisible()
    await page.getByRole("button", { name: "Refresh usage limits" }).click()
    await expect(dialog.getByRole("status")).toContainText("Usage limits refreshed.")
    const first = page.getByRole("button", { name: "Refresh usage limits" })
    await first.focus()
    await page.keyboard.press("Shift+Tab")
    await expect(
      dialog.getByRole("button", { name: /^(Unpin|Pin) Codex/ }).last(),
    ).toBeFocused()
    await page.keyboard.press("Escape")
    await expect(dialog).not.toBeVisible()
    await expect(trigger).toBeFocused()
    await expect(page.locator('[data-testid="usage-bar-notch"]').first()).toBeVisible()
    await trigger.hover()
    await expect(page.getByRole("tooltip")).toContainText("% pace")
    await expect(page.locator(".ui-tooltip")).toHaveAttribute("data-side", "top")
    await page.locator(".ui-tooltip").screenshot({
      path: fileURLToPath(
        new URL(
          `../../../../.agent-artifacts/reviews/usage-tooltip-${theme}.png`,
          import.meta.url,
        ),
      ),
    })
    await trigger.click()
    const bounds = (await dialog.boundingBox())!
    await page.mouse.move(bounds.x + 30, bounds.y + 30)
    await page.mouse.down()
    await page.mouse.move(2, 2)
    await page.mouse.up()
    await expect(dialog).toBeVisible()
    await page.mouse.click(2, 2)
    await expect(dialog).not.toBeVisible()
    await trigger.click()
    await page.getByRole("button", { name: /^Unpin Codex/ }).click()
    await page.getByRole("button", { name: "Close usage limits" }).click()
    const empty = page.getByRole("button", { name: "Pin usage limits" })
    await expect(empty).toBeFocused()
    await expect(empty).toHaveCSS("opacity", "1")
    await page.screenshot({
      path: fileURLToPath(
        new URL(
          `../../../../.agent-artifacts/reviews/sidebar-usage-${theme}-empty.png`,
          import.meta.url,
        ),
      ),
    })
    await empty.click()
    await page.screenshot({
      path: fileURLToPath(
        new URL(
          `../../../../.agent-artifacts/reviews/sidebar-usage-${theme}-modal.png`,
          import.meta.url,
        ),
      ),
    })
    await page.setViewportSize({ width: 800, height: 600 })
    await expect(dialog).toBeInViewport()
    await page.emulateMedia({ reducedMotion: "reduce" })
    await page.getByRole("button", { name: "Close usage limits" }).click()
    await expect(empty).toHaveCSS("transition-property", "none")
  })
}

for (const theme of ["light", "dark"] as const) {
  for (const usage of ["off", "on"] as const) {
    test(`empty usage ${usage} approval capture in ${theme}`, async ({ page }) => {
      await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" })
      await page.addInitScript(() =>
        localStorage.setItem("antiburn.promotions.seen.v1", '["claude-mod@1"]'),
      )
      await page.goto(`/tests/visual/?surface=main&state=empty&theme=${theme}&usage=${usage}`)
      const trigger = page.getByRole("button", { name: "Pin usage limits" })
      await trigger.focus()
      await trigger.press("Enter")
      const dialog = page.getByRole("dialog", { name: "Usage limits" })
      await expect(
        dialog.getByRole("heading", {
          name: usage === "off" ? "See your usage limits" : "Your limits will appear here",
        }),
      ).toBeVisible()
      await expect(dialog.getByRole("radio", { name: "Dials" })).toHaveCount(0)
      await expect(
        dialog.getByRole("button", {
          name: usage === "off" ? "Enable live usage" : "Check again",
        }),
      ).toBeVisible()
      await page.mouse.move(1000, 50)
      await dialog.screenshot({
        path: fileURLToPath(
          new URL(
            `../../../../.agent-artifacts/reviews/usage-empty-${usage}-${theme}.png`,
            import.meta.url,
          ),
        ),
      })
    })
  }
}

test("resizing with usage limits open does not leave promotions blocked", async ({ page }) => {
  await page.setViewportSize({ width: 1100, height: 800 })
  await page.goto("/tests/visual/?surface=main&theme=light&onboarding=complete")
  await page.getByRole("button", { name: "Close promotion" }).click()
  await page.locator("[data-usage-limits-trigger]").first().click()
  await expect(page.getByRole("dialog", { name: "Usage limits" })).toBeVisible()
  await page.setViewportSize({ width: 680, height: 800 })
  await expect(page.getByRole("dialog", { name: "Usage limits" })).toHaveCount(0)
  await page.setViewportSize({ width: 1100, height: 800 })
  await page.locator(".promotion-launcher").click()
  await expect(page.getByRole("button", { name: "Close promotion" })).toBeVisible()
})
