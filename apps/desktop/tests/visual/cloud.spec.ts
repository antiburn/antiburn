import { fileURLToPath } from "node:url"
import { expect, test, type Page } from "@playwright/test"

async function openCloud(page: Page, state: string, theme = "light", scale = 100) {
  await page.setViewportSize({ width: 860, height: 620 })
  await page.emulateMedia({ colorScheme: theme as "light" | "dark", reducedMotion: "reduce" })
  await page.goto(`/tests/visual/?surface=cloud&cloud=${state}&theme=${theme}&scale=${scale}`)
  await expect(page.getByRole("main", { name: "Antiburn Cloud setup" })).toBeVisible()
  await expect(page.getByText("Checking Cloud availability…")).toHaveCount(0)
  await expect(page.locator(".cloud-shell")).toHaveCSS("display", "grid")
  await expect(page.locator(".cloud-footer")).toHaveCSS(
    "display",
    ["identity", "identity_error"].includes(state) ? "none" : "flex",
  )
}

async function capture(page: Page, name: string) {
  await page.screenshot({
    path: fileURLToPath(
      new URL(
        `../../../../.agent-artifacts/reviews/onboarding-cloud/${name}.png`,
        import.meta.url,
      ),
    ),
  })
}

for (const theme of ["light", "dark"] as const) {
  test(`Cloud handoff and recovery states stay truthful in ${theme}`, async ({ page }) => {
    const external: string[] = []
    const errors: string[] = []
    page.on("request", (request) => {
      const url = new URL(request.url())
      if (["https:", "http:"].includes(url.protocol) && url.hostname !== "127.0.0.1")
        external.push(url.href)
    })
    page.on("pageerror", (error) => errors.push(error.message))
    await openCloud(page, "marketing", theme)
    await expect(page.getByRole("button", { name: "Learn more", exact: true })).toBeEnabled()
    await expect(page.getByRole("button", { name: /sign in with GitHub/i })).toHaveCount(0)
    await expect(page.getByRole("button", { name: "Learn more", exact: true })).toHaveClass(
      "onboarding-secondary",
    )
    await expect(page.getByRole("button", { name: "Done", exact: true })).toHaveClass(
      "onboarding-primary",
    )
    const footer = await page.locator(".cloud-footer").boundingBox()
    expect(footer?.y).toBeGreaterThan(500)
    await capture(page, `${theme}-marketing`)
    await page.getByRole("button", { name: "Learn more", exact: true }).click()
    await expect(page.getByRole("button", { name: "Learn more", exact: true })).toBeEnabled()
    await expect(page.getByRole("button", { name: "Cancel", exact: true })).toHaveCount(0)
    await expect(page.getByRole("status")).toBeEmpty()
    await page.getByRole("button", { name: "Learn more", exact: true }).click()
    await expect(page.getByRole("button", { name: "Done", exact: true })).toBeEnabled()
    await openCloud(page, "marketing_failed", theme)
    await expect(page.getByRole("status")).toContainText("browser could not open")
    await page.getByRole("button", { name: "Learn more", exact: true }).click()
    await expect(page.getByRole("status")).toBeEmpty()
    await expect(page.getByRole("heading", { name: "You’re signed in" })).toHaveCount(0)
    for (const state of [
      "auth",
      "opening",
      "waiting",
      "returned",
      "expired",
      "browser_failed",
      "offline",
      "credential_failed",
    ]) {
      await openCloud(page, state, theme)
      await expect(page.getByRole("heading", { name: "You’re signed in" })).toHaveCount(0)
      await expect(
        page.getByRole("button", { name: "Start syncing", exact: true }),
      ).toHaveCount(0)
      if (state === "auth")
        await expect(
          page.getByRole("button", { name: "Sign in with GitHub", exact: true }),
        ).toBeEnabled()
      if (state === "auth") {
        await page.getByRole("button", { name: "Sign in with GitHub", exact: true }).click()
        await expect(page.getByRole("status")).toContainText("Waiting for browser")
        await page.getByRole("button", { name: "Cancel", exact: true }).click()
        await expect(page.getByRole("status")).toContainText("cancelled")
      }
      if (state === "returned") {
        await expect(page.getByRole("status")).toBeEmpty()
        await expect(
          page.getByRole("button", { name: "Learn more", exact: true }),
        ).toBeEnabled()
        await expect(page.getByRole("button", { name: "Cancel", exact: true })).toHaveCount(0)
      }
      await capture(page, `${theme}-${state}`)
    }
    expect(errors).toEqual([])
    expect(external).toEqual([])
  })

  test(`verified identity and local folder choice in ${theme}`, async ({ page }) => {
    await openCloud(page, "identity", theme)
    await expect(page.getByRole("heading", { name: "You’re signed in" })).toBeVisible()
    await capture(page, `${theme}-identity`)
    await openCloud(page, "folders", theme)
    await expect(page.getByRole("switch")).toHaveCount(2)
    await expect(page.getByRole("switch").first()).toBeChecked()
    await expect(page.getByText("128 sessions", { exact: true })).toBeVisible()
    await expect(
      page.getByRole("button", { name: "Start syncing", exact: true }),
    ).toBeDisabled()
    await page.getByRole("switch").first().click()
    await expect(page.getByRole("switch").first()).not.toBeChecked()
    await expect(
      page.getByRole("button", { name: "Start syncing", exact: true }),
    ).toBeDisabled()
    await page.getByRole("button", { name: /Add a folder/ }).click()
    await expect(page.getByRole("switch")).toHaveCount(3)
    await expect(page.getByRole("switch").last()).toBeChecked()
    await expect(page.getByRole("button", { name: "Not now", exact: true })).toHaveCount(0)
    await capture(page, `${theme}-folders`)
    await openCloud(page, "complete", theme)
    await expect(page.getByText("Cloud sync not started", { exact: true })).toBeVisible()
    await expect(
      page.getByRole("heading", { name: "Sessions syncing! Your agent is ready", exact: true }),
    ).toBeVisible()
    await expect(page.getByText("Agent integration ready", { exact: true })).toHaveCount(0)
    await capture(page, `${theme}-no-sync`)
    await page.getByRole("button", { name: "Done", exact: true }).click()
    await expect(page.locator("body")).toHaveAttribute("data-completed", "true")
    await openCloud(page, "empty", theme)
    await expect(page.getByText(/No folders added/)).toBeVisible()
    await expect(
      page.getByRole("button", { name: "Start syncing", exact: true }),
    ).toBeDisabled()
    await capture(page, `${theme}-empty`)
  })

  test(`background sync never adds a blocking screen in ${theme}`, async ({ page }) => {
    for (const state of ["accepted", "partial", "syncing", "failed", "ambiguous"] as const) {
      await openCloud(page, state, theme)
      await page.getByRole("button", { name: "Start syncing", exact: true }).click()
      await expect(page.locator(".cloud-screen-complete")).toBeVisible()
      await expect(page.getByRole("button", { name: "Check status", exact: true })).toHaveCount(
        0,
      )
      await expect(page.getByRole("button", { name: "Cancel sync", exact: true })).toHaveCount(
        0,
      )
      await expect(page.getByRole("progressbar")).toHaveCount(0)
      await page.getByRole("button", { name: "Done", exact: true }).click()
      await expect(page.locator("body")).toHaveAttribute("data-completed", "true")
    }
  })
}

