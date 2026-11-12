import { fileURLToPath } from "node:url"
import { expect, test } from "@playwright/test"

const headings = [
  "Stop hitting your token limits",
  "Understand what each session uses",
  "See your limits—and when they reset",
  "Stay informed without losing focus",
  "See what you could improve",
  "Upload all your sessions, across devices",
] as const
const steps = ["welcome", "sessions", "limits", "hud", "checks", "cloud"]
for (const theme of ["light", "dark"] as const) {
  test(`onboarding local journey, resume and Cloud gate in ${theme}`, async ({ page }) => {
    await page.setViewportSize({ width: 860, height: 620 })
    await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" })
    const errors: string[] = []
    page.on("pageerror", (error) => errors.push(error.message))
    await page.goto(`/tests/visual/?surface=onboarding&theme=${theme}`)
    for (const [i, heading] of headings.entries()) {
      await expect(page.getByRole("heading", { name: heading, exact: true })).toBeVisible()
      if (i > 0)
        await expect(page.frameLocator("iframe").locator("#root > *").first()).toBeVisible()
      await page.mouse.move(850, 610)
      await page.screenshot({
        path: fileURLToPath(
          new URL(
            `../../../../.agent-artifacts/reviews/onboarding-production/${theme}-${steps[i]}.png`,
            import.meta.url,
          ),
        ),
      })
      await expect(page.getByRole("button", { name: /skip/i })).toHaveCount(0)
      if (i === 2) {
        await page.reload()
        await expect(page.getByRole("heading", { name: heading, exact: true })).toBeVisible()
      }
      if (i < headings.length - 1)
        await page
          .getByRole("button", { name: i === 0 ? "Get started" : "Continue", exact: true })
          .click()
    }
    await page.getByRole("button", { name: "Set up Cloud", exact: true }).click()
    await expect(
      page.getByRole("heading", { name: "Antiburn Cloud is coming soon" }),
    ).toBeVisible()
    await expect(page.getByRole("button", { name: "Learn more" })).toBeVisible()
    await expect(
      page.getByRole("button", { name: /sign in with github|start syncing|add a folder/i }),
    ).toHaveCount(0)
    await page.screenshot({
      path: fileURLToPath(
        new URL(
          `../../../../.agent-artifacts/reviews/onboarding-production/${theme}-cloud-unavailable.png`,
          import.meta.url,
        ),
      ),
    })
    await page.getByRole("button", { name: "Done", exact: true }).click()
    await expect(page.getByRole("status")).toContainText("Opening Sessions")
    expect(await page.evaluate(() => sessionStorage.getItem("onboarding-review-state"))).toBe(
      "completed",
    )
    const commands = await page.evaluate(() => window.__ANTIBURN_ONBOARDING_CALLS__)
    expect(
      commands?.filter(
        (name) =>
          ![
            "get_onboarding_state",
            "set_onboarding_state",
            "get_onboarding_window_visible",
            "hud_island_available",
            "note_interaction",
            "cloud_status",
            "get_agent_integrations",
          ].includes(name),
      ),
    ).toEqual([])
    expect(errors).toEqual([])
  })
}

test("loads one isolated demo on demand and preserves Cost after native hide", async ({
  page,
}) => {
  await page.setViewportSize({ width: 860, height: 620 })
  await page.emulateMedia({ reducedMotion: "reduce" })
  const resources: string[] = []
  const external: string[] = []
  page.on("request", (request) => {
    const url = request.url()
    if (url.includes("/onboarding-demo/")) resources.push(url)
    if (/^https?:/.test(url) && new URL(url).hostname !== "127.0.0.1") external.push(url)
  })
  await page.goto("/tests/visual/?surface=onboarding&theme=light")
  await expect(page.getByRole("heading", { name: headings[0] })).toBeVisible()
  await expect(page.locator("iframe")).toHaveCount(0)
  expect(resources).toEqual([])
  await page.getByRole("button", { name: "Get started" }).click()
  await expect(page.locator("iframe")).toHaveCount(1)
  await expect(page.locator("iframe")).toHaveAttribute("sandbox", "allow-scripts")
  const demo = page.frameLocator("iframe")
  await expect(demo.getByRole("tab", { name: "Cost", exact: true })).toHaveAttribute(
    "aria-selected",
    "true",
  )
  await page.evaluate(() => window.__ANTIBURN_VISUAL_EMIT__?.("onboarding:visibility", false))
  await expect(demo.locator("#root")).toBeEmpty()
  await page.evaluate(() => window.__ANTIBURN_VISUAL_EMIT__?.("onboarding:visibility", true))
  await expect(demo.getByRole("tab", { name: "Cost", exact: true })).toHaveAttribute(
    "aria-selected",
    "true",
  )
  const firstDetails = await demo.locator(".session-detail").innerText()
  await demo.locator(".session-card").nth(1).click()
  await expect.poll(() => demo.locator(".session-detail").innerText()).not.toBe(firstDetails)
  await page.getByRole("button", { name: "Continue", exact: true }).click()
  await expect(page.locator("iframe")).toHaveCount(1)
  await page.getByRole("button", { name: "Back", exact: true }).click()
  await page.getByRole("button", { name: "Back", exact: true }).click()
  await expect(page.locator("iframe")).toHaveCount(0)
  expect(external).toEqual([])
})

