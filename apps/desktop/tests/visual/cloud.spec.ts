import { fileURLToPath } from "node:url"
import { expect, test, type Locator, type Page } from "@playwright/test"

async function openCloud(page: Page, state: string, theme = "light", scale = 100) {
  await page.setViewportSize({ width: 860, height: 620 })
  await page.emulateMedia({ colorScheme: theme as "light" | "dark", reducedMotion: "reduce" })
  await page.goto(`/tests/visual/?surface=cloud&cloud=${state}&theme=${theme}&scale=${scale}`)
  await expect(page.getByRole("main", { name: "Antiburn Cloud setup" })).toBeVisible()
  await expect(page.getByText("Checking Cloud availability…")).toHaveCount(0)
  await expect(page.locator(".cloud-shell")).toHaveCSS(
    "display",
    (await page.locator(".cloud-screen-folders").count()) ? "flex" : "grid",
  )
  await expect(page.locator(".cloud-footer")).toHaveCSS(
    "display",
    ["identity", "identity_error"].includes(state) ? "none" : "flex",
  )
}

async function capture(page: Page, name: string, directory = "desktop-cloud-simplified") {
  await page.screenshot({
    path: fileURLToPath(
      new URL(`../../../../.agent-artifacts/reviews/${directory}/${name}.png`, import.meta.url),
    ),
  })
}

async function expectWarningAboveFooter(warning: Locator, footer: Locator) {
  await expect(warning).toBeVisible()
  await expect(warning.locator('[aria-hidden="true"] > svg')).toHaveCount(1)
  await expect(footer.getByRole("alert")).toHaveCount(0)
  const [message, bounds] = await Promise.all([warning.boundingBox(), footer.boundingBox()])
  if (!message || !bounds) throw new Error("The warning and footer must be visible.")
  expect(message.y + message.height).toBeLessThanOrEqual(bounds.y)
}

test("browser setup and return work by keyboard inside onboarding", async ({ page }) => {
  await page.setViewportSize({ width: 640, height: 480 })
  await page.emulateMedia({ colorScheme: "dark", reducedMotion: "reduce" })
  await page.goto("/tests/visual/?surface=cloud&cloud=identity&theme=dark&browserSetup")
  const open = page.getByRole("button", { name: "Finish setup in browser" })
  await expect(open).toBeEnabled({ timeout: 15_000 })
  await open.focus()
  await page.keyboard.press("Enter")
  await expect(page.locator("body")).toHaveAttribute("data-browser-setup-opened", "true")
  await expect(page.getByRole("textbox")).toHaveCount(0)
  const next = page.getByRole("button", { name: "Continue", exact: true })
  await expect(next).toBeInViewport()
  const local = page.getByRole("button", { name: "Continue locally" })
  await expect(local).toBeInViewport()
  await capture(page, "dark-onboarding-browser-setup")
  await page.keyboard.press("Tab")
  await expect(local).toBeFocused()
  await page.keyboard.press("Tab")
  await expect(next).toBeFocused()
  await page.keyboard.press("Enter")
  await expect(page.getByRole("button", { name: "Start syncing", exact: true })).toBeVisible()
  await expect(page.getByText(/No sync destination selected/)).toBeVisible()
})

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
        await page.getByRole("button", { name: "Cancel sign-in", exact: true }).click()
        await expect(page.getByRole("status")).toContainText("cancelled")
      }
      if (state === "returned") {
        await expect(page.getByRole("status")).toContainText("Checking your sign-in")
        await expect(
          page.getByRole("button", { name: "Sign in with GitHub", exact: true }),
        ).toHaveCount(0)
        await expect(
          page.getByRole("button", { name: "Cancel sign-in", exact: true }),
        ).toHaveCount(0)
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
    await expect(page.getByRole("button", { name: "Start syncing", exact: true })).toBeEnabled()
    await page.getByRole("switch").first().click()
    await expect(page.getByRole("switch").first()).not.toBeChecked()
    await expect(page.getByRole("button", { name: "Start syncing", exact: true })).toBeEnabled()
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
      await expect(page.getByRole("checkbox")).toHaveCount(0)
      await page.getByRole("button", { name: "Start syncing", exact: true }).click()
      if (state === "ambiguous") {
        await expect(page.getByText(/Could not confirm folder sync for Personal/)).toBeVisible()
        await expect(
          page.getByRole("button", { name: "Start syncing", exact: true }),
        ).toBeEnabled()
        await page.getByRole("button", { name: "Continue locally", exact: true }).click()
        await expect(page.locator("body")).toHaveAttribute("data-completed", "true")
        continue
      }
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
  await expect(page.getByText("Setting up local tools", { exact: true })).toBeVisible()
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
  await expect(page.getByRole("button", { name: "Cancel sign-in", exact: true })).toBeVisible()
  await expect(page.getByRole("heading", { name: "You’re signed in" })).toBeVisible()
  await expect(
    page.getByRole("heading", { name: "Choose what to sync to antiburn cloud" }),
  ).toBeVisible()
  await expect(page.getByRole("switch").first()).toBeChecked()
  await expect(page.getByRole("checkbox")).toHaveCount(0)
  await page.getByRole("button", { name: "Start syncing", exact: true }).click()
  await expect(
    page.getByRole("heading", { name: "Sessions sync in the background" }),
  ).toBeVisible()
  await expect(
    page.getByText(/Background sync enabled|Sessions syncing/, { exact: true }),
  ).toBeVisible()
  await expect(page.getByText("Local tools ready", { exact: true })).toBeVisible()
  await expect(page.getByText("CLI tested · Skill installed", { exact: true })).toBeVisible()
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
  await expect(page.getByRole("checkbox")).toHaveCount(0)
  await expect(page.getByText("Sync full sessions from approved folders.")).toBeInViewport()
  await expect(page.locator("footer p")).toHaveCount(0)
  await expect(page.locator("footer")).not.toContainText("folders selected")
  await expect(page.getByRole("button", { name: "Start syncing", exact: true })).toBeEnabled()
  await expect(page.getByRole("button", { name: /Remove/ })).toHaveCount(0)
})

test("keyboard folder choices stay local until an explicit Start", async ({ page }) => {
  await openCloud(page, "folders")
  await expect(
    page.locator(".cloud-folder-row details, .cloud-folder-row summary"),
  ).toHaveCount(0)
  await expect(
    page.getByText("/Example/worktrees/studio-menu-fix", { exact: true }),
  ).toHaveCount(0)
  const folder = page.getByRole("switch").first()
  await folder.focus()
  await folder.press("Space")
  await expect(folder).not.toBeChecked()
  await expect(folder).toHaveAccessibleDescription(/128 sessions/)
  await expect(page.getByRole("switch")).toHaveCount(2)
  await expect(page.getByRole("checkbox")).toHaveCount(0)
  await expect(page.getByRole("button", { name: "Start syncing", exact: true })).toBeEnabled()
})

