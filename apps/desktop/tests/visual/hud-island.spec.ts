import { expect, test, type Page, type TestInfo } from "@playwright/test"
import { fixtureIsland } from "./fakes/hud"

type Phase = "collapsed" | "expanded" | "preview" | "off"
const scales = [90, 100, 110, 125, 150, 175, 200] as const

async function openHud(
  page: Page,
  scale: number,
  theme: "light" | "dark",
  phase: Phase,
  options: Record<string, string> = {},
) {
  const params = new URLSearchParams({
    surface: "hud",
    scale: String(scale),
    theme,
    island: phase,
    ...options,
  })
  const native = fixtureIsland(params)
  const floating = phase === "off" || phase === "preview"
  const nativeWidth = floating ? 176 * native.scale : native.bodyWidth + 2 * native.fillet
  const nativeHeight = floating
    ? 420 * native.scale
    : native.height + (phase === "collapsed" ? 0 : native.bodyMaxHeight)
  await page.setViewportSize({
    width: Math.ceil(nativeWidth / native.scale),
    height: Math.ceil(nativeHeight / native.scale),
  })
  await page.clock.setFixedTime(new Date("2026-09-15T00:00:00.000Z"))
  await page.goto(`/tests/visual/?${params}`)
  await expect(
    page.locator(phase === "off" ? ".hud-frame" : `[data-island=${phase}]`),
  ).toBeVisible()
  await expect
    .poll(() =>
      page.evaluate(() =>
        window.__ANTIBURN_VISUAL_HUD_RESIZES__?.some(
          (request) => request.geometryRevision === 7,
        ),
      ),
    )
    .toBe(true)
  return native
}

async function noHorizontalClip(page: Page) {
  const clipped = await page
    .locator("#root, [data-testid=island-body], .hud-detail-card")
    .evaluateAll((nodes) =>
      nodes
        .filter((node) => node.scrollWidth > node.clientWidth + 1)
        .map((node) => ({
          element: node.getAttribute("data-testid") ?? node.className,
          client: node.clientWidth,
          scroll: node.scrollWidth,
        })),
    )
  expect(clipped).toEqual([])
  expect(
    await page.locator(".fixture-scale-root").evaluate((node) => getComputedStyle(node).zoom),
  ).toBe("1")
}

async function capture(page: Page, testInfo: TestInfo, name: string) {
  await page.evaluate(() => document.fonts.ready)
  const path = testInfo.outputPath(`${name}.png`)
  await page.screenshot({ path, animations: "disabled" })
  await testInfo.attach(name, { path, contentType: "image/png" })
}

