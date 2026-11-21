import { expect, test } from "@playwright/test"

import { fixtureIsland } from "./fakes/hud"

for (const reducedMotion of ["no-preference", "reduce"] as const) {
  test(`notch body enters and exits with ${reducedMotion} motion`, async ({ page }) => {
    await page.emulateMedia({ reducedMotion })
    await page.setViewportSize({ width: 500, height: 600 })
    await page.goto("/tests/visual/?surface=hud&island=collapsed&scale=100")
    await expect(page.getByTestId("island-header")).toBeVisible()
    const expanded = {
      ...fixtureIsland(new URLSearchParams("island=expanded&scale=100")),
      revision: 8,
    }
    const entered = await page.evaluate((state) => {
      return new Promise<boolean>((resolve) => {
        const deadline = window.setTimeout(() => resolve(false), 1000)
        document.addEventListener("transitionrun", (event) => {
          if (
            (event.target as HTMLElement).dataset.testid === "island-body" &&
            (event as TransitionEvent).propertyName === "opacity"
          ) {
            window.clearTimeout(deadline)
            resolve(true)
          }
        })
        window.__ANTIBURN_VISUAL_EMIT__!("hud-island:state", state)
      })
    }, expanded)
    expect(entered).toBe(true)
    const body = page.getByTestId("island-body")
    await expect(body).toHaveCSS("opacity", "1")
    await expect(body).toHaveCSS(
      "transition-duration",
      reducedMotion === "reduce" ? "0.06s" : "0.2s, 0.2s, 0.2s",
    )
    if (reducedMotion === "reduce") {
      await expect(body).toHaveCSS("transform", "none")
      await expect(body).toHaveCSS("clip-path", "none")
    } else {
      await expect(body).toHaveCSS("clip-path", "inset(0px round 0px 0px 24px 24px)")
    }
    const header = await page.getByTestId("island-header").boundingBox()
    const collapsed = {
      ...fixtureIsland(new URLSearchParams("island=collapsed&scale=100")),
      revision: 9,
    }
    const closing = await page.evaluate((state) => {
      return new Promise<{ present: boolean; width: number; duration: string }>((resolve) => {
        const observer = new MutationObserver(() => {
          const body = document.querySelector<HTMLElement>('[data-testid="island-body"]')
          if (!body?.classList.contains("hud-island-body-closed")) return
          observer.disconnect()
          const header = document.querySelector<HTMLElement>('[data-testid="island-header"]')!
          resolve({
            present: true,
            width: header.getBoundingClientRect().width,
            duration: getComputedStyle(body).transitionDuration,
          })
        })
        observer.observe(document.body, {
          subtree: true,
          attributes: true,
          attributeFilter: ["class"],
        })
        window.__ANTIBURN_VISUAL_EMIT__!("hud-island:state", state)
      })
    }, collapsed)
    expect(closing.present).toBe(true)
    expect(closing.width).toBeCloseTo(header!.width, 1)
    if (reducedMotion === "reduce") expect(closing.duration).toBe("0.06s")
    await expect
      .poll(() =>
        page.evaluate(() =>
          window.__ANTIBURN_VISUAL_HUD_RESIZES__?.some(
            (request) => request.geometryRevision === 9 && Number(request.height) > 33,
          ),
        ),
      )
      .toBe(true)
    await expect(body).toHaveCount(0)
    await expect
      .poll(() =>
        page.evaluate(() =>
          window.__ANTIBURN_VISUAL_HUD_RESIZES__?.some(
            (request) =>
              request.geometryRevision === 9 &&
              Number(request.height) <= 33 &&
              request.bodyPresent === false,
          ),
        ),
      )
      .toBe(true)
    const shortWhileBodyPresent = await page.evaluate(
      () =>
        window.__ANTIBURN_VISUAL_HUD_RESIZES__?.filter(
          (request) =>
            request.geometryRevision === 9 &&
            Number(request.height) <= 33 &&
            request.bodyPresent === true,
        ).length ?? 0,
    )
    expect(shortWhileBodyPresent).toBe(0)
  })
}

test("notch exit reverses without replacing the body", async ({ page }) => {
  await page.setViewportSize({ width: 500, height: 600 })
  await page.goto("/tests/visual/?surface=hud&island=expanded&scale=100")
  const body = page.getByTestId("island-body")
  await expect(body).toHaveCSS("opacity", "1")
  const collapsed = {
    ...fixtureIsland(new URLSearchParams("island=collapsed&scale=100")),
    revision: 8,
  }
  const expanded = {
    ...fixtureIsland(new URLSearchParams("island=expanded&scale=100")),
    revision: 9,
  }
  const retained = await page.evaluate(
    ({ collapsed, expanded }) => {
      const original = document.querySelector('[data-testid="island-body"]')!
      return new Promise<boolean>((resolve) => {
        const deadline = window.setTimeout(() => {
          original.removeEventListener("transitionrun", reverse)
          resolve(false)
        }, 1000)
        const reverse = () => {
          window.clearTimeout(deadline)
          window.__ANTIBURN_VISUAL_EMIT__!("hud-island:state", expanded)
          resolve(document.querySelector('[data-testid="island-body"]') === original)
        }
        original.addEventListener("transitionrun", reverse, { once: true })
        requestAnimationFrame(() =>
          requestAnimationFrame(() => {
            window.__ANTIBURN_VISUAL_EMIT__!("hud-island:state", collapsed)
          }),
        )
      })
    },
    { collapsed, expanded },
  )
  expect(retained).toBe(true)
  await expect(body).toHaveCSS("opacity", "1")
  await expect(page.locator("[data-island]")).toHaveAttribute("data-island", "expanded")
})

test("closing reentry follows the visible clip instead of the retained frame", async ({
  page,
}) => {
  await page.setViewportSize({ width: 500, height: 600 })
  await page.goto("/tests/visual/?surface=hud&island=expanded&scale=100")
  const body = page.getByTestId("island-body")
  await expect(body).toHaveCSS("opacity", "1")
  const collapsed = {
    ...fixtureIsland(new URLSearchParams("island=collapsed&scale=100")),
    revision: 8,
  }
  const points = await page.evaluate((next) => {
    const body = document.querySelector<HTMLElement>('[data-testid="island-body"]')!
    const original = body.getBoundingClientRect()
    window.__ANTIBURN_VISUAL_HUD_DRAGS__ = []
    return new Promise<{ x: number; visibleY: number; hiddenY: number }>((resolve) => {
      const observer = new MutationObserver(() => {
        if (!body.classList.contains("hud-island-body-closed")) return
        observer.disconnect()
        body.style.transition = "none"
        body.style.clipPath = "inset(0 0 50% 0)"
        body.style.transform = "none"
        body.style.opacity = "1"
        resolve({
          x: original.x + original.width / 2,
          visibleY: original.y + original.height / 4,
          hiddenY: original.y + (original.height * 3) / 4,
        })
      })
      observer.observe(body, { attributes: true, attributeFilter: ["class"] })
      window.__ANTIBURN_VISUAL_EMIT__!("hud-island:state", next)
    })
  }, collapsed)
  await page.mouse.move(points.x, points.hiddenY)
  expect(await page.evaluate(() => window.__ANTIBURN_VISUAL_HUD_DRAGS__ ?? [])).toEqual([])
  await page.mouse.move(points.x, points.visibleY)
  await expect
    .poll(() => page.evaluate(() => window.__ANTIBURN_VISUAL_HUD_DRAGS__ ?? []))
    .toContain("expand_hud_island")
})
