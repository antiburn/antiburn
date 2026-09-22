import { expect, test, type Page, type TestInfo } from "@playwright/test"

const scales = [90, 100, 110, 125, 150, 175, 200] as const
const surfaces = [
  "main",
  "settings",
  "onboarding",
  "popover",
  "preview",
  "hud",
  "hud-detail",
  "nudge",
] as const
const states = ["populated", "empty", "loading", "error", "long"] as const
const layouts = [
  { name: "normal", viewport: { width: 1280, height: 860 } },
  { name: "constrained", viewport: { width: 640, height: 480 } },
] as const

type Surface = (typeof surfaces)[number]
type State = (typeof states)[number]
type FixtureFault = "session-analysis" | "scale-save" | "onboarding-bootstrap" | "peek-data"

function fixtureUrl(
  surface: Surface,
  options: {
    scale?: number
    theme?: "light" | "dark"
    state?: State
    fault?: FixtureFault
    platform?: "macos" | "windows" | "linux"
  } = {},
): string {
  const params = new URLSearchParams({
    surface,
    scale: String(options.scale ?? 100),
    theme: options.theme ?? "light",
    state: options.state ?? "populated",
  })
  if (options.fault) params.set("fault", options.fault)
  if (options.platform) params.set("platform", options.platform)
  return `/tests/visual/?${params}`
}

async function openFixture(
  page: Page,
  surface: Surface,
  options: {
    scale?: number
    theme?: "light" | "dark"
    state?: State
    fault?: FixtureFault
    platform?: "macos" | "windows" | "linux"
  } = {},
) {
  await page.clock.setFixedTime(new Date("2026-09-15T00:00:00.000Z"))
  await page.emulateMedia({ colorScheme: options.theme ?? "light" })
  await page.goto(fixtureUrl(surface, options))
  // The first navigation waits for the dev server to transform the surface
  // tree. Give it more time than an ordinary assertion gets.
  await expect(page.locator(".fixture-scale-root > *").first()).toBeVisible({
    timeout: 30_000,
  })
}

async function expectNoHorizontalOverflow(page: Page) {
  await expect
    .poll(() => page.locator("#root").evaluate((node) => node.scrollWidth - node.clientWidth))
    .toBeLessThanOrEqual(1)
}

async function expectControlsReachable(page: Page) {
  const offscreen = await page
    .locator("button, input, select, [role=tab]")
    .evaluateAll((nodes) => {
      const width = document.documentElement.clientWidth
      return nodes
        .map((node) => {
          const box = node.getBoundingClientRect()
          return { label: node.getAttribute("aria-label") ?? node.textContent?.trim(), box }
        })
        .filter(
          ({ box }) =>
            box.width > 0 && box.height > 0 && (box.left < -1 || box.right > width + 1),
        )
    })
  expect(offscreen).toEqual([])
}

async function selectNavigationItem(page: Page, surface: "main" | "settings", name: string) {
  const navigationLabel = surface === "main" ? "Main navigation" : "Settings navigation"
  const opener = page.getByRole("button", { name: `Open ${navigationLabel}` })
  if (await opener.isVisible()) await opener.click()
  await page.getByRole("tab", { name, exact: true }).click()
}

async function capture(page: Page, name: string, testInfo: TestInfo) {
  await page.evaluate(() => document.fonts.ready)
  await page.screenshot({
    path: testInfo.outputPath(`${name}.png`),
    fullPage: false,
    animations: "disabled",
  })
}

test.describe("horizontal overflow assertion", () => {
  test("retries until measured overflow clears", async ({ page }) => {
    await page.setContent('<div id="root" style="width: 200px"></div>')
    await page.locator("#root").evaluate((node) => {
      let measurements = 0
      Object.defineProperty(node, "scrollWidth", {
        get: () => (++measurements < 3 ? 400 : 200),
      })
    })
    await expectNoHorizontalOverflow(page)
  })

  test("rejects persistent overflow", async ({ page }) => {
    await page.setContent(
      '<div id="root" style="width: 200px"><div style="width: 400px">Overflow</div></div>',
    )
    await expect(expectNoHorizontalOverflow(page)).rejects.toThrow(/toBeLessThanOrEqual/)
  })
})