test("compact Cloud Settings keeps the real upload control without progress boilerplate", async ({
  page,
}) => {
  await page.setViewportSize({ width: 760, height: 620 })
  await page.emulateMedia({ reducedMotion: "reduce" })
  await page.goto("/tests/visual/?surface=cloud&cloud=settings-progress&theme=light")
  const upload = page.getByRole("switch", {
    name: "Upload sessions to Antiburn Cloud",
    exact: true,
  })
  await expect(upload).toBeChecked()
  await expect(upload).toBeInViewport()
  await expect(page.getByText(/41 uploaded|Transferred/)).toHaveCount(0)
  await expect(page.getByRole("button", { name: "Retry saved uploads" })).toHaveCount(0)
  await expect(page.getByRole("button", { name: "Refresh folders" })).toHaveCount(0)
  await expect(page.locator("[id^='cloud-settings-folder-'] details")).toHaveCount(0)
  await expect(
    page.getByText("/Example/worktrees/studio-menu-fix", { exact: true }),
  ).toHaveCount(0)
  await expect(page.getByRole("checkbox")).toHaveCount(0)
  await expect(page.getByRole("progressbar")).toHaveCount(0)
  await upload.click()
  await expect(upload).not.toBeChecked()
  await expect(page.getByText("Cloud uploads paused", { exact: true })).toHaveCount(0)
  await expect(page.getByRole("button", { name: "Update sync selection" })).toHaveCount(0)
  await expect(
    page.getByText("Uploads are paused on this device.", { exact: true }),
  ).toBeVisible()
})

test("empty Cloud Settings uses default sources without pickers or a consent form", async ({
  page,
}) => {
  await page.goto("/tests/visual/?surface=cloud&cloud=settings-empty&theme=light")
  await expect(page.getByText("No session folders found.")).toBeVisible()
  await expect(
    page.getByRole("button", { name: "Choose Claude session folders…" }),
  ).toHaveCount(0)
  await expect(page.getByRole("button", { name: "Choose OpenCode databases…" })).toHaveCount(0)
  await expect(page.getByRole("checkbox")).toHaveCount(0)
  await expect(page.getByRole("button", { name: "Start syncing", exact: true })).toBeDisabled()
})

for (const width of [760, 480]) {
  test(`Cloud Settings only offers a folder recheck when a partial list blocks use at ${width}px`, async ({
    page,
  }) => {
    await page.setViewportSize({ width, height: 780 })
    const settings = page.getByRole("main", { name: "Cloud Settings preview" })
    const footer = settings.locator("footer")
    const retry = footer.getByRole("button", { name: "Check again" })

    await page.goto(
      "/tests/visual/?surface=cloud&cloud=settings-missing-sources&partialSettingsFolders=1",
    )
    const alert = settings.getByRole("alert")
    await expect(alert).toHaveText("Could not check session folders.")
    await expectWarningAboveFooter(alert, footer)
    await expect(retry).toHaveAccessibleDescription("Could not check session folders.")
    await retry.press("Enter")
    await expect(settings.getByRole("switch", { name: /Sync Studio/ })).toBeEnabled()
    await expect(alert).toHaveCount(0)

    await page.goto(
      "/tests/visual/?surface=cloud&cloud=settings-folders&footerPreview=1&partialSettingsFolders=1",
    )
    await expect(page.locator("body")).toHaveAttribute("data-folder-read-calls", "1")
    await expect(settings.getByRole("switch", { name: /Sync Studio/ })).toBeChecked()
    await expect(footer.getByRole("button", { name: "Sync now" })).toBeEnabled()
    await expect(alert).toHaveCount(0)
    await expect(retry).toHaveCount(0)
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(width)
  })
}

test("unreadable upload storage does not show cached totals as current", async ({ page }) => {
  await page.goto("/tests/visual/?surface=cloud&cloud=settings-storage-error&theme=light")
  await expect(
    page.getByText("Could not read saved uploads. Saved files are kept.", { exact: true }),
  ).toBeVisible()
  await expect(
    page.getByRole("group", { name: "Cloud folders" }).getByRole("switch"),
  ).toHaveCount(0)
  await expect(page.getByText(/41 uploaded|0 uploaded|Transferred/)).toHaveCount(0)
  await expect(page.getByText("Background sync enabled", { exact: true })).toHaveCount(0)
  await expect(page.getByRole("button", { name: "Check saved uploads" })).toHaveCount(0)
  await expect(page.getByRole("button", { name: "Refresh folders" })).toHaveCount(0)
})

test("the first Personal folder save can be retried at narrow width", async ({ page }) => {
  const width = 375
  await page.setViewportSize({ width, height: 620 })
  await page.goto("/tests/visual/?surface=cloud&cloud=settings-folders&personalSaveFailure=1")
  const settings = page.getByRole("main", { name: "Cloud Settings preview" })
  const footer = settings.locator("footer")
  const save = footer.getByRole("button", { name: "Start syncing", exact: true })
  const retry = footer.getByRole("button", { name: "Try again", exact: true })
  const saveCalls = page.locator("body")
  const pause = page.getByRole("switch", { name: "Upload sessions to Antiburn Cloud" })
  await expect(pause).toHaveCount(0)
  await expect(save).toBeEnabled()
  await expect(retry).toHaveCount(0)
  await save.click()
  await expect(settings.getByRole("alert")).toHaveText("Could not save the folder choices.")
  await expect(retry).toBeEnabled()
  await expect(footer.getByRole("button")).toHaveCount(1)
  await expect(save).toHaveCount(0)
  await expect(saveCalls).toHaveAttribute("data-personal-save-calls", "1")
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(width)
  expect(await settings.evaluate((pane) => pane.scrollWidth === pane.clientWidth)).toBe(true)

  await retry.click()
  await expect(footer.getByText("Folder sync enabled", { exact: true })).toBeVisible()
  await expect(settings.getByRole("alert")).toHaveCount(0)
  await expect(retry).toHaveCount(0)
  await expect(footer.getByRole("button", { name: "Sync now", exact: true })).toBeEnabled()
  await expect(pause).toBeChecked()
  await expect(saveCalls).toHaveAttribute("data-personal-save-calls", "2")
  await expect(saveCalls).toHaveAttribute("data-sync-calls", "1")
})

