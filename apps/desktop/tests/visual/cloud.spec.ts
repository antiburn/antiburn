import { fileURLToPath } from "node:url"
import { expect, test, type Page } from "@playwright/test"

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

async function capture(page: Page, name: string) {
  await page.screenshot({
    path: fileURLToPath(
      new URL(
        `../../../../.agent-artifacts/reviews/desktop-cloud-simplified/${name}.png`,
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
  await page.getByRole("button", { name: "Connect in browser", exact: true }).click()
  await expect(page.getByRole("button", { name: "Cancel", exact: true })).toBeVisible()
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
    await page.getByRole("button", { name: "Connect in browser", exact: true }).click()
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
  await expect(
    page.getByText("Sync full sessions from selected folders and their worktrees."),
  ).toBeInViewport()
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
  const upload = page.getByRole("switch", { name: "Upload sessions", exact: true })
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
  await expect(page.getByText("Uploads paused", { exact: true })).toBeVisible()
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
  await expect(page.getByRole("button", { name: "Start syncing", exact: true })).toHaveCount(0)
})

test("unreadable upload storage does not show cached totals as current", async ({ page }) => {
  await page.goto("/tests/visual/?surface=cloud&cloud=settings-storage-error&theme=light")
  await expect(page.getByText("Could not read saved uploads.", { exact: true })).toBeVisible()
  await expect(page.getByText(/41 uploaded|0 uploaded|Transferred/)).toHaveCount(0)
  await expect(page.getByText("Background sync enabled", { exact: true })).toHaveCount(0)
  await expect(page.getByRole("button", { name: "Retry saved uploads" })).toBeEnabled()
  await expect(page.getByRole("button", { name: "Refresh folders" })).toHaveCount(0)
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

test("Settings switches save exclusions and confirm additions without an Update button", async ({
  page,
}) => {
  await page.goto("/tests/visual/?surface=cloud&cloud=settings-folders&theme=light")
  const studio = page.getByRole("switch", { name: /Sync Studio:/ })
  await expect(studio).toBeChecked()
  await studio.click()
  await expect(studio).not.toBeChecked()
  await expect(page.getByRole("dialog")).toHaveCount(0)
  await studio.click()
  await expect(page.getByRole("dialog")).toHaveAccessibleName("Upload sessions from Studio?")
  await page.getByRole("button", { name: "Cancel", exact: true }).click()
  await expect(studio).not.toBeChecked()
  await studio.click()
  await page.getByRole("button", { name: "Allow uploads", exact: true }).click()
  await expect(studio).toBeChecked()
  await expect(page.getByRole("dialog")).toHaveCount(0)
  await expect(
    page.getByRole("button", { name: /Update sync|Choose Claude|Choose OpenCode/ }),
  ).toHaveCount(0)
  const card = page
    .getByText("Signed in as example", { exact: true })
    .locator("..")
    .locator("..")
    .locator("..")
  expect(
    await card.evaluate((element) =>
      Array.from(element.children).map((child) => getComputedStyle(child).borderTopWidth),
    ),
  ).not.toContain("1px")
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
        await page.getByRole("button", { name: "Load more folders" }).click()
        await expect(page.getByRole("switch")).toHaveCount(2)
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
    await page.setViewportSize({ width: 480, height: 620 })
    await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" })
    await page.goto(`/tests/visual/?surface=cloud&cloud=settings-attention&theme=${theme}`)
    const failure = page.getByText("Could not resume saved uploads.", { exact: true })
    await expect(failure).toHaveCount(1)
    await expect(failure).toBeInViewport()
    const retry = page.getByRole("button", { name: "Retry saved uploads", exact: true })
    await expect(retry).toBeInViewport()
    await retry.click()
    await expect(failure).toBeVisible()
    await expect(page.getByText(/uploaded ·|Transferred/)).toHaveCount(0)
    await capture(page, `cloud-account-${theme}-blocked`)
    await page.getByRole("button", { name: "Sign out", exact: true }).click()
    await expect(page.getByRole("button", { name: "Sign in", exact: true })).toBeEnabled()
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
        page.getByRole("switch", { name: "Upload sessions", exact: true }),
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
  await expect(page.getByRole("button", { name: "Refresh folders" })).toBeEnabled()
  await expect(page.locator("footer p")).toHaveCount(0)
  await capture(page, "light-unchecked-help")
})
