import { expect, test, type Locator, type Page, type TestInfo } from "@playwright/test"
import { AGENT_SLUGS, agentIconName } from "../../src/lib/presentation/agents"

async function open(
  page: Page,
  surface: "main" | "settings" | "icons",
  theme: "light" | "dark",
  extra = "",
) {
  await page.clock.setFixedTime(new Date("2026-09-15T00:00:00.000Z"))
  await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" })
  const params = new URLSearchParams({ surface, theme, vendors: "all" })
  new URLSearchParams(extra.replace(/^&/, "")).forEach((value, key) => params.set(key, value))
  await page.goto(`/tests/visual/?${params}`)
  await expect(page.locator(".fixture-scale-root > *").first()).toBeVisible({ timeout: 30_000 })
}

async function capture(page: Page, name: string, info: TestInfo) {
  await page.evaluate(() => document.fonts.ready)
  const closePromotion = page.getByRole("button", { name: "Close promotion", exact: true })
  if (await closePromotion.isVisible()) await closePromotion.click()
  await expect(page.locator(".promotion-panel")).toHaveCount(0)
  await page.screenshot({ path: info.outputPath(`${name}.png`), animations: "disabled" })
}

async function renderedPixelVariation(page: Page, target: Locator) {
  const screenshot = await target.screenshot({ animations: "disabled" })
  return page.evaluate(async (base64) => {
    const image = new Image()
    image.src = `data:image/png;base64,${base64}`
    await image.decode()
    const canvas = document.createElement("canvas")
    canvas.width = image.naturalWidth
    canvas.height = image.naturalHeight
    const context = canvas.getContext("2d")
    if (!context) throw new Error("Cannot read screenshot pixels")
    context.drawImage(image, 0, 0)
    const { data } = context.getImageData(0, 0, canvas.width, canvas.height)
    let variation = 0
    for (let index = 4; index < data.length; index += 4) {
      for (let channel = 0; channel < 4; channel++) {
        variation = Math.max(variation, Math.abs(data[index + channel]! - data[channel]!))
      }
    }
    return variation
  }, screenshot.toString("base64"))
}

async function expectImagesLoaded(page: Page) {
  const images = page.locator("[data-agent-icon] img:visible")
  for (const img of await images.all()) {
    await expect(img).toHaveJSProperty("complete", true)
    await expect(img).not.toHaveJSProperty("naturalWidth", 0)
  }
}