for (const width of [375, 768, 1440]) {
  test(`Cloud Settings reserves loading rows at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 })
    await page.emulateMedia({ reducedMotion: "reduce" })
    await page.goto(
      "/tests/visual/?surface=cloud&cloud=settings-progress&deferAccount=1&deferStatus=1",
    )
    const settings = page.getByRole("main", { name: "Cloud Settings preview" })
    const status = settings.getByRole("status")
    await expect(status).toHaveText("Loading Cloud settings…")
    await expect(settings.locator("[aria-busy=true]")).toHaveCount(1)
    await expect(settings.locator("[data-placeholder]").first()).toBeVisible()
    await expect(settings.getByRole("switch")).toHaveCount(0)
    await expect(
      settings.getByText(/Not signed in|have not started|Could not read/),
    ).toHaveCount(0)
    await expect(settings.getByRole("button", { name: "Sync now" })).toBeDisabled()
    await capture(page, `account-${width}`, "cloud-settings-loading")

    await page.evaluate(() => window.dispatchEvent(new Event("cloud-fixture:finish-account")))
    await expect(settings.getByText("Signed in as example")).toBeVisible()
    await expect(status).toHaveText("Loading Cloud settings…")
    await expect(settings.getByRole("switch")).toHaveCount(0)
    await expect(settings.getByText(/Status unavailable|Uploads paused/)).toHaveCount(0)
    await capture(page, `status-${width}`, "cloud-settings-loading")

    await page.evaluate(() => window.dispatchEvent(new Event("cloud-fixture:finish-status")))
    await expect(settings.locator("[aria-busy=true]")).toHaveCount(0)
    await expect(settings.locator("[data-placeholder]")).toHaveCount(0)
    await expect(settings.getByRole("switch", { name: /Sync Studio/ })).toBeVisible()
    await expect(status).not.toHaveText(/Loading|\?/)
    expect(await settings.evaluate((node) => node.scrollWidth === node.clientWidth)).toBe(true)
    await capture(page, `loaded-${width}`, "cloud-settings-loading")
  })
}

test("folder discovery retries and saved verification stay automatic in Sessions settings", async ({
  page,
}) => {
  await page.setViewportSize({ width: 480, height: 780 })
  await page.goto(
    "/tests/visual/?surface=cloud&cloud=settings-progress&theme=light&folderDiscovery=2&deferFolders=1&savedRecovery=1",
  )
  await expect(page.locator("body")).toHaveAttribute("data-folder-read-calls", "3")
  await expect(page.getByText("Loading folders…")).toBeVisible()
  await page.evaluate(() => window.dispatchEvent(new Event("cloud-fixture:finish-folders")))
  await expect(page.getByRole("switch", { name: /Sync Studio/ })).toBeChecked()
  await expect(page.locator("footer").getByRole("status")).toHaveText("3 saved uploads pending")
  await expect(
    page.locator("footer").getByRole("button", { name: "Syncing", exact: true }),
  ).toBeDisabled()
  await expect(page.getByRole("alert")).toHaveCount(0)
  await expect(
    page.getByRole("button", { name: /Retry folders|Retry folder list|Retry saved uploads/ }),
  ).toHaveCount(0)
  await expect(page.getByRole("progressbar")).toHaveCount(0)
})

test("Settings opens browser setup and refreshes a newly eligible team without selecting it", async ({
  page,
}) => {
  await page.setViewportSize({ width: 375, height: 780 })
  await page.goto(
    "/tests/visual/?surface=cloud&cloud=settings-folders&joinedTeam=1&browserSetup",
  )
  await expect(page.getByText("1 workspace", { exact: true })).toBeVisible({ timeout: 15_000 })
  const setup = page.getByRole("button", { name: "Finish setup in browser" })
  await setup.focus()
  await page.keyboard.press("Enter")
  await expect(page.locator("body")).toHaveAttribute("data-browser-setup-opened", "true")
  await capture(page, "settings-browser-setup")
  const refresh = page.getByRole("button", { name: "Refresh workspaces", exact: true })
  await refresh.focus()
  await page.evaluate(() => {
    document.body.dataset.joinedTeam = "true"
  })
  await page.keyboard.press("Enter")
  await expect(page.getByText("2 workspaces", { exact: true })).toBeVisible()
  await expect(refresh).toBeFocused()
  await expect(page.locator('option[value="joined-team"]').first()).toHaveJSProperty(
    "selected",
    false,
  )
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(375)
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
  ).not.toBeChecked()
  await expect(page.getByRole("checkbox")).toHaveCount(0)
  await expect(page.getByRole("button", { name: "Update sync selection" })).toHaveCount(0)
})

test("background and full sync use the same footer and cannot overlap from Settings", async ({
  page,
}) => {
  await page.setViewportSize({ width: 480, height: 620 })
  await page.emulateMedia({ reducedMotion: "reduce" })
  await page.goto(
    "/tests/visual/?surface=cloud&cloud=settings-progress&rollingProgress=1&backgroundProgress=1",
  )
  const footer = page.locator("footer")
  const progress = footer.getByRole("status")
  const action = footer.getByRole("button", { name: /^(Sync now|Syncing)$/ })
  await expect(progress).toHaveText("Syncing 2 of 5 sessions found (12 MB transferred)")
  await expect(page.getByText("5 sessions found", { exact: true })).toBeVisible()
  await expect(action).toHaveText("Syncing")
  await expect(action).toBeDisabled()
  await action.dispatchEvent("click")
  await expect(page.locator("body")).not.toHaveAttribute("data-sync-calls")
  await capture(page, "background-sync", "cloud-sync-coordination")

  await page.evaluate(() => window.dispatchEvent(new Event("cloud-fixture:finish-transfer")))
  await expect(action).toBeEnabled()
  await expect(action).toHaveText("Sync now")
  await action.click()
  await expect(action).toHaveText("Syncing")
  await expect(action).toBeDisabled()
  await expect(page.locator("body")).toHaveAttribute("data-sync-calls", "1")
  await expect(progress).toHaveText("Syncing 1 of 5 sessions found (12 MB transferred)")
  await capture(page, "full-sync", "cloud-sync-coordination")
  await page.evaluate(() => window.dispatchEvent(new Event("cloud-fixture:finish-sync")))
  await expect(progress).toHaveText("Syncing 1 of 5 sessions (12 MB transferred)")
  await expect(action).toBeDisabled()
  await page.evaluate(() => window.dispatchEvent(new Event("cloud-fixture:finish-transfer")))
  await expect(action).toBeEnabled()
})

test("Cloud Settings loads all folder pages without approving new folders", async ({
  page,
}) => {
  await page.setViewportSize({ width: 480, height: 620 })
  await page.goto("/tests/visual/?surface=cloud&cloud=settings-folders&pagedSettingsFolders=1")
  const folders = page.getByRole("group", { name: "Cloud folders" })
  await expect(folders.getByRole("switch", { name: /Sync Studio:/ })).toBeChecked()
  await expect(folders.getByRole("switch", { name: /Sync Website:/ })).not.toBeChecked()
  await expect(folders.getByRole("button", { name: "Load more folders" })).toHaveCount(0)
  await expect(page.locator("body")).toHaveAttribute("data-folder-read-calls", "2")
  await expect(page.locator("body")).not.toHaveAttribute("data-sync-calls")
})

for (const moveFocus of [false, true]) {
  test(`background grouping ${moveFocus ? "keeps focus outside the list" : "focuses the grouped saved switch"}`, async ({
    page,
  }) => {
    await page.goto(
      "/tests/visual/?surface=cloud&cloud=settings-progress&theme=light&deferFolders=1&savedFolderFocus=1",
    )
    const exact = page.getByRole("switch", {
      name: "Sync /Example/worktrees/studio-menu-fix to Personal",
    })
    const manageSaved = page.getByText("Manage other saved folders", { exact: true })
    await expect(manageSaved).toBeVisible()
    await expect(exact).toHaveCount(0)
    await manageSaved.click()
    await exact.focus()
    await expect(exact).toBeFocused()
    const other = page.getByRole("button", { name: "Sign out" })
    if (moveFocus) await other.focus()
    await page.evaluate(() => window.dispatchEvent(new Event("cloud-fixture:finish-folders")))
    const grouped = page.getByRole("switch", { name: "Sync Studio: /Example/Studio" })
    await expect(grouped).toBeChecked()
    await expect(exact).toHaveCount(0)
    await expect(moveFocus ? other : grouped).toBeFocused()
  })
}

test("moves focus to saved folders when discovery hides the focused grouped row", async ({
  page,
}) => {
  await page.goto(
    "/tests/visual/?surface=cloud&cloud=settings-progress&deferFolders=1&savedFolderFocus=1&reverseFolderFocus=1",
  )
  const grouped = page.getByRole("switch", { name: "Sync Studio: /Example/Studio" })
  const finishRead = () =>
    page.evaluate(() => window.dispatchEvent(new Event("cloud-fixture:finish-folders")))
  await expect(page.getByText("Manage other saved folders")).toBeVisible()
  await finishRead()
  await expect(grouped).toBeChecked()
  await grouped.focus()
  await expect(grouped).toBeFocused()
  await page.evaluate(() => window.dispatchEvent(new Event("cloud-fixture:recheck-folders")))
  await expect(page.locator("body")).toHaveAttribute("data-folder-read-calls", "2")
  await expect(
    page.getByRole("group", { name: "Cloud folders" }).getByRole("status"),
  ).toHaveCount(0)
  await expect(grouped).toBeFocused()
  await finishRead()
  await expect(grouped).toHaveCount(0)
  await expect(page.getByText("Manage other saved folders")).toBeFocused()
})

for (const width of [860, 480]) {
  test(`Cloud sync status does not move focused folders or Sync now at ${width}px`, async ({
    page,
  }) => {
    await page.setViewportSize({ width, height: 780 })
    await page.emulateMedia({ reducedMotion: "reduce" })
    await page.goto("/tests/visual/?surface=cloud&cloud=settings-progress&theme=light")
    const control = page.getByRole("switch", { name: "Sync Studio: /Example/Studio" })
    await expect(control).toBeChecked()
    await control.focus()
    const row = page.locator('[data-cloud-folder="Studio"]')
    const save = page.getByRole("button", { name: /^(Sync now|Syncing)$/ })
    const bounds = async () =>
      Promise.all([control.boundingBox(), row.boundingBox(), save.boundingBox()])
    const before = await bounds()
    await page.evaluate(() => window.dispatchEvent(new Event("cloud-fixture:update-queue")))
    await expect(page.locator("footer").getByRole("status")).toHaveText(
      "Syncing 6 of 96 sessions (1 MB transferred)",
    )
    await expect(page.getByText("3 queued")).toHaveCount(0)
    await expect(control).toBeFocused()
    await expect(control).toBeDisabled()
    await expect(control).toBeChecked()
    const after = await bounds()
    for (const [index, box] of before.entries()) {
      expect(box).not.toBeNull()
      for (const metric of ["x", "y", "width", "height"] as const) {
        expect(Math.abs(after[index]![metric] - box![metric])).toBeLessThanOrEqual(1)
      }
    }
    await capture(page, `cloud-settings-sync-status-${width}`, "cloud-sync-status")
    await page.evaluate(() => window.dispatchEvent(new Event("cloud-fixture:update-queue")))
    await expect(
      page.getByText("Syncing 6 of 96 sessions (1 MB transferred)", {
        exact: true,
      }),
    ).toHaveCount(0)
    await expect(control).toBeFocused()
    await expect(control).toBeEnabled()
  })
}

for (const theme of ["light", "dark"] as const) {
  for (const width of [375, 768, 1440]) {
    test(`Cloud scan progress keeps counters through recovery in ${theme} at ${width}px`, async ({
      page,
    }) => {
      const external: string[] = []
      const errors: string[] = []
      page.on("request", (request) => {
        const url = new URL(request.url())
        if (["https:", "http:"].includes(url.protocol) && url.hostname !== "127.0.0.1")
          external.push(url.href)
      })
      page.on("pageerror", (error) => errors.push(error.message))
      await page.setViewportSize({ width, height: 900 })
      await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" })
      await page.goto(
        `/tests/visual/?surface=cloud&cloud=settings-progress&rollingProgress=1&theme=${theme}`,
      )
      const settings = page.getByRole("main", { name: "Cloud Settings preview" })
      const footer = settings.locator("footer")
      const status = footer.getByRole("status")
      const sync = footer.getByRole("button", { name: /^(Sync now|Syncing)$/ })
      await expect(sync).toHaveText("Sync now")
      await expect(sync).toBeEnabled()
      await sync.focus()
      const initialSize = await sync.evaluate((button) => ({
        width: button.getBoundingClientRect().width,
        height: button.getBoundingClientRect().height,
      }))
      await sync.press("Enter")
      const busy = async (message: string, phase: string) => {
        await expect(status).toHaveText(message)
        await expect(footer.getByRole("button")).toHaveCount(1)
        await expect(sync).toHaveAccessibleName("Syncing")
        await expect(sync).toHaveText("Syncing")
        await expect(sync).toBeDisabled()
        await expect(sync).not.toBeFocused()
        await expect(sync).toHaveAttribute("aria-busy", "true")
        await expect(sync).toHaveCSS("opacity", "1")
        const label = sync.locator("span:not([aria-hidden])")
        await expect(label).toHaveCount(1)
        await expect(label).toHaveText("Syncing")
        await expect(label).toBeVisible()
        await expect(label).toHaveCSS("opacity", "1")
        await expect(sync.locator('span[aria-hidden="true"] svg')).toHaveCount(1)
        await expect(sync.locator("svg")).toBeVisible()
        const size = await sync.evaluate((button) => ({
          width: button.getBoundingClientRect().width,
          height: button.getBoundingClientRect().height,
        }))
        for (const metric of ["width", "height"] as const)
          expect(Math.abs(size[metric] - initialSize[metric])).toBeLessThanOrEqual(1)
        await footer.scrollIntoViewIfNeeded()
        await expect(status).toBeInViewport({ ratio: 1 })
        expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(width)
        expect(await status.evaluate((line) => line.scrollWidth <= line.clientWidth)).toBe(true)
        await expect(status.locator("span[title]")).toHaveCSS("white-space", "normal")
        await capture(page, `${theme}-${width}-${phase}`, "cloud-sync-status")
      }
      await busy("Syncing 1 of 5 sessions found (12 MB transferred)", "checking-folders")
      await sync.dispatchEvent("click")
      await expect(page.locator("body")).toHaveAttribute("data-sync-calls", "1")
      await page.evaluate(() => window.dispatchEvent(new Event("cloud-fixture:finish-sync")))
      await busy("Syncing 1 of 5 sessions (12 MB transferred)", "upload")
      await page.evaluate(() =>
        window.dispatchEvent(new Event("cloud-fixture:recover-transfer")),
      )
      await busy("Syncing 1 of 5 sessions (12 MB transferred)", "recovery")
      await page.evaluate(() =>
        window.dispatchEvent(new Event("cloud-fixture:update-transfer")),
      )
      await busy("Syncing 2 of 5 sessions (17 MB transferred)", "next-upload")
      await expect(status).not.toContainText("2 MB of 5 MB")
      await page.evaluate(() =>
        window.dispatchEvent(new Event("cloud-fixture:finish-transfer")),
      )
      await expect(status).toHaveText(
        "Last full scan • 5 of 5 sessions uploaded • 85 MB of 85 MB transferred",
      )
      await expect(sync).toHaveText("Sync now")
      await expect(sync).toBeEnabled()
      await expect(sync).not.toBeFocused()
      await sync.focus()
      await expect(sync).toBeFocused()
      await expect(sync).not.toHaveAttribute("aria-busy", "true")
      await expect(sync.locator("svg")).toHaveCount(0)
      await capture(page, `${theme}-${width}-complete`, "cloud-sync-status")
      await page.evaluate(() => window.dispatchEvent(new Event("cloud-fixture:fail-transfer")))
      await expect(settings.getByRole("alert")).toHaveText(
        "Cloud is offline. Saved uploads are kept.",
      )
      await expect(status).toHaveText("Folder sync needs attention")
      await expect(status).not.toContainText("sessions")
      await expect(status).not.toContainText("uploaded")
      await footer.scrollIntoViewIfNeeded()
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(width)
      await capture(page, `${theme}-${width}-error`, "cloud-sync-status")
      expect(errors).toEqual([])
      expect(external).toEqual([])
    })
  }
}

for (const [size, width, scale] of [
  ["default", 760, 100],
  ["narrow", 480, 100],
  ["zoom-200", 640, 200],
] as const) {
  test(`Cloud folder footer stays compact and shows recovery at ${size}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 780 })
    await page.emulateMedia({ reducedMotion: "reduce" })
    const open = async (failure: boolean) => {
      await page.goto(
        `/tests/visual/?surface=cloud&cloud=settings-folders&theme=light&scale=${scale}&footerPreview=1${failure ? "&footerFailure=1" : ""}`,
      )
      if (scale !== 100)
        await page.locator(".fixture-scale-root").evaluate((el: HTMLElement) => {
          el.style.zoom = "2"
        })
      await expect(page.getByRole("switch", { name: /Sync Studio:/ })).toBeChecked()
    }
    const save = page.getByRole("button", { name: "Sync now", exact: true })
    const retry = page.getByRole("button", { name: "Retry saved uploads" })
    const footer = page.locator("footer").filter({ has: save })
    await open(false)
    await expect(save).toBeEnabled()
    await expect(retry).toHaveCount(0)
    await save.scrollIntoViewIfNeeded()
    const relativeSave = () =>
      save.evaluate((button) => {
        const { x, y } = button.getBoundingClientRect()
        const footer = button.closest("footer")!.getBoundingClientRect()
        return { x: x - footer.x, y: y - footer.y }
      })
    const normal = await relativeSave()
    await expect(footer.getByRole("button")).toHaveCount(1)
    await expect(footer.getByRole("status")).toHaveText("Folder sync enabled")
    await expect(footer.getByText("Personal", { exact: true })).toHaveCount(0)
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(width)
    await capture(page, `cloud-settings-normal-${size}`)

    await open(true)
    await save.scrollIntoViewIfNeeded()
    await retry.scrollIntoViewIfNeeded()
    await expect(retry).toBeInViewport({ ratio: 1 })
    await expectWarningAboveFooter(page.getByRole("alert"), footer)
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(width)
    expect(await relativeSave()).toEqual(normal)
    await expect(page.getByText("Some sessions are not uploading.")).toHaveCount(0)
    await expect(page.getByText("Syncing sessions", { exact: false })).toHaveCount(0)
    await capture(page, `cloud-settings-error-${size}`)
  })

  test(`Cloud folder sync action stays put with choices and errors at ${size} width`, async ({
    page,
  }) => {
    await page.setViewportSize({ width, height: 780 })
    await page.emulateMedia({ colorScheme: "light", reducedMotion: "reduce" })
    await page.goto(
      `/tests/visual/?surface=cloud&cloud=settings-team-confirmation&scale=${scale}&saveFailure=1`,
    )
    if (scale !== 100)
      await page.locator(".fixture-scale-root").evaluate((el: HTMLElement, zoom) => {
        el.style.zoom = String(zoom / 100)
      }, scale)
    const save = page.getByRole("button", { name: "Sync now", exact: true })
    const footer = page.getByRole("main", { name: "Cloud Settings preview" }).locator("footer")
    const action = footer.getByRole("button")
    const website = page.getByRole("combobox", { name: /Sync Website/ })
    const team =
      "Team workspace: Example Team for Design, Documentation, and Developer Experience"
    await expect(save).toBeEnabled()
    await expect(save).toHaveCSS("transition-property", "opacity, background-color")
    expect(
      await save.evaluate((el) => parseFloat(getComputedStyle(el).transitionDuration)),
    ).toBeLessThan(0.001)
    await expect(page.getByText("Sync now approves the listed Team folders.")).toBeVisible()
    await expect(page.getByText("Uploads need review")).toHaveCount(0)
    await expect(page.getByRole("button", { name: /Retry/ })).toHaveCount(0)
    const geometry = (locator: typeof save) =>
      locator.evaluate((el) => {
        const { left, top, width, height } = el.getBoundingClientRect()
        return { x: left + window.scrollX, y: top + window.scrollY, width, height }
      })
    await website.scrollIntoViewIfNeeded()
    const row = page.locator("[data-cloud-folder]").filter({ has: website })
    const rowBefore = await geometry(row)
    await website.selectOption("synthetic-team")
    const destination = page.locator("span").getByText(team, { exact: true })
    await expect(destination).toHaveCount(1)
    await expect(destination).toBeVisible()
    expect(await destination.evaluate((el) => el.scrollWidth <= el.clientWidth)).toBe(true)
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(width)
    // The picker and confirmation can wrap outside the footer.
    await expect(save).toBeEnabled()
    await expect(
      page.getByText("Sync now approves the listed and new Team folders."),
    ).toBeVisible()
    const relativeGeometry = async () => {
      const bounds = await geometry(footer)
      const button = await geometry(action)
      return { height: bounds.height, x: button.x - bounds.x, y: button.y - bounds.y }
    }
    const draftGeometry = await relativeGeometry()
    await website.selectOption("")
    await expect(save).toBeEnabled()
    const listedGeometry = await relativeGeometry()
    expect({ x: listedGeometry.x, y: listedGeometry.y }).toEqual({
      x: draftGeometry.x,
      y: draftGeometry.y,
    })
    expect((await geometry(row)).height).toBe(rowBefore.height)
    await website.selectOption("synthetic-team")
    await save.scrollIntoViewIfNeeded()
    const beforeError = await relativeGeometry()
    await capture(page, `team-confirmation-${size}`)
    await save.focus()
    await page.keyboard.press("Enter")
    await expect(page.getByRole("alert")).toHaveText("Could not save folders. Try again.")
    await expect(page.getByRole("alert")).toBeInViewport({ ratio: 1 })
    const retry = footer.getByRole("button", { name: "Try again", exact: true })
    await expectWarningAboveFooter(page.getByRole("alert"), footer)
    await expect(retry).toHaveAccessibleDescription("Could not save folders. Try again.")
    await expect(action).toHaveCount(1)
    await expect(retry).toBeEnabled()
    await expect(retry).toBeFocused()
    const afterError = await relativeGeometry()
    for (const metric of ["x", "y"] as const)
      expect(Math.abs(afterError[metric] - beforeError[metric])).toBeLessThanOrEqual(1)
    await capture(page, `team-save-error-${size}`)
    await page.keyboard.press("Enter")
    await expect(save).toBeEnabled()
    const savedWebsite = page.getByRole("combobox", { name: /Sync Website/ })
    await expect(save).toBeFocused()
    await expect(savedWebsite).toHaveValue("synthetic-team")
    await expect(page.getByRole("switch", { name: /Upload sessions/ })).toBeChecked()
    await savedWebsite.scrollIntoViewIfNeeded()
    await capture(page, `team-confirmed-${size}`)
  })
}