test.describe("interface-scale smoke", () => {
  test("the main navigation reaches Sessions and settings", async ({ page }, testInfo) => {
    await openFixture(page, "main")
    await expect(page.getByRole("main", { name: "antiburn main window" })).toBeVisible()
    await expect(
      page.getByRole("group", { name: "Allowance for the past 30 days" }),
    ).toBeVisible()
    await selectNavigationItem(page, "main", "Sessions")
    await expect(
      page
        .getByRole("tabpanel", { name: "Sessions", exact: true })
        .locator("[data-session-row]")
        .filter({ hasText: "Make every interface scale reachable" }),
    ).toBeVisible()
    await expectNoHorizontalOverflow(page)
    await expectControlsReachable(page)
    await capture(page, "main-populated-100-light", testInfo)
  })

  test("settings navigation exposes the interface scale control", async ({
    page,
  }, testInfo) => {
    await openFixture(page, "settings", { scale: 125, theme: "dark" })
    await selectNavigationItem(page, "settings", "Appearance")
    const control = page.getByRole("combobox", { name: "Interface size" })
    await expect(control).toBeVisible()
    await control.selectOption("150")
    await expect(control).toHaveValue("150")
    await page.getByRole("button", { name: "Reset to 100%" }).click()
    await expect(control).toHaveValue("100")
    await expectNoHorizontalOverflow(page)
    await expectControlsReachable(page)
    await capture(page, "settings-125-dark", testInfo)
  })

  test("popover and nudge respond to reader interactions", async ({ page }, testInfo) => {
    await openFixture(page, "popover", { scale: 150, state: "populated" })
    await expect(page.getByText("Codex", { exact: true }).first()).toBeVisible()
    await expectNoHorizontalOverflow(page)
    await expectControlsReachable(page)
    await capture(page, "popover-150-light", testInfo)

    await openFixture(page, "nudge", { scale: 150, theme: "dark" })
    const card = page.getByText("Codex 5-hour window is 72% used").locator("..").locator("..")
    await card.hover()
    await expect(page.getByRole("button", { name: "Open usage" })).toBeVisible()
    await capture(page, "nudge-expanded-150-dark", testInfo)
  })
})