test("Cloud keyboard navigation and reduced-motion completion remain usable at larger scale", async ({
  page,
}) => {
  await openCloud(page, "marketing", "dark", 125)
  await page.keyboard.press("Tab")
  await expect(page.getByRole("button", { name: "Learn more", exact: true })).toBeFocused()
  await page.keyboard.press("Enter")
  await expect(page.getByRole("button", { name: "Learn more", exact: true })).toBeEnabled()
  await expect(page.getByRole("button", { name: "Cancel", exact: true })).toHaveCount(0)
  await openCloud(page, "complete", "dark", 125)
  await expect(page.getByRole("button", { name: "Done", exact: true })).toBeInViewport()
  await expect(page.getByText("Cloud sync not started", { exact: true })).toBeVisible()
  await expect(page.getByText("Agent setup pending", { exact: true })).toBeVisible()
  await expect(page.locator(".cloud-ready-questions li")).toHaveCount(2)
  await expect(page.locator(".cloud-ready-questions li").first()).toHaveCSS(
    "animation-name",
    "none",
  )
  await capture(page, "dark-complete-scale125")
})

test("Cloud prompt animation pauses while hidden and resumes without layout shifts", async ({
  page,
}) => {
  await page.setViewportSize({ width: 860, height: 620 })
  await page.emulateMedia({ colorScheme: "dark", reducedMotion: "no-preference" })
  await page.goto("/tests/visual/?surface=cloud&cloud=complete&theme=dark")
  const cards = page.locator(".cloud-ready-questions")
  await expect(cards).toBeVisible()
  const height = (await cards.boundingBox())?.height
  await page.evaluate(() => {
    Object.defineProperty(document, "hidden", { configurable: true, value: true })
    document.dispatchEvent(new Event("visibilitychange"))
  })
  const first = page.locator(".cloud-prompt-character").first()
  await expect(first).toHaveCSS("animation-play-state", "paused")
  const pausedAt = await first.evaluate(async (element) => {
    const animation = element.getAnimations()[0]
    await animation?.ready
    return Number(animation?.currentTime)
  })
  await page.waitForTimeout(120)
  const stillAt = await first.evaluate((element) =>
    Number(element.getAnimations()[0]?.currentTime),
  )
  expect(Math.abs(stillAt - pausedAt)).toBeLessThan(2)
  await page.evaluate(() => {
    Object.defineProperty(document, "hidden", { configurable: true, value: false })
    document.dispatchEvent(new Event("visibilitychange"))
  })
  await expect(first).toHaveCSS("animation-play-state", "running")
  await expect(page.locator(".cloud-prompt-character").last()).toHaveCSS("opacity", "1", {
    timeout: 7000,
  })
  expect((await cards.boundingBox())?.height).toBe(height)
  await capture(page, "dark-completion-normal-motion")
})