test("a saved folder stays editable when switching between Team and Personal", async ({
  page,
}) => {
  await page.setViewportSize({ width: 480, height: 780 })
  await page.goto("/tests/visual/?surface=cloud&cloud=settings-team-confirmation&theme=dark")
  const save = page.getByRole("button", { name: "Sync now", exact: true })
  const folder = page.getByRole("combobox", { name: /Sync Website/ })
  await folder.selectOption("synthetic-team")
  await save.click()
  await expect(save).toBeEnabled()
  await expect(folder).toHaveValue("synthetic-team")
  await folder.selectOption("synthetic-personal")
  await expect(page.getByText("Previous uploads stay in the old workspace.")).toBeVisible()
  await expect(save).toBeEnabled()
  await save.focus()
  await page.keyboard.press("Enter")
  await expect(save).toBeEnabled()
  await expect(save).toBeFocused()
  await expect(folder).toHaveValue("synthetic-personal")
  await expect(page.getByRole("alert")).toHaveCount(0)
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(480)
  await capture(page, "cloud-settings-workspace-move-narrow-dark")
})

test("a deferred folder save failure returns keyboard focus to retry", async ({ page }) => {
  await page.setViewportSize({ width: 480, height: 320 })
  await page.emulateMedia({ reducedMotion: "reduce" })
  await page.goto(
    "/tests/visual/?surface=cloud&cloud=settings-team-confirmation&saveFailure=1&deferSaveFailure=1",
  )
  const settings = page.getByRole("main", { name: "Cloud Settings preview" })
  const footer = settings.locator("footer")
  const sync = footer.getByRole("button", { name: /^(Sync now|Syncing|Try again)$/ })
  await expect(sync).toBeEnabled()
  await sync.focus()
  await sync.press("Enter")
  await expect(sync).toHaveText("Syncing")
  await expect(sync).toBeDisabled()
  await expect(sync).not.toBeFocused()
  await page.evaluate(() => window.dispatchEvent(new Event("cloud-fixture:finish-save")))
  const warning = settings.getByRole("alert")
  await expect(warning).toHaveText("Could not save folders. Try again.")
  await expect(sync).toHaveText("Try again")
  await expect(sync).toBeEnabled()
  await expect(sync).toBeFocused()
  await expect(sync).toBeInViewport({ ratio: 1 })
  await expect(warning).toBeInViewport({ ratio: 1 })
  await capture(page, "save-error-focus", "cloud-settings-loading")
})