test.describe("interface-scale integrated chrome", () => {
  test.use({ viewport: { width: 320, height: 240 }, reducedMotion: "reduce" })

  test("settings search target focuses interface size without changing it", async ({
    page,
  }, testInfo) => {
    await openFixture(page, "settings", { scale: 200, theme: "dark" })
    await expect(page.getByRole("tabpanel", { name: "General", exact: true })).toBeVisible()
    await page.waitForFunction(() => Boolean(window.__ANTIBURN_VISUAL_EMIT__))
    const requestTarget = () =>
      page.evaluate(() => {
        const emit = window.__ANTIBURN_VISUAL_EMIT__
        if (!emit) throw new Error("Fixture event bridge is unavailable")
        emit("settings:pane", "appearance#interfaceSize")
      })
    await requestTarget()
    const control = page.getByRole("combobox", { name: "Interface size" })
    await expect(control).toBeFocused()
    await expect(control).toBeInViewport()
    await expect(control).toHaveValue("200")
    await expectNoHorizontalOverflow(page)
    await expectControlsReachable(page)
    await capture(page, "settings-search-target-200", testInfo)
    await page.getByRole("button", { name: "Open Settings navigation" }).focus()
    await requestTarget()
    await expect(control).toBeFocused()
    await expect(control).toHaveValue("200")
    expect(await page.evaluate(() => window.__ANTIBURN_VISUAL_SETTINGS_CALLS__ ?? [])).toEqual(
      [],
    )
  })

  for (const theme of ["light", "dark"] as const) {
    for (const scale of [90, 200]) {
      test(`search at ${scale}% constrained in ${theme} restores focus and targets settings`, async ({
        page,
      }, testInfo) => {
        await page.setViewportSize({
          width: Math.round(640 / (scale / 100)),
          height: Math.round(480 / (scale / 100)),
        })
        await openFixture(page, "main", { scale, theme })
        await expect(page.locator("html")).toHaveAttribute("data-platform", "macos")
        await expect(page.getByRole("group", { name: "Window controls" })).toHaveCount(0)
        const trigger = page.getByRole("button", { name: "Search antiburn", exact: true })
        await trigger.click()
        const dialog = page.getByRole("dialog", { name: "Search antiburn" })
        const query = dialog.getByRole("combobox", { name: "Search antiburn" })
        await expect(query).toBeFocused()
        await query.fill("Interface size")
        const result = dialog.getByRole("option", { name: /Interface size/ })
        await expect(result).toBeVisible()
        await expectNoHorizontalOverflow(page)
        await expectControlsReachable(page)
        await capture(page, `search-${scale}-${theme}`, testInfo)
        await page.keyboard.press("Escape")
        await expect(dialog).toHaveCount(0)
        await expect(trigger).toBeFocused()

        await page.keyboard.press("Meta+k")
        await expect(query).toBeFocused()
        await query.fill("Interface size")
        await page.keyboard.press("Enter")
        await expect(dialog).toHaveCount(0)
        await expect
          .poll(() => page.evaluate(() => window.__ANTIBURN_VISUAL_SETTINGS_CALLS__))
          .toEqual([
            { command: "open_settings_window", args: { pane: "appearance#interfaceSize" } },
          ])
      })
    }

    test(`history and compact navigation remain reachable at 200% in ${theme}`, async ({
      page,
    }, testInfo) => {
      await openFixture(page, "main", { scale: 200, theme })
      const opener = page.getByRole("button", { name: "Open Main navigation" })
      await expect(opener).toBeVisible()
      await selectNavigationItem(page, "main", "Sessions")
      await expect(page.getByRole("dialog", { name: "Main navigation" })).toHaveCount(0)
      await expect(page.getByRole("tabpanel", { name: "Sessions", exact: true })).toBeVisible()
      await selectNavigationItem(page, "main", "Checks")
      await expect(page.getByRole("tabpanel", { name: "Checks", exact: true })).toBeVisible()
      await page.getByRole("button", { name: "Back", exact: true }).click()
      await expect(page.getByRole("tabpanel", { name: "Sessions", exact: true })).toBeVisible()
      await page.getByRole("button", { name: "Forward", exact: true }).click()
      await expect(page.getByRole("tabpanel", { name: "Checks", exact: true })).toBeVisible()
      await page.keyboard.press("Meta+[")
      await expect(page.getByRole("tabpanel", { name: "Sessions", exact: true })).toBeVisible()
      await page.keyboard.press("Meta+]")
      await expect(page.getByRole("tabpanel", { name: "Checks", exact: true })).toBeVisible()
      await opener.click()
      await page.keyboard.press("Escape")
      await expect(opener).toBeFocused()
      await expectNoHorizontalOverflow(page)
      await expectControlsReachable(page)
      await capture(page, `history-200-${theme}`, testInfo)
    })
  }

  for (const platform of ["windows", "linux"] as const) {
    for (const scale of [90, 200]) {
      test(`${platform} caption controls and resize handles at ${scale}%`, async ({
        page,
      }, testInfo) => {
        await page.setViewportSize({
          width: Math.round(640 / (scale / 100)),
          height: Math.round(480 / (scale / 100)),
        })
        await openFixture(page, "main", { platform, scale, theme: "dark" })
        await expect(page.locator("html")).toHaveAttribute("data-platform", platform)
        const controls = page.getByRole("group", { name: "Window controls" })
        await expect(controls).toBeVisible()
        await expectNoHorizontalOverflow(page)
        await expectControlsReachable(page)
        const handles = page.locator(".main-window-resize-edge")
        await expect(handles).toHaveCount(platform === "linux" ? 8 : 0)
        if (platform === "linux") {
          const east = page.locator('.main-window-resize-edge[data-direction="East"]')
          await expect(east).toBeVisible()
          await east.hover()
          await page.mouse.down()
          await page.mouse.up()
          await expect
            .poll(() => page.evaluate(() => window.__ANTIBURN_VISUAL_WINDOW_ACTIONS__))
            .toEqual(["resize:East"])
        }
        await controls.getByRole("button", { name: "Maximize window" }).click()
        await expect(controls.getByRole("button", { name: "Restore window" })).toBeVisible()
        await expect(handles).toHaveCount(0)
        await controls.getByRole("button", { name: "Restore window" }).click()
        await expect(controls.getByRole("button", { name: "Maximize window" })).toBeVisible()
        await expect(handles).toHaveCount(platform === "linux" ? 8 : 0)
        await controls.getByRole("button", { name: "Minimize window" }).click()
        await controls.getByRole("button", { name: "Close window" }).click()
        await expect
          .poll(() => page.evaluate(() => window.__ANTIBURN_VISUAL_WINDOW_ACTIONS__))
          .toEqual([
            ...(platform === "linux" ? ["resize:East"] : []),
            "toggleMaximize",
            "toggleMaximize",
            "minimize",
            "close",
          ])
        await expect(page.getByRole("alert")).toHaveCount(0)
        await capture(page, `${platform}-chrome-${scale}`, testInfo)
      })
    }
  }
})

