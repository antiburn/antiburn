import { readFileSync } from "node:fs"
import { createHash } from "node:crypto"
import { expect, test, type Page } from "@playwright/test"

const config = JSON.parse(
  readFileSync(new URL("../../src-tauri/tauri.conf.json", import.meta.url), "utf8"),
) as { app: { security: { csp: string } } }

/** Apply the shipping policy to every document, as the shared Linux handler does. */
async function serveSharedPolicy(page: Page, origin: string) {
  await page.route(`${origin}/**`, async (route) => {
    const url = new URL(route.request().url())
    const response = await route.fetch({
      url: `http://127.0.0.1:4174${url.pathname}${url.search}`,
    })
    const headers: Record<string, string> = {
      ...response.headers(),
      "access-control-allow-origin": origin,
    }
    if (headers["content-type"]?.includes("text/html")) {
      const body = await response.text()
      // Tauri hashes inline scripts. The Vite fixture also needs its React preamble.
      const hashes = [...body.matchAll(/<script\b[^>]*>([\s\S]*?)<\/script>/gi)]
        .filter((match) => match[1]?.trim())
        .map((match) => `'sha256-${createHash("sha256").update(match[1]!).digest("base64")}'`)
      headers["content-security-policy"] = config.app.security.csp.replace(
        "script-src",
        `script-src ${hashes.join(" ")}`,
      )
      await route.fulfill({ response, headers, body })
    } else await route.fulfill({ response, headers })
  })
}

for (const protocol of ["http", "https"] as const) {
  test(`shared CSP preserves isolated examples over ${protocol}`, async ({ page }) => {
    const origin = `${protocol}://tauri.localhost`
    await serveSharedPolicy(page, origin)
    await page.setViewportSize({ width: 860, height: 620 })
    await page.emulateMedia({ colorScheme: "dark", reducedMotion: "reduce" })
    const errors: string[] = []
    page.on("pageerror", (error) => errors.push(error.message))
    await page.goto(`${origin}/tests/visual/?surface=onboarding&theme=dark`)
    await expect(page.locator("iframe")).toHaveCount(0)
    await page.getByRole("button", { name: "Get started", exact: true }).click()
    const frame = page.locator("iframe")
    const demo = page.frameLocator("iframe")
    for (const scene of ["sessions-scene", "limits-scene", "hud-scene", "checks-scene"]) {
      await expect(frame).toHaveAttribute("data-ready", "true")
      await expect(demo.locator(`.${scene}`)).toBeVisible()
      await expect(page.getByText(/example could not load/i)).toHaveCount(0)
      if (scene !== "checks-scene")
        await page.getByRole("button", { name: "Continue", exact: true }).click()
    }
    await expect(frame).toHaveAttribute("sandbox", "allow-scripts")
    const isolation = await demo.locator("html").evaluate(() => {
      let parentBlocked = false
      try {
        void parent.document
      } catch {
        parentBlocked = true
      }
      return {
        parentBlocked,
        nativeBridge: "__TAURI_INTERNALS__" in window,
      }
    })
    expect(isolation).toEqual({ parentBlocked: true, nativeBridge: false })
    await page.route("http://ipc.localhost/onboarding-policy-probe", (route) =>
      route.fulfill({ status: 200, body: "", headers: { "access-control-allow-origin": "*" } }),
    )
    const connectionBlocked = await demo.locator("html").evaluate(
      () =>
        new Promise<boolean>((resolve) => {
          const finish = (blocked: boolean) => {
            clearTimeout(timeout)
            document.removeEventListener("securitypolicyviolation", onViolation)
            resolve(blocked)
          }
          const onViolation = (event: SecurityPolicyViolationEvent) => {
            if (
              event.effectiveDirective === "connect-src" &&
              (event.blockedURI === "http://ipc.localhost" ||
                event.blockedURI.startsWith("http://ipc.localhost/"))
            )
              finish(true)
          }
          const timeout = setTimeout(() => finish(false), 1000)
          document.addEventListener("securitypolicyviolation", onViolation)
          void fetch("http://ipc.localhost/onboarding-policy-probe").catch(() => undefined)
        }),
    )
    expect(connectionBlocked).toBe(true)
    await page.evaluate(() => window.__ANTIBURN_VISUAL_EMIT__?.("onboarding:visibility", false))
    await expect(demo.locator("#root")).toBeEmpty()
    await page.evaluate(() => window.__ANTIBURN_VISUAL_EMIT__?.("onboarding:visibility", true))
    await expect(demo.locator(".checks-scene")).toBeVisible()
    await page.getByRole("button", { name: "Back", exact: true }).click()
    await expect(page.frameLocator("iframe").locator(".hud-scene")).toBeVisible()
    expect(errors).toEqual([])
  })
}