test("a failed folder save does not scroll away from another focused setting", async ({
  page,
}) => {
  await page.setViewportSize({ width: 480, height: 320 })
  await page.emulateMedia({ reducedMotion: "reduce" })
  await page.goto(
    "/tests/visual/?surface=cloud&cloud=settings-team-confirmation&saveFailure=1&deferSaveFailure=1",
  )
  const save = page.getByRole("button", { name: "Sync now", exact: true })
  const settings = page.getByRole("main", { name: "Cloud Settings preview" })
  const foldersRow = settings.getByRole("group", { name: "Cloud folders" })
  const footer = settings.locator("footer")
  const folder = foldersRow.getByRole("combobox", { name: /Sync Website/ })
  await expect(folder).toBeEnabled()
  await expect(save).toBeEnabled()
  await save.focus()
  await page.keyboard.press("Enter")
  const syncing = footer.getByRole("button", { name: "Syncing", exact: true })
  await expect(syncing).toBeDisabled()
  await expect(syncing).not.toBeFocused()
  await expect(folder).toBeDisabled()
  await expect(footer.getByRole("status")).toHaveText("Saving folder choices")
  await expect(footer.getByRole("status").locator('span[aria-hidden="true"] svg')).toBeVisible()
  await expect(footer.getByText("Folder sync enabled")).toHaveCount(0)
  await settings.evaluate((pane) => {
    pane.scrollTop = 0
  })
  await foldersRow.focus()
  await expect(foldersRow).toBeFocused()
  const scrollBefore = await settings.evaluate((pane) => pane.scrollTop)
  expect(scrollBefore).toBeLessThan(
    await settings.evaluate((pane) => pane.scrollHeight - pane.clientHeight),
  )

  await page.evaluate(() => window.dispatchEvent(new Event("cloud-fixture:finish-save")))
  await expect(page.getByRole("alert")).toHaveText("Could not save folders. Try again.")
  await expect(page.getByRole("button", { name: "Try again", exact: true })).toBeEnabled()
  await page.evaluate(
    () => new Promise<void>((resolve) => requestAnimationFrame(() => resolve())),
  )
  await expect(foldersRow).toBeFocused()
  expect(await settings.evaluate((pane) => pane.scrollTop)).toBe(scrollBefore)
})