test("Cloud Dev walkthrough signs in and syncs simulated folders", async ({ page }) => {
  await openCloud(page, "dev-walkthrough")
  await page.getByRole("button", { name: "Sign in with GitHub", exact: true }).click()
  await expect(page.getByRole("button", { name: "Cancel", exact: true })).toBeVisible()
  await expect(page.getByRole("heading", { name: "You’re signed in" })).toBeVisible()
  await expect(
    page.getByRole("heading", { name: "Choose what to sync to antiburn cloud" }),
  ).toBeVisible()
  await expect(page.getByRole("switch").first()).toBeChecked()
  await page.getByRole("button", { name: "Start syncing", exact: true }).click()
  await expect(
    page.getByRole("heading", { name: "Sessions syncing! Your agent is ready" }),
  ).toBeVisible()
  await expect(page.getByText("Syncing 2 folders", { exact: true })).toBeVisible()
  await expect(page.getByText("Agent setup", { exact: true })).toBeVisible()
  await expect(page.getByText("Ask from your coding agent", { exact: true })).toBeVisible()
  await expect(page.locator(".cloud-sync-dot.is-syncing")).toHaveCount(1)
  await expect(page.locator(".cloud-agent-tick")).toHaveCount(1)
  await page.getByRole("button", { name: "Done", exact: true }).click()
  await expect(page.locator("body")).toHaveAttribute("data-completed", "true")
})

test("prototype completion indicators pulse only for active sync", async ({ page }) => {
  for (const theme of ["light", "dark"]) {
    await openCloud(page, "dev-walkthrough", theme)
    await page.getByRole("button", { name: "Sign in with GitHub", exact: true }).click()
    await page.getByRole("button", { name: "Start syncing", exact: true }).click()
    await page.emulateMedia({ reducedMotion: "no-preference" })
    const dot = page.locator(".cloud-sync-dot")
    await expect(dot).toHaveCSS("animation-name", "cloud-sync-pulse")
    await expect(dot).toHaveCSS("animation-duration", "2s")
    await expect(page.getByText("Syncing 2 folders", { exact: true })).toBeVisible()
    await expect(page.locator(".cloud-agent-tick")).toBeVisible()
    await capture(page, `${theme}-prototype-syncing`)
    await page.emulateMedia({ reducedMotion: "reduce" })
    await expect(dot).toHaveCSS("animation-name", "none")
    await openCloud(page, "dev-complete-pending", theme)
    await expect(page.locator(".cloud-agent-tick")).toHaveCount(0)
    await expect(page.locator(".cloud-sync-dot.is-syncing")).toHaveCount(0)
  }
})

test("failed identity handoff remains retryable", async ({ page }) => {
  await openCloud(page, "identity_error")
  await expect(page.getByRole("alert")).toBeVisible()
  await page.getByRole("button", { name: "Try again", exact: true }).click()
  await expect(page.getByRole("switch")).toHaveCount(2)
})