test.describe("interface-scale resize focus", () => {
  for (const theme of ["light", "dark"] as const) {
    for (const returnToCollection of [false, true]) {
      test(`${theme}: preserves ${returnToCollection ? "collection" : "detail"} focus when compact`, async ({
        page,
      }, testInfo) => {
        await page.setViewportSize({ width: 1280, height: 860 })
        await openFixture(page, "main", { theme })
        await selectNavigationItem(page, "main", "Sessions")
        const row = page
          .getByRole("tabpanel", { name: "Sessions", exact: true })
          .locator("[data-session-row]")
          .filter({ hasText: "Make every interface scale reachable" })
        await row.click()
        const detail = page.locator("[data-detail-pane]")
        await expect(detail).toBeVisible()
        await detail.focus()
        await expect
          .poll(() => detail.evaluate((node) => node.contains(document.activeElement)))
          .toBe(true)
        if (returnToCollection) await row.focus()
        await page.setViewportSize({ width: 850, height: 860 })
        await expect(page.locator(".main-window-collection-detail")).toHaveAttribute(
          "data-single-pane",
          "true",
        )
        if (returnToCollection) {
          await expect(row).toBeFocused()
          await expect(row).toBeVisible()
        } else {
          await expect(detail).toBeVisible()
          await expect
            .poll(() => detail.evaluate((node) => node.contains(document.activeElement)))
            .toBe(true)
          await expect(page.getByRole("button", { name: "Back to sessions" })).toBeVisible()
        }
        await expect
          .poll(() =>
            page.evaluate(() => {
              const active = document.activeElement as HTMLElement | null
              return active !== document.body && active?.checkVisibility() === true
            }),
          )
          .toBe(true)
        await capture(
          page,
          `compact-focus-${theme}-${returnToCollection ? "collection" : "detail"}`,
          testInfo,
        )
      })
    }
  }
})