for (const [size, width, zoom] of [
  ["default", 760, 1],
  ["narrow", 480, 1],
  ["zoom-200", 640, 2],
] as const) {
  test(`Cloud folder loading and background retries keep Sync now in place at ${size}`, async ({
    page,
  }) => {
    await page.setViewportSize({ width, height: 320 })
    await page.emulateMedia({ reducedMotion: "reduce" })
    await page.goto(
      "/tests/visual/?surface=cloud&cloud=settings-folders&deferFolders=1&refreshFailures=3",
    )
    if (zoom !== 1)
      await page.locator(".fixture-scale-root").evaluate((el: HTMLElement, value) => {
        el.style.zoom = String(value)
      }, zoom)
    const settings = page.getByRole("main", { name: "Cloud Settings preview" })
    const folders = page.getByRole("group", { name: "Cloud folders" })
    const save = page.getByRole("button", { name: "Sync now", exact: true })
    const folderSwitch = folders.getByRole("switch", { name: /Sync Studio/ })
    const layout = async () => ({
      folders: await folders.boundingBox(),
      save: await save.boundingBox(),
      scroll: await settings.evaluate((pane) => pane.scrollTop),
    })
    const finishRead = () =>
      page.evaluate(() => window.dispatchEvent(new Event("cloud-fixture:finish-folders")))
    await expect(folders.getByRole("status")).toHaveText("Loading folders…")
    await expect(folders.getByRole("switch")).toHaveCount(0)
    await expect(folders.getByText(/\d+ sessions?/)).toHaveCount(0)
    await save.scrollIntoViewIfNeeded()
    const cold = await layout()
    await finishRead()
    await expect(folderSwitch).toBeChecked()
    await expect(folders.getByRole("status")).toHaveCount(0)
    expect(await layout()).toEqual(cold)
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(width)

    await folderSwitch.focus()
    const warm = await layout()
    await page.evaluate(() => window.dispatchEvent(new Event("cloud-fixture:recheck-folders")))
    await expect(page.locator("body")).toHaveAttribute("data-folder-read-calls", "5")
    await expect(folders.getByRole("status")).toHaveCount(0)
    await expect(settings.getByText("Still checking session folders.")).toHaveCount(0)
    await expect(settings.getByRole("alert")).toHaveCount(0)
    await expect(save).toBeEnabled()
    await expect(folderSwitch).toBeChecked()
    await expect(folderSwitch).toBeFocused()
    expect(await layout()).toEqual(warm)
    await finishRead()
    await expect(folders.getByRole("status")).toHaveCount(0)
    await expect(folderSwitch).toBeFocused()
    expect(await layout()).toEqual(warm)
  })
}