test("failed demo script does not block navigation", async ({ page }) => {
  await page.route("**/onboarding-demo/demo.js", (route) => route.abort())
  await page.goto("/tests/visual/?surface=onboarding&step=sessions")
  await expect(page.getByText("The example could not load. You can continue.")).toBeVisible({
    timeout: 10000,
  })
  await page.getByRole("button", { name: "Continue", exact: true }).click()
  await expect(page.getByRole("heading", { name: headings[2] })).toBeVisible()
})

for (const theme of ["light", "dark"] as const) {
  test(`holds an unready themed frame and replays Checks without focus-scroll churn in ${theme}`, async ({
    page,
  }) => {
    await page.setViewportSize({ width: 860, height: 620 })
    await page.emulateMedia({ colorScheme: theme, reducedMotion: "no-preference" })
    await page.addInitScript(() => {
      const original = Element.prototype.scrollIntoView
      Object.assign(window, { checkFocusScrolls: 0 })
      Element.prototype.scrollIntoView = function (options) {
        if (this.closest(".checks-scene")) {
          const metrics = window as unknown as { checkFocusScrolls: number }
          metrics.checkFocusScrolls += 1
        }
        original.call(this, options)
      }
    })
    await page.goto(`/tests/visual/?surface=onboarding&theme=${theme}`)
    const marks = page.locator(".onboarding-agent-marks img")
    await expect(marks.first()).toHaveCSS(
      "filter",
      theme === "dark" ? "brightness(0) invert(1)" : "brightness(0)",
    )
    let release: (() => void) | undefined
    const pending = new Promise<void>((resolve) => {
      release = resolve
    })
    await page.route("**/onboarding-demo/demo.js", async (route) => {
      await pending
      await route.continue()
    })
    await page.getByRole("button", { name: "Get started", exact: true }).click()
    await expect(page.locator("iframe")).toHaveAttribute("data-ready", "false")
    await expect(page.locator("iframe")).toHaveCSS("opacity", "0")
    release!()
    await expect(page.locator("iframe")).toHaveAttribute("data-ready", "true")
    await expect(page.frameLocator("iframe").locator("html")).toHaveAttribute(
      "data-theme",
      theme,
    )
    for (let index = 0; index < 3; index++) {
      await page.getByRole("button", { name: "Continue", exact: true }).click()
      await expect(page.locator("iframe")).toHaveAttribute("data-ready", "true")
      if (index < 2) await expect(page.frameLocator("iframe").locator(".menu-app")).toBeEmpty()
    }
    for (let visit = 0; visit < 2; visit++) {
      const demo = page.frameLocator("iframe")
      await expect(demo.locator(".checks-scene")).toHaveClass(/step-3/, { timeout: 6000 })
      const metrics = await demo
        .locator(".burn-checks-collection-scroll")
        .evaluate((element) => ({
          scrollTop: element.scrollTop,
          focusScrolls: (window as unknown as { checkFocusScrolls: number }).checkFocusScrolls,
        }))
      expect(metrics.scrollTop).toBeGreaterThan(0)
      expect(metrics.focusScrolls).toBeLessThan(12)
      if (visit === 0) {
        await page.getByRole("button", { name: "Continue", exact: true }).click()
        await page.getByRole("button", { name: "Back", exact: true }).click()
      }
    }
  })
}