test.describe("interface-scale keyboard", () => {
  test.use({
    viewport: { width: 320, height: 240 },
    deviceScaleFactor: 2,
    reducedMotion: "reduce",
  })
  test("drawer and detail preserve keyboard focus at 200%", async ({ page }, testInfo) => {
    await openFixture(page, "main", { scale: 200, theme: "dark" })
    const opener = page.getByRole("button", { name: "Open Main navigation" })
    await opener.click()
    const dialog = page.getByRole("dialog", { name: "Main navigation" })
    await dialog.getByRole("tab", { name: "Checks", exact: true }).focus()
    await page.keyboard.press("ArrowDown")
    await expect(dialog).toBeVisible()
    await page.keyboard.press("Escape")
    await expect(dialog).not.toBeVisible()
    await expect(opener).toBeFocused()
    await selectNavigationItem(page, "main", "Sessions")
    const row = page
      .getByRole("tabpanel", { name: "Sessions", exact: true })
      .locator("[data-session-row]")
      .first()
    await row.click()
    const back = page.getByRole("button", { name: "Back to sessions" })
    await expect(back).toBeVisible()
    await back.click()
    await testInfo.attach("return-focus-state", {
      body: JSON.stringify(
        await page.evaluate(() => ({
          active: document.activeElement?.outerHTML,
          targets: Array.from(
            document.querySelectorAll(
              '.main-window-collection [aria-selected="true"], .main-window-collection [aria-current="true"]',
            ),
          ).map((node) => node.outerHTML),
        })),
      ),
      contentType: "application/json",
    })
    await expect(row).toBeFocused()
    await capture(page, "main-200-keyboard", testInfo)
  })
  test("settings controls stay reachable with reduced motion", async ({ page }, testInfo) => {
    await openFixture(page, "settings", { scale: 200, theme: "dark" })
    await selectNavigationItem(page, "settings", "Appearance")
    const control = page.getByRole("combobox", { name: "Interface size" })
    await control.scrollIntoViewIfNeeded()
    await control.focus()
    await expect(control).toBeInViewport()
    await expect(control).toBeFocused()
    await page.keyboard.press("Tab")
    await expect(page.getByRole("button", { name: "Reset to 100%" })).toBeFocused()
    await capture(page, "settings-200-keyboard", testInfo)
  })

  test("external session target opens the compact detail again after Back", async ({
    page,
  }, testInfo) => {
    await openFixture(page, "main", { scale: 200, theme: "dark" })
    await selectNavigationItem(page, "main", "Sessions")
    const row = page
      .getByRole("tabpanel", { name: "Sessions", exact: true })
      .locator("[data-session-row]")
      .first()
    await expect(row).toBeVisible()
    await page.waitForFunction(() => Boolean(window.__ANTIBURN_VISUAL_EMIT__))

    const emitTarget = (revision: number) =>
      page.evaluate(
        (target) => {
          const emit = window.__ANTIBURN_VISUAL_EMIT__
          if (!emit) throw new Error("Fixture event bridge is unavailable")
          emit("main:navigation-target", target)
        },
        {
          revision,
          destination: {
            section: "activity",
            target: { agent: "codex", sessionId: "fixture-active-session", wslDistro: null },
          },
        },
      )

    await emitTarget(1)
    const detail = page.locator("[data-detail-pane]")
    await expect(detail).toBeVisible()
    await expect
      .poll(() => detail.evaluate((node) => node.contains(document.activeElement)))
      .toBe(true)
    await capture(page, "main-200-external-target", testInfo)

    await page.getByRole("button", { name: "Back to sessions" }).click()
    await expect(row).toBeFocused()

    await emitTarget(2)
    await expect(detail).toBeVisible()
    await expect
      .poll(() => detail.evaluate((node) => node.contains(document.activeElement)))
      .toBe(true)
    await capture(page, "main-200-external-target-repeated", testInfo)
  })
})