for (const zoom of [1, 2]) {
  test(`Cloud Settings scrolls full rows and recovery together in a short window at ${zoom * 100}%`, async ({
    page,
  }) => {
    await page.setViewportSize({ width: 672, height: 320 })
    await page.emulateMedia({ reducedMotion: "reduce" })
    await page.goto("/tests/visual/?surface=cloud&cloud=settings-attention&theme=dark")
    if (zoom !== 1) {
      await page.locator(".fixture-scale-root").evaluate((node: HTMLElement, scale) => {
        node.style.zoom = String(scale)
      }, zoom)
    }
    const rows = page.locator("[data-cloud-folder]")
    await expect(rows).toHaveCount(7)
    const last = rows.last()
    const footer = page.locator("footer").filter({
      has: page.getByRole("button", { name: /^(Sync now|Syncing)$/ }),
    })
    expect(
      await last.evaluate(
        (row) => row.parentElement!.scrollHeight - row.parentElement!.clientHeight,
      ),
    ).toBeLessThanOrEqual(1)
    await last.scrollIntoViewIfNeeded()
    await expect(last).toBeInViewport({ ratio: 0.98 })
    await expect(last.getByRole("switch")).toBeInViewport({ ratio: 0.98 })
    expect(
      (await last.boundingBox())!.y + (await last.boundingBox())!.height,
    ).toBeLessThanOrEqual((await footer.boundingBox())!.y)
    const sync = footer.getByRole("button", { name: /^(Sync now|Syncing)$/ })
    await footer.scrollIntoViewIfNeeded()
    await expect(sync).toBeInViewport({ ratio: 0.98 })
    expect(await sync.locator("svg").count()).toBe(0)
    await expectWarningAboveFooter(page.getByRole("alert"), footer)
    await expect(page.getByRole("button", { name: "Retry saved uploads" })).toBeVisible()
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(672)
    await capture(page, `cloud-settings-short-${zoom * 100}`, "cloud-sync-status")
    await sync.focus()
    await sync.press("Enter")
    await expect(sync).toHaveText("Syncing")
    await expect(sync).toBeDisabled()
    await expect(sync).not.toBeFocused()
    const status = footer.getByRole("status")
    await expect(status).toHaveText("Folder sync needs attention")
    const text = status.getByText("Folder sync needs attention")
    await expect(text).toHaveCSS("white-space", "normal")
    await expect(text).toHaveAttribute("title", "Folder sync needs attention")
    expect(await status.evaluate((line) => line.scrollWidth <= line.clientWidth)).toBe(true)
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(672)
  })
}

test("Settings switches save exclusions and require the primary button for additions", async ({
  page,
}) => {
  await page.goto("/tests/visual/?surface=cloud&cloud=settings-folders&theme=light")
  const studio = page.getByRole("switch", { name: /Sync Studio:/ })
  const sync = page.getByRole("button", { name: "Sync now", exact: true })
  const actions = page.locator("body")
  await expect(studio).toBeChecked()
  await expect(sync).toBeEnabled()
  await studio.click()
  await expect(studio).not.toBeChecked()
  await expect(sync).toBeEnabled()
  await expect(page.getByRole("dialog")).toHaveCount(0)
  await studio.click()
  await expect(studio).toBeChecked()
  await expect(sync).toBeEnabled()
  await expect(actions).not.toHaveAttribute("data-sync-calls")
  await sync.click()
  await expect(actions).toHaveAttribute("data-sync-calls", "1")
  await expect(sync).toBeEnabled()
  await expect(page.getByRole("dialog")).toHaveCount(0)
  await expect(
    page.getByRole("button", { name: /Update sync|Choose Claude|Choose OpenCode/ }),
  ).toHaveCount(0)
})

