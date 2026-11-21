import { expect, test, type Page, type TestInfo } from "@playwright/test"
import { writeFile } from "node:fs/promises"
import { fixtureIsland } from "./fakes/hud"

const barCounts = [1, 3, 6, 12] as const
type Motion = "no-preference" | "reduce"
type ClosingSample = {
  bodyStillMounted: boolean
  opacity: string
  header: { x: number; y: number; width: number; height: number }
  intermediateClip?: boolean
  opacityStayedOpaque?: boolean
}

test.use({ video: "on" })

async function openFixture(page: Page, bars: number, motion: Motion, short = false) {
  await page.emulateMedia({ reducedMotion: motion })
  const params = new URLSearchParams({
    surface: "hud",
    island: "collapsed",
    scale: "100",
    bars: String(bars),
    ...(short ? { display: "short" } : {}),
  })
  const geometry = fixtureIsland(params)
  await page.setViewportSize({
    width: Math.ceil((geometry.bodyWidth + 2 * geometry.fillet) / geometry.scale),
    height: Math.ceil((geometry.height + geometry.bodyMaxHeight) / geometry.scale),
  })
  await page.goto(`/tests/visual/?${params}`)
  await expect(page.getByTestId("island-header")).toBeVisible()
  await expect(page.getByTestId("hud-bar-label")).toHaveCount(0)
  return geometry
}

async function emitPhase(page: Page, phase: "expanded" | "collapsed", revision: number) {
  const state = {
    ...fixtureIsland(
      new URLSearchParams(`island=${phase}&scale=100${locationSearchDisplay(page)}`),
    ),
    revision,
  }
  await page.evaluate(
    (next) => window.__ANTIBURN_VISUAL_EMIT__!("hud-island:state", next),
    state,
  )
}

function locationSearchDisplay(page: Page) {
  return new URL(page.url()).searchParams.get("display") === "short" ? "&display=short" : ""
}