test.describe("issue 507 targeted journeys", () => {
  test.use({
    viewport: { width: 320, height: 240 },
    deviceScaleFactor: 2,
    reducedMotion: "reduce",
  })

  for (const theme of ["light", "dark"] as const) {
    test(`settings reaches every pane at 200% constrained in ${theme}`, async ({
      page,
    }, testInfo) => {
      await openFixture(page, "settings", { scale: 200, theme })
      for (const pane of [
        "General",
        "Privacy",
        "Notifications",
        "Usage",
        "Sources",
        "Appearance",
        "About",
      ]) {
        await selectNavigationItem(page, "settings", pane)
        await expect(page.getByRole("tabpanel", { name: pane })).toBeVisible()
        await expectNoHorizontalOverflow(page)
        await expectControlsReachable(page)
        await capture(page, `settings-${pane.toLowerCase()}-200-${theme}`, testInfo)
      }
    })

    test(`onboarding advances through every step at 200% constrained in ${theme}`, async ({
      page,
    }, testInfo) => {
      await openFixture(page, "onboarding", { scale: 200, theme })
      const setup = page.getByRole("region", { name: "Set up antiburn" })
      await expect(
        setup.getByRole("heading", { name: "Stop hitting your token limits." }),
      ).toBeVisible()
      await expectNoHorizontalOverflow(page)
      await expectControlsReachable(page)
      await capture(page, `onboarding-welcome-200-${theme}`, testInfo)

      await page.getByRole("button", { name: "Continue" }).click()
      await expect(setup.getByRole("heading", { name: "Scan Locations: Agents" })).toBeVisible()
      await expectNoHorizontalOverflow(page)
      await expectControlsReachable(page)
      await capture(page, `onboarding-agents-200-${theme}`, testInfo)

      await page.getByRole("button", { name: "Continue" }).click()
      await expect(setup.getByRole("heading", { name: "Scan Locations: Repos" })).toBeVisible()
      await expectNoHorizontalOverflow(page)
      await expectControlsReachable(page)
      await capture(page, `onboarding-repos-200-${theme}`, testInfo)

      await page.getByRole("button", { name: "Continue" }).click()
      await expect(setup.getByRole("heading", { name: "Ready" })).toBeVisible()
      await expect(setup.getByText("128 sessions from the last 7 days")).toBeVisible()
      await expectNoHorizontalOverflow(page)
      await expectControlsReachable(page)
      await capture(page, `onboarding-ready-200-${theme}`, testInfo)
    })
  }
})

test.describe("interface-scale error recovery", () => {
  test.use({
    viewport: { width: 320, height: 240 },
    deviceScaleFactor: 2,
    reducedMotion: "reduce",
  })

  async function clearFixtureFault(page: Page) {
    await page.evaluate(() => {
      window.__ANTIBURN_VISUAL_FAULT__ = null
    })
  }

  test("session detail retries an analysis failure at 200%", async ({ page }, testInfo) => {
    await openFixture(page, "main", { scale: 200, theme: "dark", fault: "session-analysis" })
    await selectNavigationItem(page, "main", "Sessions")
    await page
      .getByRole("tabpanel", { name: "Sessions", exact: true })
      .locator("[data-session-row]")
      .first()
      .click()
    await expect(page.getByText("Could not refresh this session.")).toBeVisible()
    await expectNoHorizontalOverflow(page)
    await expectControlsReachable(page)
    await capture(page, "main-session-analysis-error-200-dark", testInfo)

    await clearFixtureFault(page)
    await page.getByRole("button", { name: "Retry" }).click()
    await expect(page.getByText("Could not refresh this session.")).not.toBeVisible()
  })

  test("settings retries an interface scale save failure at 200%", async ({
    page,
  }, testInfo) => {
    await openFixture(page, "settings", { scale: 200, theme: "dark", fault: "scale-save" })
    await selectNavigationItem(page, "settings", "Appearance")
    const control = page.getByRole("combobox", { name: "Interface size" })
    await control.selectOption("150")
    const alert = page.getByRole("alert")
    await expect(alert).toHaveText("Could not change interface size. Try again.")
    await alert.scrollIntoViewIfNeeded()
    await expectNoHorizontalOverflow(page)
    await expectControlsReachable(page)
    await capture(page, "settings-interface-scale-error-200-dark", testInfo)

    await clearFixtureFault(page)
    await control.selectOption("150")
    await expect(control).toHaveValue("150")
    await expect(alert).not.toBeVisible()
  })

  test("onboarding retries a bootstrap failure at 200%", async ({ page }, testInfo) => {
    await openFixture(page, "onboarding", {
      scale: 200,
      theme: "dark",
      fault: "onboarding-bootstrap",
    })
    await expect(page.getByText("antiburn could not start setup")).toBeVisible()
    await expect(page.getByText("Fixture onboarding bootstrap failure")).toBeVisible()
    await expectNoHorizontalOverflow(page)
    await expectControlsReachable(page)
    await capture(page, "onboarding-bootstrap-error-200-dark", testInfo)

    await clearFixtureFault(page)
    await page.getByRole("button", { name: "Try again" }).click()
    await expect(
      page.getByRole("heading", { name: "Stop hitting your token limits." }),
    ).toBeVisible()
  })

  test("preview shows its unavailable state when data fails at 200%", async ({
    page,
  }, testInfo) => {
    await openFixture(page, "preview", { scale: 200, theme: "dark", fault: "peek-data" })
    await expect(
      page.getByText("Preview unavailable. Move the pointer away, then try again."),
    ).toBeVisible()
    await expectNoHorizontalOverflow(page)
    await expectControlsReachable(page)
    await capture(page, "preview-data-error-200-dark", testInfo)
  })
})