for (const theme of ["light", "dark"] as const) {
  test(`local tool setup settles live without claiming vendor readiness in ${theme}`, async ({
    page,
  }) => {
    await openCloud(page, "tools-live", theme)
    await page.setViewportSize({ width: 640, height: 620 })
    for (const [phase, title] of [
      ["pending", "Setting up local tools"],
      ["ready", "Local tools ready"],
      ["failed", "Local tools setup failed"],
    ] as const) {
      await page.evaluate((phase) => {
        window.dispatchEvent(new Event(`fixture:local-tools-${phase}`))
      }, phase)
      await expect(page.getByText(title, { exact: true })).toBeInViewport({ ratio: 1 })
      await expect(page.getByRole("button", { name: "Done", exact: true })).toBeInViewport({
        ratio: 1,
      })
      await expect(page.getByRole("button", { name: "Refresh folders" })).toHaveCount(0)
      await expect(page.getByText("Local integration is active", { exact: true })).toHaveCount(
        0,
      )
      await expect(page.getByText("Local agent ready", { exact: true })).toHaveCount(0)
      if (phase === "ready")
        await expect(
          page.getByText("CLI tested · Skill installed", { exact: true }),
        ).toBeInViewport({ ratio: 1 })
      if (phase === "failed")
        await expect(
          page.getByText("Try Repair in Settings → CLI.", { exact: true }),
        ).toBeInViewport({ ratio: 1 })
      await capture(page, `${theme}-local-tools-${phase}`)
    }
  })

  test(`incomplete empty discovery stays neutral with pinned actions in ${theme}`, async ({
    page,
  }) => {
    for (const state of ["folders-empty-partial", "folders-empty-paged"]) {
      await openCloud(page, state, theme)
      await expect(
        page.getByText("Folder discovery is incomplete.", { exact: true }),
      ).toBeVisible()
      await expect(page.getByText(/No project folders found/)).toHaveCount(0)
      await expect(page.getByRole("button", { name: "Refresh folders" })).toHaveCount(0)
      await expect(page.getByRole("button", { name: "Start syncing" })).toBeDisabled()
      await expect(page.getByRole("button", { name: "Continue locally" })).toBeInViewport()
      await expect(page.locator("footer p")).toHaveCount(0)
      await capture(page, `${theme}-${state}`)
      if (state === "folders-empty-paged") {
        const more = page.getByRole("button", { name: "Load more folders" })
        await more.focus()
        await more.press("Enter")
        await expect(page.getByRole("switch")).toHaveCount(2)
        await expect(page.getByRole("group", { name: "Cloud project folders" })).toBeFocused()
        await expect(page.getByRole("button", { name: "Start syncing" })).toBeEnabled()
      }
    }
  })

  test(`missing source access does not offer custom provider pickers in ${theme}`, async ({
    page,
  }) => {
    await page.setViewportSize({ width: 480, height: 620 })
    await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" })
    await page.goto(
      `/tests/visual/?surface=cloud&cloud=settings-missing-sources&theme=${theme}`,
    )
    await expect(page.getByText("~/dev/Studio", { exact: true })).toBeVisible()
    await expect(page.getByRole("button", { name: "Start syncing" })).toBeDisabled()
    await expect(
      page.getByRole("button", { name: "Choose Claude session folders…" }),
    ).toHaveCount(0)
    await expect(page.getByRole("button", { name: "Choose OpenCode databases…" })).toHaveCount(
      0,
    )
    await expect(page.getByRole("button", { name: "Sign out" })).toBeEnabled()
    await expect(page.getByRole("button", { name: "Refresh folders" })).toHaveCount(0)
    await expect(page.getByText("No session folders found.", { exact: true })).toHaveCount(0)
    await expect(page.locator("[id^='cloud-settings-folder-'] details")).toHaveCount(0)
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(480)
    await capture(page, `cloud-account-${theme}-source-access`)
  })

  test(`saved-work failures stay visible during other uploads in ${theme}`, async ({
    page,
  }) => {
    const width = theme === "dark" ? 375 : 760
    await page.setViewportSize({ width, height: 720 })
    await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" })
    await page.goto(`/tests/visual/?surface=cloud&cloud=settings-attention&theme=${theme}`)
    const failure = page
      .getByRole("alert")
      .filter({ hasText: "Some saved uploads are blocked." })
    await expect(failure).toHaveCount(1)
    await failure.scrollIntoViewIfNeeded()
    await expect(failure).toBeInViewport()
    const retry = page.getByRole("button", { name: "Retry saved uploads", exact: true })
    await expect(retry).toBeInViewport()
    expect((await retry.boundingBox())?.height).toBeGreaterThanOrEqual(24)
    await retry.focus()
    await expect(retry).toBeFocused()
    await retry.press("Enter")
    await expect(failure).toBeVisible()
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(width)
    const footer = page.locator("footer")
    const status = footer.getByRole("status")
    const sync = footer.getByRole("button", { name: /^(Sync now|Syncing)$/ })
    await sync.focus()
    await expect(sync).toBeFocused()
    expect((await sync.boundingBox())?.height).toBeGreaterThanOrEqual(24)
    const idle = await sync.boundingBox()
    await sync.press("Enter")
    await expect(sync).toHaveText("Syncing")
    await expect(sync).toBeDisabled()
    await expect(sync).not.toBeFocused()
    await expect(sync).toHaveAttribute("aria-busy", "true")
    await expect(sync.locator('span[aria-hidden="true"] svg')).toBeVisible()
    await expect(status).toHaveText("Folder sync needs attention")
    await expectWarningAboveFooter(failure, footer)
    await expect(retry).toBeEnabled()
    const busyBounds = await sync.boundingBox()
    for (const metric of ["x", "y", "width", "height"] as const)
      expect(Math.abs(busyBounds![metric] - idle![metric])).toBeLessThanOrEqual(1)
    await expect(footer.getByText(/queued|automatically|accepted|sync enabled/)).toHaveCount(0)
    await expect(page.getByRole("progressbar")).toHaveCount(0)
    await expect(page.getByText(/uploaded ·|Transferred/)).toHaveCount(0)
    await capture(page, `cloud-account-${theme}-blocked`, "cloud-sync-status")
    await page.evaluate(() => window.dispatchEvent(new Event("cloud-fixture:finish-sync")))
    await expect(status).toHaveText("Folder sync needs attention")
    await expect(sync).toHaveText("Syncing")
    await expect(sync).toBeDisabled()
    await page.evaluate(() => window.dispatchEvent(new Event("cloud-fixture:update-transfer")))
    await expect(status).toHaveText("Folder sync needs attention")
    await expect(page.getByRole("progressbar")).toHaveCount(0)
    await capture(page, `cloud-account-${theme}-sync-progress`, "cloud-sync-status")
    await page.evaluate(() => window.dispatchEvent(new Event("cloud-fixture:finish-transfer")))
    await expect(sync).toBeEnabled()
    await expect(sync).toHaveText("Sync now")
    const completedBounds = await sync.boundingBox()
    for (const metric of ["x", "y", "width", "height"] as const)
      expect(Math.abs(completedBounds![metric] - idle![metric])).toBeLessThanOrEqual(1)
    await expect(status).toHaveText("Folder sync needs attention")
    await expectWarningAboveFooter(failure, footer)
    await expect(page.getByRole("progressbar")).toHaveCount(0)
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(width)
    await page.getByRole("button", { name: "Sign out", exact: true }).click()
    await expect(
      page.getByRole("button", { name: "Sign in with GitHub", exact: true }),
    ).toBeEnabled()
    await expect(page.locator("[id^='cloud-settings-folder-']")).toHaveCount(0)
  })

  for (const [size, width] of [
    ["full", 760],
    ["narrow", 480],
  ] as const) {
    test(`personal Cloud account stays compact in ${theme} at ${size} width`, async ({
      page,
    }) => {
      await page.setViewportSize({ width, height: 620 })
      await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" })
      await page.goto(`/tests/visual/?surface=cloud&cloud=settings-folders&theme=${theme}`)
      await expect(page.getByText("~/dev/Studio", { exact: true })).toBeVisible()
      await expect(page.getByRole("button", { name: "Sign out", exact: true })).toBeInViewport()
      await expect(
        page.getByRole("switch", { name: "Upload sessions to Antiburn Cloud", exact: true }),
      ).toBeChecked()
      await expect(page.getByRole("button", { name: "Update sync selection" })).toHaveCount(0)
      await expect(page.getByRole("switch", { name: /Sync Website:/ })).not.toBeChecked()
      await expect(
        page.getByRole("button", { name: /Refresh folders|Retry saved uploads/ }),
      ).toHaveCount(0)
      await expect(
        page.getByText(/Transferred|uploaded ·|Your account is verified|Session file access/),
      ).toHaveCount(0)
      await expect(page.locator("[id^='cloud-settings-folder-'] details")).toHaveCount(0)
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(width)
      await capture(page, `cloud-account-${theme}-${size}`)
    })
  }
}

for (const theme of ["light", "dark"] as const) {
  for (const [size, width, height] of [
    ["baseline", 860, 620],
    ["large", 1100, 760],
    ["narrow", 640, 620],
  ] as const) {
    test(`simple folder rows fill the window at ${size} in ${theme}`, async ({ page }) => {
      await openCloud(page, "folders-long-paths", theme)
      await page.setViewportSize({ width, height })
      await expect(page.getByRole("checkbox")).toHaveCount(0)
      await expect(page.getByRole("button", { name: "Start syncing" })).toBeInViewport()
      const rows = page.locator(".cloud-folder-row")
      expect((await rows.first().boundingBox())!.height).toBeLessThanOrEqual(74)
      const metadata = rows.first().locator("[id^='cloud-folder-']")
      await expect(metadata).toContainText("~/dev/Studio")
      await expect(rows.locator("details, summary, ul")).toHaveCount(0)
      await expect(page.getByText("8 folders selected", { exact: true })).toBeVisible()
      const name = rows.first().locator("strong")
      const path = metadata.locator("span[title]").first()
      expect((await path.boundingBox())!.x).toBe((await name.boundingBox())!.x)
      expect(
        await page
          .locator(".cloud-folder-list")
          .evaluate((element) => element.scrollHeight > element.clientHeight),
      ).toBe(true)
      await expect(page.locator("footer p")).toHaveCount(0)
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(width)
      await capture(page, `${theme}-${size}-folder-rows`)
    })
  }
}

test("unchecked upload support uses compact generic help, not an unsupported-provider claim", async ({
  page,
}) => {
  await openCloud(page, "folders-unchecked")
  await expect(page.getByRole("button", { name: "Start syncing" })).toBeDisabled()
  await expect(page.getByText("Cloud upload unavailable", { exact: true })).toHaveCount(0)
  await expect(
    page.getByText("Could not check uploads for some folders.", { exact: true }),
  ).toHaveCount(1)
  await expect(page.getByRole("button", { name: "Refresh folders" })).toHaveCount(0)
  await expect(page.locator("footer p")).toHaveCount(0)
  await capture(page, "light-unchecked-help")
})