for (const theme of ["light", "dark"] as const) {
  test(`all vendor artwork and neutral marks render in ${theme}`, async ({ page }, info) => {
    const errors: string[] = []
    page.on("pageerror", (error) => errors.push(error.message))
    await open(page, "icons", theme)
    for (const slug of AGENT_SLUGS) {
      const row = page.locator(`[data-vendor="${slug}"]`)
      await expect(row).toBeVisible()
      for (const size of [16, 28, 40]) {
        const icon = row.locator(
          `[data-appearance="default"][data-size="${size}"] [role="img"]`,
        )
        await expect(icon).toHaveAttribute("data-agent-icon", agentIconName(slug))
        {
          await expect(icon.locator("img:visible")).toHaveCount(1)
          await expect(icon.locator(`.agent-icon-${theme}`)).toBeVisible()
          const box = await icon.boundingBox()
          expect(box?.width).toBe(size)
          expect(box?.height).toBe(size)
        }
        const neutral = row.locator(
          `[data-appearance="neutral"][data-size="${size}"] [role="img"]`,
        )
        await expect(neutral.locator("img")).toHaveCount(0)
        await expect(neutral).toHaveClass(/text-label-tertiary/)
        const mask = neutral.locator("span")
        await expect(mask).toHaveCSS(
          "mask-mode",
          ["kiro", "opencode"].includes(slug) ? "luminance" : "alpha",
        )
        await expect(mask).not.toHaveCSS("mask-image", "none")
        expect(
          await renderedPixelVariation(page, mask),
          `${slug} neutral ${size}`,
        ).toBeGreaterThan(0)
      }
    }
    await expectImagesLoaded(page)
    expect(errors).toEqual([])
    await capture(page, `gallery-${theme}`, info)
  })

  test(`Agents discovery includes Kiro and Amp in ${theme}`, async ({ page }, info) => {
    await open(page, "main", theme, "&onboarding=complete")
    await page.getByRole("tab", { name: "Agents", exact: true }).click()
    await expect(page.getByRole("heading", { name: "Agents", exact: true })).toBeVisible()
    for (const slug of AGENT_SLUGS)
      await expect(
        page.locator(`[data-agent-icon="${agentIconName(slug)}"]:visible`).first(),
      ).toBeVisible()
    await expectImagesLoaded(page)
    await capture(page, `agents-found-${theme}`, info)
  })

  test(`Settings Agents and Sessions in ${theme}`, async ({ page }, info) => {
    const errors: string[] = []
    page.on("pageerror", (error) => errors.push(error.message))
    await open(page, "settings", theme)
    await page.getByRole("tab", { name: "Agents", exact: true }).click()
    for (const slug of AGENT_SLUGS) {
      const icon = page.locator(`[data-agent-icon="${agentIconName(slug)}"]`).first()
      await icon.scrollIntoViewIfNeeded()
      await expect(icon).toBeVisible()
    }
    await expectImagesLoaded(page)
    await capture(page, `settings-agents-bottom-${theme}`, info)
    await page.locator('[data-agent-icon="claude"]').first().scrollIntoViewIfNeeded()
    await capture(page, `settings-agents-top-${theme}`, info)
    await page.getByRole("tab", { name: "Sessions", exact: true }).click()
    await expect(page.getByRole("heading", { name: "Sessions", exact: true })).toBeVisible()
    await capture(page, `settings-sessions-${theme}`, info)
    expect(errors).toEqual([])
  })

  test(`Sessions retain neutral card icons and branded filters in ${theme}`, async ({
    page,
  }, info) => {
    await open(page, "main", theme, "&onboarding=complete")
    await page.getByRole("tab", { name: "Sessions", exact: true }).click()
    await expect(page.locator('[aria-label="Sessions"]:visible [role="list"]')).toBeVisible()
    for (const slug of AGENT_SLUGS) {
      const card = page
        .locator('[aria-label="Sessions"]:visible [role="list"]')
        .locator(`[data-agent-icon="${agentIconName(slug)}"]`)
        .first()
      await card.scrollIntoViewIfNeeded()
      await expect(card).toHaveClass(/text-label-tertiary/)
      await expect(card.locator("img")).toHaveCount(0)
    }
    await capture(page, `sessions-neutral-bottom-${theme}`, info)
    await page
      .locator('[aria-label="Sessions"]:visible [role="list"]')
      .locator('[data-agent-icon="claude"]')
      .first()
      .scrollIntoViewIfNeeded()
    await capture(page, `sessions-neutral-top-${theme}`, info)
    await page.getByRole("button", { name: "Filters", exact: true }).click()
    await expect(page.getByRole("menu", { name: "Filters" })).toBeVisible()
    for (const slug of AGENT_SLUGS) {
      const icon = page
        .getByRole("menu", { name: "Filters" })
        .locator(`[data-agent-icon="${agentIconName(slug)}"]`)
      await icon.scrollIntoViewIfNeeded()
      await expect(icon).toHaveClass(/text-agent-mark/)
    }
    await expectImagesLoaded(page)
    await capture(page, `session-filters-${theme}`, info)
  })

  test(`onboarding vendor strip and expanded settings in ${theme}`, async ({ page }, info) => {
    await open(page, "main", theme)
    await page.getByRole("button", { name: "Get Started", exact: true }).click()
    await expect(page.locator('[data-agent-icon="kiro"]').first()).toBeVisible()
    await expect(page.locator('[data-agent-icon="amp"]').first()).toBeVisible()
    await expectImagesLoaded(page)
    await capture(page, `onboarding-agents-${theme}`, info)
    await page.getByRole("button", { name: "Expand settings", exact: true }).click()
    await expect(page.locator('[data-agent-icon="kiro"]')).toHaveCount(2)
    await capture(page, `onboarding-agents-settings-${theme}`, info)
  })
}