test.describe("interface-scale matrix", () => {
  for (const surface of surfaces) {
    for (const scale of scales) {
      for (const theme of ["light", "dark"] as const) {
        for (const layout of layouts) {
          test.describe(`${surface} populated at ${scale}% ${theme} ${layout.name}`, () => {
            const factor = scale / 100
            test.use({
              viewport: {
                width: Math.round(layout.viewport.width / factor),
                height: Math.round(layout.viewport.height / factor),
              },
              deviceScaleFactor: factor,
              colorScheme: theme,
            })
            test("stays reachable", async ({ page }, testInfo) => {
              await openFixture(page, surface, { scale, theme })
              if (surface === "main") {
                await expect(
                  page.getByRole("group", { name: "Allowance for the past 30 days" }),
                ).toBeVisible()
                await expectNoHorizontalOverflow(page)
                await expectControlsReachable(page)
                await capture(page, `overview-${layout.name}-${scale}-${theme}`, testInfo)
                if (scale === 200 && layout.name === "constrained") {
                  const chart = page.getByRole("group", {
                    name: "Allowance for the past 30 days",
                  })
                  const lastDay = chart.locator('button[data-day-index="29"]')
                  await lastDay.focus()
                  await expect(lastDay).toBeFocused()
                  await expect(lastDay).toBeInViewport()
                  await lastDay.press("ArrowLeft")
                  const previousDay = chart.locator('button[data-day-index="28"]')
                  await expect(previousDay).toBeFocused()
                  await expect(previousDay).toBeInViewport()
                  await expect
                    .poll(() =>
                      page
                        .locator(".overview-viewport .ui-scroll-viewport")
                        .evaluate((node) => node.scrollTop),
                    )
                    .toBeGreaterThan(0)
                  await capture(page, `overview-chart-scrolled-200-${theme}`, testInfo)
                  const providers = page.getByRole("region", {
                    name: "Provider limits card",
                  })
                  await providers.focus()
                  await expect(providers).toBeFocused()
                  await expect(providers).toBeInViewport()
                  await providers.press("End")
                  await expect
                    .poll(() => providers.evaluate((node) => node.scrollTop))
                    .toBeGreaterThan(0)
                }
                await selectNavigationItem(page, "main", "Sessions")
              }
              if (surface === "settings")
                await selectNavigationItem(page, "settings", "Appearance")
              await expectNoHorizontalOverflow(page)
              await expectControlsReachable(page)
              await capture(page, `${surface}-${layout.name}-${scale}-${theme}`, testInfo)
            })
          })
        }
      }
    }
  }

  test.describe("state variants", () => {
    test.use({ viewport: { width: 320, height: 240 }, deviceScaleFactor: 2 })
    for (const surface of surfaces) {
      for (const state of states.filter((state) => state !== "populated")) {
        test(`${surface} ${state} state stays bounded`, async ({ page }, testInfo) => {
          await openFixture(page, surface, { scale: 200, theme: "dark", state })
          if (surface === "main") await selectNavigationItem(page, "main", "Sessions")
          if (surface === "settings") await selectNavigationItem(page, "settings", "Appearance")
          await expect(page.getByTestId("fixture-surface")).toHaveText(surface)
          await expectNoHorizontalOverflow(page)
          await expectControlsReachable(page)
          await capture(page, `${surface}-constrained-200-dark-${state}`, testInfo)
        })
      }
    }
  })
})