for (const percent of scales) {
  test.describe(`HUD native geometry ${percent}%`, () => {
    test.use({ deviceScaleFactor: percent / 100, reducedMotion: "reduce" })

    for (const theme of ["light", "dark"] as const) {
      test(`fixed camera gap and scaled wings in all phases in ${theme}`, async ({
        page,
      }, testInfo) => {
        for (const phase of ["collapsed", "expanded", "preview", "off"] as const) {
          await test.step(phase, async () => {
            const native = await openHud(page, percent, theme, phase)
            if (phase === "collapsed" || phase === "expanded") {
              const header = await page.getByTestId("island-header").boundingBox()
              expect(header).not.toBeNull()
              const expanded = phase === "expanded"
              expect(header!.width * native.scale).toBeCloseTo(
                expanded ? native.bodyWidth : 204 + 60 * native.scale,
                0,
              )
              expect(header!.height * native.scale).toBeCloseTo(32, 0)
              expect(header!.x * native.scale).toBeCloseTo(
                (expanded ? 0 : native.headerOffset) + 19,
                0,
              )
              const wings = await page
                .locator(".hud-island-wing")
                .evaluateAll((nodes) => nodes.map((node) => node.getBoundingClientRect().width))
              for (const width of wings)
                expect(width * native.scale).toBeCloseTo(
                  expanded ? (native.bodyWidth - native.notch) / 2 : native.wing,
                  0,
                )
              const gap = await page
                .locator('[data-testid="island-header"] > div')
                .nth(1)
                .boundingBox()
              expect(gap!.width * native.scale).toBeCloseTo(204, 0)
              expect(gap!.x * native.scale).toBeCloseTo(
                native.headerOffset + native.wing + 19,
                0,
              )
              for (const [id, width, height] of [
                ["island-mark", 8.125, 8.125],
                ["island-live-led", 11.375, 3.25],
              ] as const) {
                const box = await page.getByTestId(id).boundingBox()
                expect(box!.width * native.scale).toBeCloseTo(width * native.scale, 0)
                expect(box!.height * native.scale).toBeCloseTo(height * native.scale, 0)
              }
              if (phase === "collapsed") {
                await expect(page.getByTestId("island-body")).toHaveCount(0)
              } else {
                await expect(page.getByTestId("hud-bar-label")).toHaveCount(2)
                const body = await page.getByTestId("island-body").boundingBox()
                expect(header!.x).toBeCloseTo(body!.x, 2)
                expect(header!.width).toBeCloseTo(body!.width, 2)
                expect(body!.width * native.scale).toBeCloseTo(native.bodyWidth, 0)
                expect(body!.height * native.scale).toBeLessThanOrEqual(
                  native.bodyMaxHeight + 1,
                )
                expect(body!.y * native.scale).toBeCloseTo(32, 0)
              }
            } else if (phase === "preview") {
              await expect(page.getByTestId("island-body")).toBeVisible()
              expect(
                (await page.getByTestId("island-header").boundingBox())!.height * native.scale,
              ).toBeCloseTo(32, 0)
            } else {
              await expect(page.getByTestId("island-header")).toHaveCount(0)
              await expect(page.locator(".hud-leds")).toBeVisible()
            }
            if (phase === "expanded" || phase === "preview") {
              const colors = await page
                .locator(".hud-leds .led-lit")
                .first()
                .evaluate((led) => {
                  const body = led.closest("[data-testid=island-body]")!
                  return [
                    getComputedStyle(led).backgroundColor,
                    getComputedStyle(body).backgroundColor,
                  ] as const
                })
              const luminance = (color: string) => {
                expect(color).toMatch(/^rgb\(/)
                const channels = color
                  .match(/[\d.]+/g)!
                  .map(Number)
                  .map((channel) => {
                    const value = channel / 255
                    return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4
                  })
                expect(channels).toHaveLength(3)
                return channels[0]! * 0.2126 + channels[1]! * 0.7152 + channels[2]! * 0.0722
              }
              const ink = luminance(colors[0])
              const background = luminance(colors[1])
              expect(
                (Math.max(ink, background) + 0.05) / (Math.min(ink, background) + 0.05),
              ).toBeGreaterThanOrEqual(3)
            } else if (phase === "off") {
              expect(
                await page
                  .locator(".hud-leds .led-lit")
                  .first()
                  .evaluate((led) => (led as HTMLElement).style.backgroundColor),
              ).toBe("var(--color-label)")
            }
            await noHorizontalClip(page)
            if ([90, 100, 200].includes(percent))
              await capture(page, testInfo, `${phase}-${percent}-${theme}`)
          })
        }
      })
    }

    if ([90, 100, 200].includes(percent)) {
      for (const theme of ["light", "dark"] as const) {
        test(`short display reaches final content and rejects stale state in ${theme}`, async ({
          page,
        }, testInfo) => {
          const cdp = await page.context().newCDPSession(page)
          await cdp.send("Emulation.setEmulatedMedia", {
            features: [
              { name: "prefers-reduced-motion", value: "reduce" },
              { name: "prefers-reduced-transparency", value: "reduce" },
              { name: "prefers-color-scheme", value: theme },
            ],
          })
          const native = await openHud(page, percent, theme, "expanded", {
            state: "long",
            display: "short",
            map: "on",
          })
          await expect(page.getByTestId("hud-bar-label")).toHaveCount(12)
          await expect(page.getByTestId("hud-map-legend").locator("li")).toHaveCount(8)
          const body = page.getByTestId("island-body")
          const before = await body.boundingBox()
          expect(before!.height * native.scale).toBeCloseTo(native.bodyMaxHeight, 0)
          expect(await body.evaluate((node) => node.scrollHeight > node.clientHeight)).toBe(
            true,
          )
          await noHorizontalClip(page)
          await capture(page, testInfo, `short-top-${percent}-${theme}`)
          await body.hover()
          await page.mouse.wheel(0, 10_000)
          await expect(page.getByTestId("hud-bar-reset").last()).toBeInViewport()
          const bottom = await page.getByTestId("hud-bar-reset").last().boundingBox()
          expect(bottom!.y + bottom!.height).toBeLessThanOrEqual(before!.y + before!.height)
          await capture(page, testInfo, `short-bottom-${percent}-${theme}`)

          await page.evaluate(
            (state) => window.__ANTIBURN_VISUAL_EMIT__?.("hud-island:state", state),
            {
              ...native,
              revision: 8,
              bodyMaxHeight: 120,
            },
          )
          await expect
            .poll(async () => Math.round((await body.boundingBox())!.height * native.scale))
            .toBe(120)
          await page.evaluate(
            (state) => window.__ANTIBURN_VISUAL_EMIT__?.("hud-island:state", state),
            {
              ...native,
              revision: 7,
              island: "collapsed",
            },
          )
          await expect(page.locator("[data-island=expanded]")).toBeVisible()
          await expect
            .poll(() =>
              page.evaluate(
                () => window.__ANTIBURN_VISUAL_HUD_RESIZES__?.at(-1)?.geometryRevision,
              ),
            )
            .toBe(8)
          expect(
            await page.evaluate(
              () => matchMedia("(prefers-reduced-transparency: reduce)").matches,
            ),
          ).toBe(true)
        })

        test(`empty HUD and token map detail in ${theme}`, async ({ page }, testInfo) => {
          await openHud(page, percent, theme, "off", { map: "on" })
          await expect(page.locator("svg[data-dot-value] [data-blob]")).toHaveCount(3)
          await noHorizontalClip(page)
          await capture(page, testInfo, `floating-map-${percent}-${theme}`)

          await openHud(page, percent, theme, "expanded", { state: "empty", map: "on" })
          await expect(page.getByTestId("hud-bar-label")).toHaveCount(0)
          await expect(page.getByTestId("hud-map-legend")).toHaveCount(0)
          await expect(page.locator(".hud-leds")).toBeVisible()
          await noHorizontalClip(page)
          await capture(page, testInfo, `empty-${percent}-${theme}`)

          await page.setViewportSize({ width: 176, height: 500 })
          for (const detail of ["usage", "session"] as const) {
            await page.goto(
              `/tests/visual/?surface=hud-detail&scale=${percent}&theme=${theme}&map=on&detail=${detail}`,
            )
            await expect(
              page.getByTestId(detail === "usage" ? "hud-detail-map" : "hud-detail-session"),
            ).toBeVisible()
            if (detail === "usage")
              await expect(page.getByTestId("hud-detail-spend")).toHaveText("$0.12 per minute")
            else
              await expect(page.locator("[data-lit=true]")).toContainText("sub-agent fixture-")
            await expect(page.locator(".hud-detail-card p").last()).toBeInViewport()
            await noHorizontalClip(page)
            await capture(page, testInfo, `detail-${detail}-${percent}-${theme}`)
          }
        })
      }
    }
  })
}

test("header alignment follows interrupted phase and scale updates", async ({ page }) => {
  await openHud(page, 200, "dark", "expanded")
  let revision = 7
  for (const [phase, percent] of [
    ["collapsed", 200],
    ["expanded", 200],
    ["collapsed", 90],
    ["expanded", 90],
    ["expanded", 200],
  ] as const) {
    const state = {
      ...fixtureIsland(new URLSearchParams({ island: phase, scale: String(percent) })),
      revision: ++revision,
    }
    await page.evaluate(
      (next) => window.__ANTIBURN_VISUAL_EMIT__?.("hud-island:state", next),
      state,
    )
    await page.setViewportSize({
      width: Math.ceil((state.bodyWidth + 2 * state.fillet) / state.scale),
      height: Math.ceil((state.height + state.bodyMaxHeight) / state.scale),
    })
    await expect(page.locator(`[data-island=${phase}]`)).toBeVisible()
    const header = await page.getByTestId("island-header").boundingBox()
    const gap = await page.locator('[data-testid="island-header"] > div').nth(1).boundingBox()
    expect(header!.height * state.scale).toBeCloseTo(32, 0)
    expect(gap!.x * state.scale).toBeCloseTo(state.headerOffset + state.wing + state.fillet, 0)
    if (phase === "expanded") {
      const body = await page.getByTestId("island-body").boundingBox()
      expect(header!.x).toBeCloseTo(body!.x, 2)
      expect(header!.width).toBeCloseTo(body!.width, 2)
      const hits = await page.getByTestId("island-header").evaluate((node) => {
        const rect = node.getBoundingClientRect()
        return [rect.left + 2, rect.right - 2].map(
          (x) =>
            document
              .elementFromPoint(x, rect.top + rect.height / 2)
              ?.closest('[data-testid="island-header"]') === node,
        )
      })
      expect(hits).toEqual([true, true])
    }
  }
})

test.describe("HUD expanded body interaction", () => {
  test.use({ deviceScaleFactor: 2, reducedMotion: "reduce" })

  test("short notch height caps both indicators without stretching them", async ({ page }) => {
    await openHud(page, 200, "light", "collapsed", { header: "short" })
    const header = await page.getByTestId("island-header").boundingBox()
    const mark = await page.getByTestId("island-mark").boundingBox()
    const led = await page.getByTestId("island-live-led").boundingBox()
    expect(header!.height * 2).toBeCloseTo(16, 0)
    expect(mark!.height * 2).toBeCloseTo(8, 0)
    expect(mark!.width).toBeCloseTo(mark!.height, 2)
    expect(led!.width / led!.height).toBeCloseTo(3.5, 1)
    expect(mark!.y * 2).toBeGreaterThanOrEqual(4)
    expect((mark!.y + mark!.height) * 2).toBeLessThanOrEqual(12)
  })

  test("a usage label starts tear-off while wheel scrolling stays inside the body", async ({
    page,
  }) => {
    await openHud(page, 200, "dark", "expanded", { state: "long", display: "short" })
    await expect(page.getByTestId("hud-bar-label")).toHaveCount(12)
    const body = page.getByTestId("island-body")
    await body.hover()
    await page.mouse.wheel(0, 100)
    await expect.poll(() => body.evaluate((node) => node.scrollTop)).toBeGreaterThan(0)
    expect(await page.evaluate(() => window.__ANTIBURN_VISUAL_HUD_DRAGS__ ?? [])).toEqual([])
    await page.mouse.wheel(0, -10_000)
    await expect.poll(() => body.evaluate((node) => node.scrollTop)).toBe(0)

    const label = await page.getByTestId("hud-bar-label").first().boundingBox()
    expect(label).not.toBeNull()
    const x = label!.x + label!.width / 2
    const y = label!.y + label!.height / 2
    await page.mouse.move(x, y)
    await page.mouse.down()
    await page.mouse.move(x + 30, y + 15, { steps: 3 })
    await expect
      .poll(() => page.evaluate(() => window.__ANTIBURN_VISUAL_HUD_DRAGS__ ?? []))
      .toContain("tear_off_overlay")
    await page.mouse.up()
    await expect
      .poll(() => page.evaluate(() => window.__ANTIBURN_VISUAL_HUD_DRAGS__ ?? []))
      .toContain("hud_drag_ended")
  })
})
