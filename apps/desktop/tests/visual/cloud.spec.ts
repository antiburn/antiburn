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
          page.getByRole("button", { name: "Connect in browser", exact: true }),
        ).toBeEnabled()
      if (state === "auth") {
        await page.getByRole("button", { name: "Connect in browser", exact: true }).click()
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
    await expect(page.locator(".cloud-folder-count").first()).toContainText("128 sessions")
    await expect(
      page.getByRole("button", { name: "Start syncing", exact: true }),
    ).toBeDisabled()
    await page.getByRole("switch").first().click()
    await expect(page.getByRole("switch").first()).not.toBeChecked()
    await expect(
      page.getByRole("button", { name: "Start syncing", exact: true }),
    ).toBeDisabled()
    await expect(page.getByRole("switch")).toHaveCount(2)
    await expect(page.getByRole("switch").last()).toBeChecked()
    await expect(page.getByRole("button", { name: "Not now", exact: true })).toHaveCount(0)
    await capture(page, `${theme}-folders`)
    await openCloud(page, "complete", theme)
    await expect(page.getByText("Cloud sync not started", { exact: true })).toBeVisible()
    await expect(
      page.getByRole("heading", { name: "Continue using Antiburn locally", exact: true }),
    ).toBeVisible()
    await expect(page.getByText("Agent integration ready", { exact: true })).toHaveCount(0)
    await capture(page, `${theme}-no-sync`)
    await page.getByRole("button", { name: "Done", exact: true }).click()
    await expect(page.locator("body")).toHaveAttribute("data-completed", "true")
    await openCloud(page, "empty", theme)
    await expect(page.getByText(/No project folders found/)).toBeVisible()
    await expect(
      page.getByRole("button", { name: "Start syncing", exact: true }),
    ).toBeDisabled()
    await capture(page, `${theme}-empty`)
  })

  test(`background sync never adds a blocking screen in ${theme}`, async ({ page }) => {
    for (const state of ["accepted", "partial", "syncing", "failed", "ambiguous"] as const) {
      await openCloud(page, state, theme)
      await page.getByRole("checkbox").check()
      await page.getByRole("button", { name: "Start syncing", exact: true }).click()
      await expect(page.locator(".cloud-screen-complete")).toBeVisible()
      await expect(page.getByRole("button", { name: "Check status", exact: true })).toHaveCount(
        0,
      )
      await expect(page.getByRole("button", { name: "Cancel sync", exact: true })).toHaveCount(
        0,
      )
      await expect(page.getByRole("progressbar")).toHaveCount(0)
      await expect(page.getByRole("button", { name: /Retry|Remove/ })).toHaveCount(0)
      await expect(page.getByText(/Ask from your coding agent/)).toHaveCount(0)
      await expect(page.getByRole("button", { name: "Done", exact: true })).toBeInViewport()
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
  await expect(page.getByText("Local agent setup pending", { exact: true })).toBeVisible()
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
  await page.getByRole("button", { name: "Connect in browser", exact: true }).click()
  await expect(page.getByRole("button", { name: "Cancel", exact: true })).toBeVisible()
  await expect(page.getByRole("heading", { name: "You’re signed in" })).toBeVisible()
  await expect(
    page.getByRole("heading", { name: "Choose what to sync to antiburn cloud" }),
  ).toBeVisible()
  await expect(page.getByRole("switch").first()).toBeChecked()
  await page.getByRole("checkbox").check()
  await page.getByRole("button", { name: "Start syncing", exact: true }).click()
  await expect(
    page.getByRole("heading", { name: "Sessions sync in the background" }),
  ).toBeVisible()
  await expect(
    page.getByText(/Background sync enabled|Sessions syncing/, { exact: true }),
  ).toBeVisible()
  await expect(page.getByText("Local agent ready", { exact: true })).toBeVisible()
  await expect(page.getByText("Local integration is active", { exact: true })).toBeVisible()
  await expect(page.locator(".cloud-sync-dot.is-syncing")).toHaveCount(1)
  await expect(page.locator(".cloud-agent-tick")).toHaveCount(1)
  await page.getByRole("button", { name: "Done", exact: true }).click()
  await expect(page.locator("body")).toHaveAttribute("data-completed", "true")
})

test("prototype completion indicators pulse only for active sync", async ({ page }) => {
  for (const theme of ["light", "dark"]) {
    await openCloud(page, "dev-walkthrough", theme)
    await page.getByRole("button", { name: "Connect in browser", exact: true }).click()
    await page.getByRole("checkbox").check()
    await page.getByRole("button", { name: "Start syncing", exact: true }).click()
    await page.emulateMedia({ reducedMotion: "no-preference" })
    const dot = page.locator(".cloud-sync-dot")
    await expect(dot).toHaveCSS("animation-name", "cloud-sync-pulse")
    await expect(dot).toHaveCSS("animation-duration", "2s")
    await expect(
      page.getByText(/Background sync enabled|Sessions syncing/, { exact: true }),
    ).toBeVisible()
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

test("grouped folders and full-content consent fit the desktop setup window", async ({
  page,
}) => {
  await openCloud(page, "folders")
  const rows = page.locator(".cloud-folder-row")
  await expect(rows).toHaveCount(2)
  await expect(rows.first()).toBeInViewport()
  await expect(rows.last()).toBeInViewport()
  await expect(page.getByText("1 worktree", { exact: true })).toBeVisible()
  await expect(page.getByRole("checkbox")).toBeInViewport()
  await expect(page.getByRole("button", { name: "Start syncing", exact: true })).toBeDisabled()
  await page.getByRole("checkbox").check()
  await expect(page.getByRole("button", { name: "Start syncing", exact: true })).toBeEnabled()
  await expect(page.getByRole("button", { name: /Remove/ })).toHaveCount(0)
})

test("keyboard users can inspect every included folder before confirming uploads", async ({
  page,
}) => {
  await openCloud(page, "folders")
  const disclosure = page.locator("summary[aria-label='Included folders for Studio']")
  const external = page.getByText("/Example/worktrees/studio-menu-fix", { exact: true })
  await expect(external).toBeHidden()
  await disclosure.focus()
  await expect(disclosure).toBeFocused()
  await disclosure.press("Enter")
  await expect(external).toBeVisible()
  await expect(page.getByRole("switch")).toHaveCount(2)
  await expect(page.getByRole("checkbox")).not.toBeChecked()
  await expect(page.getByRole("button", { name: "Start syncing", exact: true })).toBeDisabled()
  await page.getByRole("checkbox").check()
  await expect(page.getByRole("button", { name: "Start syncing", exact: true })).toBeEnabled()
})

test("compact Cloud Settings keeps pause and upload status above folder changes", async ({
  page,
}) => {
  await page.setViewportSize({ width: 760, height: 620 })
  await page.emulateMedia({ reducedMotion: "reduce" })
  await page.goto("/tests/visual/?surface=cloud&cloud=settings-progress&theme=light")
  const upload = page.getByRole("switch", { name: "Upload sessions", exact: true })
  await expect(upload).toBeChecked()
  await expect(upload).toBeInViewport()
  await expect(page.getByText(/41 uploaded · 3 waiting/)).toBeInViewport()
  await expect(page.getByRole("button", { name: "Retry saved uploads" })).toBeInViewport()
  const disclosure = page.locator("summary[aria-label='Included folders for Studio']")
  await disclosure.focus()
  await disclosure.press("Enter")
  await expect(
    page.getByText("/Example/worktrees/studio-menu-fix", { exact: true }),
  ).toBeVisible()
  await expect(page.getByRole("checkbox")).not.toBeChecked()
  await expect(page.getByRole("progressbar")).toHaveCount(0)
  await upload.click()
  await expect(upload).not.toBeChecked()
  await expect(page.getByText("Cloud uploads paused", { exact: true })).toBeVisible()
  await expect(page.getByRole("button", { name: "Update sync selection" })).toBeDisabled()
})

test("empty Cloud Settings offers session files instead of an unusable consent form", async ({
  page,
}) => {
  await page.goto("/tests/visual/?surface=cloud&cloud=settings-empty&theme=light")
  await expect(page.getByText(/No project folders found yet/)).toBeVisible()
  await expect(
    page.getByRole("button", { name: "Choose Claude session folders…" }),
  ).toBeVisible()
  await expect(page.getByRole("button", { name: "Choose OpenCode databases…" })).toBeVisible()
  await expect(page.getByRole("checkbox")).toHaveCount(0)
  await expect(page.getByRole("button", { name: "Start syncing", exact: true })).toHaveCount(0)
})

test("unreadable upload storage does not show cached totals as current", async ({ page }) => {
  await page.goto("/tests/visual/?surface=cloud&cloud=settings-storage-error&theme=light")
  await expect(
    page.getByText("Saved upload status is unavailable", { exact: true }),
  ).toBeVisible()
  await expect(page.getByText(/41 uploaded|0 uploaded|Transferred/)).toHaveCount(0)
  await expect(page.getByText("Background sync enabled", { exact: true })).toHaveCount(0)
  await expect(page.getByRole("button", { name: "Retry saved uploads" })).toBeEnabled()
  await expect(page.getByRole("button", { name: "Refresh folders" })).toBeEnabled()
})

test("reopened Settings keeps exclusions and does not confirm newly shown groups", async ({
  page,
}) => {
  await page.goto("/tests/visual/?surface=cloud&cloud=settings-reopened&theme=light")
  await expect(
    page.getByRole("switch", { name: "Sync Website: /Example/Website" }),
  ).not.toBeChecked()
  await expect(
    page.getByRole("switch", { name: "Sync New project: /Example/NewProject" }),
  ).toBeChecked()
  await expect(page.getByRole("checkbox")).not.toBeChecked()
  await expect(page.getByRole("button", { name: "Update sync selection" })).toBeDisabled()
})