test("explicit and system themes choose exactly one vendor image", async ({ page }) => {
  await open(page, "icons", "light")
  for (const system of ["light", "dark"] as const) {
    await page.emulateMedia({ colorScheme: system })
    for (const explicit of ["light", "dark", null] as const) {
      await page.evaluate((theme) => {
        if (theme) document.documentElement.dataset.theme = theme
        else delete document.documentElement.dataset.theme
      }, explicit)
      const expected = explicit ?? system
      const visible = page.locator('[data-appearance="default"] img:visible')
      await expect(visible).toHaveCount(36)
      for (const img of await visible.all())
        await expect(img).toHaveClass(`agent-icon-${expected}`)
    }
  }
})

test("other agents without sessions retain their vendor marks", async ({ page }, info) => {
  await open(page, "main", "light", "&onboarding=complete&vendors=mixed")
  await page.getByRole("tab", { name: "Agents", exact: true }).click()
  await page.getByText("Other agents checked", { exact: false }).click()
  await expect(page.locator('details [data-agent-icon="kiro"]')).toBeVisible()
  await expect(page.locator('details [data-agent-icon="amp"]')).toBeVisible()
  await expectImagesLoaded(page)
  await capture(page, "agents-no-sessions-expanded-light", info)
})

test("vendor marks survive reduced motion and reduced transparency", async ({ page }, info) => {
  await open(page, "icons", "dark")
  const session = await page.context().newCDPSession(page)
  await session.send("Emulation.setEmulatedMedia", {
    features: [
      { name: "prefers-reduced-motion", value: "reduce" },
      { name: "prefers-reduced-transparency", value: "reduce" },
    ],
  })
  expect(
    await page.evaluate(() => matchMedia("(prefers-reduced-motion: reduce)").matches),
  ).toBe(true)
  expect(
    await page.evaluate(() => matchMedia("(prefers-reduced-transparency: reduce)").matches),
  ).toBe(true)
  await expect(page.locator('[data-appearance="default"] img:visible')).toHaveCount(36)
  await expectImagesLoaded(page)
  await capture(page, "gallery-reduced-motion-transparency-dark", info)
})

for (const vendors of ["all", "mixed"]) {
  test(`empty discovery takes precedence over ${vendors} vendor fixtures`, async ({ page }) => {
    await open(page, "main", "light", `&state=empty&vendors=${vendors}`)
    await page.getByRole("button", { name: "Get Started", exact: true }).click()
    await expect(page.getByText("No agents found", { exact: true })).toBeVisible({
      timeout: 3_000,
    })
  })
}

test("pixel validation rejects a neutral mask with no visible ink", async ({ page }) => {
  await open(page, "icons", "light")
  const mask = page.locator(
    '[data-vendor="claude-code"] [data-appearance="neutral"][data-size="40"] [role="img"] > span',
  )
  expect(await renderedPixelVariation(page, mask)).toBeGreaterThan(0)
  await mask.evaluate((node) => {
    node.style.maskImage = "linear-gradient(transparent, transparent)"
  })
  expect(await renderedPixelVariation(page, mask)).toBe(0)
})

for (const [vendors, total] of [
  ["all", 12],
  ["mixed", 3],
  ["", 128],
] as const) {
  test(`session totals match ${vendors || "default"} fixture data`, async ({ page }) => {
    await open(page, "main", "light", `&onboarding=complete&vendors=${vendors}`)
    await expect(page.getByRole("tab", { name: "Sessions", exact: true })).toHaveText(
      new RegExp(`Sessions\\s*${total}$`),
      { timeout: 3_000 },
    )
  })
}

for (const vendors of ["all", "mixed"]) {
  test(`empty ${vendors} fixture keeps a zero session total`, async ({ page }) => {
    await open(page, "main", "light", `&onboarding=complete&state=empty&vendors=${vendors}`)
    await expect(page.getByRole("tab", { name: "Sessions", exact: true })).toHaveText(
      /Sessions\s*0$/,
    )
  })
}