async function sampleClipDuringEntry(page: Page) {
  return page.evaluate(
    () =>
      new Promise<number | null>((resolve) => {
        const onTransition = (event: Event) => {
          const body = event.target as HTMLElement
          if (
            body.dataset.testid !== "island-body" ||
            (event as TransitionEvent).propertyName !== "clip-path"
          )
            return
          const deadline = performance.now() + 800
          const sample = () => {
            const clip = getComputedStyle(body).clipPath
            const match = clip.match(/inset\([^ ]+ [^ ]+ ([\d.]+)(px|%)/)
            const bottom = match
              ? Number(match[1]) * (match[2] === "%" ? body.clientHeight / 100 : 1)
              : Number.NaN
            if (Number.isFinite(bottom) && bottom > 0 && bottom < body.clientHeight)
              return resolve(bottom)
            if (performance.now() >= deadline) return resolve(null)
            requestAnimationFrame(sample)
          }
          requestAnimationFrame(sample)
        }
        document.addEventListener("transitionrun", onTransition)
        window.setTimeout(() => resolve(null), 1000)
      }),
  )
}

async function capture(page: Page, testInfo: TestInfo, name: string) {
  const path = testInfo.outputPath(`${name}.png`)
  await page.screenshot({ path, animations: "disabled" })
  await testInfo.attach(name, { path, contentType: "image/png" })
}

for (const bars of barCounts) {
  for (const motion of ["no-preference", "reduce"] as const) {
    test(`HUD height with ${bars} usage bars and ${motion} motion`, async ({
      page,
    }, testInfo) => {
      const geometry = await openFixture(page, bars, motion)
      const header = page.getByTestId("island-header")
      const headerBefore = await header.boundingBox()
      expect(headerBefore).not.toBeNull()
      const entering = motion === "no-preference" ? sampleClipDuringEntry(page) : null
      await emitPhase(page, "expanded", 8)
      const body = page.getByTestId("island-body")
      await expect(body).toBeVisible()
      await expect(page.getByTestId("hud-bar-label")).toHaveCount(bars)
      await expect(body).toHaveCSS("opacity", "1")
      await expect(body).toHaveCSS(
        "transition-duration",
        motion === "reduce" ? "0.06s" : "0.2s, 0.2s, 0.2s",
      )

      if (motion === "reduce") {
        await expect(body).toHaveCSS("transform", "none")
        await expect(body).toHaveCSS("clip-path", "none")
        await expect(body).toHaveCSS("transition-property", "opacity")
        await expect(body).toHaveCSS("transition-delay", "0s")
      } else {
        expect(await entering).not.toBeNull()
        await expect(body).toHaveCSS("clip-path", "inset(0px round 0px 0px 24px 24px)")
        await expect(body).toHaveCSS("transform", "matrix(1, 0, 0, 1, 0, 0)")
      }

      const bodyBefore = await body.boundingBox()
      const headerOpen = await header.boundingBox()
      const bodyHeight = bodyBefore!.height * geometry.scale
      const expectedNaturalHeight = 58.40625 + (bars - 1) * 50.15625
      expect(bodyHeight).toBeCloseTo(Math.min(expectedNaturalHeight, geometry.bodyMaxHeight), 0)
      const summaryPath = testInfo.outputPath("geometry-summary.json")
      await writeFile(
        summaryPath,
        JSON.stringify(
          { visibleBars: bars, motion, bodyHeight, bodyMaxHeight: geometry.bodyMaxHeight },
          null,
          2,
        ),
      )
      await testInfo.attach("geometry-summary", {
        path: summaryPath,
        contentType: "application/json",
      })
      expect(headerOpen!.x).toBeCloseTo(headerBefore!.x, 2)
      expect(headerOpen!.y).toBeCloseTo(headerBefore!.y, 2)
      expect(headerOpen!.width).toBeCloseTo(headerBefore!.width, 2)
      expect(headerOpen!.height).toBeCloseTo(headerBefore!.height, 2)
      expect(bodyBefore!.height * geometry.scale).toBeLessThanOrEqual(
        geometry.bodyMaxHeight + 1,
      )
      expect(bodyBefore!.y * geometry.scale).toBeCloseTo(geometry.height, 0)
      await capture(page, testInfo, `bars-${bars}-${motion}-open`)

      const collapsed = {
        ...fixtureIsland(
          new URLSearchParams(`island=collapsed&scale=100${locationSearchDisplay(page)}`),
        ),
        revision: 9,
      }
      const closing = await page.evaluate(
        ({ next, sampleMotion }): ClosingSample | Promise<ClosingSample> => {
          const body = document.querySelector<HTMLElement>('[data-testid="island-body"]')!
          const header = document.querySelector<HTMLElement>('[data-testid="island-header"]')!
          const initial = {
            bodyStillMounted: document.querySelector('[data-testid="island-body"]') === body,
            opacity: getComputedStyle(body).opacity,
            header: header.getBoundingClientRect().toJSON(),
          }
          if (sampleMotion === "reduce") {
            window.__ANTIBURN_VISUAL_EMIT__!("hud-island:state", next)
            return initial
          }
          return new Promise((resolve) => {
            const onTransition = (event: Event) => {
              if (
                event.target !== body ||
                (event as TransitionEvent).propertyName !== "clip-path"
              )
                return
              document.removeEventListener("transitionrun", onTransition)
              const deadline = performance.now() + 800
              const sample = () => {
                const clip = getComputedStyle(body).clipPath
                const match = clip.match(/inset\([^ ]+ [^ ]+ ([\d.]+)(px|%)/)
                const bottom = match
                  ? Number(match[1]) * (match[2] === "%" ? body.clientHeight / 100 : 1)
                  : Number.NaN
                if (Number.isFinite(bottom) && bottom > 0 && bottom < body.clientHeight) {
                  resolve({
                    ...initial,
                    intermediateClip: true,
                    opacityStayedOpaque: getComputedStyle(body).opacity === "1",
                    bodyStillMounted:
                      document.querySelector('[data-testid="island-body"]') === body,
                  })
                  return
                }
                if (performance.now() >= deadline) {
                  resolve({ ...initial, intermediateClip: false, opacityStayedOpaque: false })
                  return
                }
                requestAnimationFrame(sample)
              }
              requestAnimationFrame(sample)
            }
            document.addEventListener("transitionrun", onTransition)
            window.__ANTIBURN_VISUAL_EMIT__!("hud-island:state", next)
          })
        },
        { next: collapsed, sampleMotion: motion },
      )
      expect(closing.bodyStillMounted).toBe(true)
      expect(closing.header.x).toBeCloseTo(headerOpen!.x, 2)
      expect(closing.header.width).toBeCloseTo(headerOpen!.width, 2)
      if (motion === "no-preference") {
        expect(closing.intermediateClip).toBe(true)
        expect(closing.opacityStayedOpaque).toBe(true)
      }
      await expect(body).toHaveCount(0)
      const headerClosed = await header.boundingBox()
      expect(headerClosed!.x).toBeCloseTo(headerOpen!.x, 2)
      expect(headerClosed!.y).toBeCloseTo(headerOpen!.y, 2)
      expect(headerClosed!.width).toBeCloseTo(headerOpen!.width, 2)
      expect(headerClosed!.height).toBeCloseTo(headerOpen!.height, 2)

      const expanded = {
        ...fixtureIsland(
          new URLSearchParams(`island=expanded&scale=100${locationSearchDisplay(page)}`),
        ),
        revision: 10,
      }
      await page.evaluate(
        (next) => window.__ANTIBURN_VISUAL_EMIT__!("hud-island:state", next),
        expanded,
      )
      await expect(body).toBeVisible()
      await expect(body).toHaveCSS("opacity", "1")
      if (motion === "no-preference")
        await expect(body).toHaveCSS("transform", "matrix(1, 0, 0, 1, 0, 0)")
      expect(await body.boundingBox()).toMatchObject({
        x: expect.closeTo(bodyBefore!.x, 2),
        y: expect.closeTo(bodyBefore!.y, 2),
        width: expect.closeTo(bodyBefore!.width, 2),
        height: expect.closeTo(bodyBefore!.height, 2),
      })

      const interrupted = await page.evaluate((next) => {
        const original = document.querySelector<HTMLElement>('[data-testid="island-body"]')!
        return new Promise<boolean>((resolve) => {
          window.__ANTIBURN_VISUAL_EMIT__!("hud-island:state", {
            ...next,
            island: "collapsed",
            revision: 11,
          })
          requestAnimationFrame(() => {
            const retained = document.querySelector('[data-testid="island-body"]') === original
            window.__ANTIBURN_VISUAL_EMIT__!("hud-island:state", { ...next, revision: 12 })
            requestAnimationFrame(() => resolve(retained))
          })
        })
      }, expanded)
      expect(interrupted).toBe(true)
      await expect(page.locator('[data-island="expanded"]')).toBeVisible()
      await expect(body).toHaveAttribute("data-open", "true")
      await expect(body).toHaveCSS("opacity", "1")
      await page.waitForTimeout(350)
      await expect(body).toBeVisible()
      await expect(body).toHaveAttribute("data-open", "true")
    })
  }
}

for (const short of [true, false] as const) {
  test(`12 usage bars scroll to the final reset with ${short ? "170" : "500"}px body cap`, async ({
    page,
  }, testInfo) => {
    const geometry = await openFixture(page, 12, "reduce", short)
    await emitPhase(page, "expanded", 8)
    const body = page.getByTestId("island-body")
    await expect(page.getByTestId("hud-bar-label")).toHaveCount(12)
    const before = await body.boundingBox()
    const bodyHeight = before!.height * geometry.scale
    expect(bodyHeight).toBeCloseTo(geometry.bodyMaxHeight, 0)
    const summaryPath = testInfo.outputPath("geometry-summary.json")
    await writeFile(
      summaryPath,
      JSON.stringify(
        {
          visibleBars: 12,
          motion: "reduce",
          bodyHeight,
          bodyMaxHeight: geometry.bodyMaxHeight,
          bodyScrollHeight: await body.evaluate((node) => node.scrollHeight),
        },
        null,
        2,
      ),
    )
    await testInfo.attach("geometry-summary", {
      path: summaryPath,
      contentType: "application/json",
    })
    expect(await body.evaluate((node) => node.scrollHeight > node.clientHeight)).toBe(true)
    await body.hover()
    await page.mouse.wheel(0, 10_000)
    await expect(page.getByTestId("hud-bar-reset").last()).toBeInViewport()
    const finalReset = await page.getByTestId("hud-bar-reset").last().boundingBox()
    expect(finalReset!.y + finalReset!.height).toBeLessThanOrEqual(before!.y + before!.height)
  })
}
