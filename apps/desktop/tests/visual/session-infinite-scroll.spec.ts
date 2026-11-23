import { expect, test, type Locator } from "@playwright/test"

test("the empty fixture starts with no session summaries", async ({ page }) => {
  await page.goto("/tests/visual/?surface=main&onboarding=complete&state=empty")
  await page.getByRole("tab", { name: "Sessions", exact: true }).click()
  const pane = page.getByRole("tabpanel", { name: "Sessions", exact: true })
  await expect(pane.getByText("0 total sessions, 0 matching", { exact: true })).toBeAttached()
  await expect(
    pane.getByRole("button", { name: "Change time range", exact: true }),
  ).toBeVisible()
})

function sessionsViewport(pane: Locator) {
  return pane
    .getByRole("region", { name: "Sessions", exact: true })
    .locator(".ui-scroll-viewport")
}

function scrollToBottom(viewport: Locator) {
  return viewport.evaluate((element) => {
    element.scrollTop = element.scrollHeight
  })
}

test("scrolling the virtualized Sessions list requests rows beyond 500", async ({ page }) => {
  await page.clock.setFixedTime(new Date("2026-09-15T00:00:00Z"))
  await page.goto("/tests/visual/?surface=main&onboarding=complete&infinite=1")
  await page.getByRole("tab", { name: "Sessions", exact: true }).click()
  const pane = page.getByRole("tabpanel", { name: "Sessions", exact: true })
  const viewport = sessionsViewport(pane)
  const rows = viewport.locator('[data-virtual-kind="row"]')
  const totalBadge = pane.getByText("551", { exact: true })
  await expect(rows.first()).toContainText("Streamed session 1")
  await expect(rows.first()).toHaveAttribute("aria-setsize", "50")
  await expect(totalBadge).toBeVisible()

  for (let loaded = 100; loaded <= 550; loaded += 50) {
    await scrollToBottom(viewport)
    await expect(rows.last()).toHaveAttribute("aria-setsize", String(loaded))
    if (loaded === 100) await expect(totalBadge).toBeVisible()
  }
  await scrollToBottom(viewport)
  await expect(rows.last()).toHaveAttribute("aria-setsize", "551")
  await scrollToBottom(viewport)
  await expect(rows.last()).toContainText("Streamed session 551")
  await expect(totalBadge).toBeVisible()
  expect(await rows.count()).toBeLessThan(30)
  await expect(pane.getByRole("button", { name: "Find older matches" })).toHaveCount(0)
})

test("slow checks preserve matches and older-only agents stay selectable", async ({ page }) => {
  await page.clock.setFixedTime(new Date("2026-09-15T00:00:00Z"))
  await page.goto("/tests/visual/?surface=main&onboarding=complete&infinite=1&slow-hygiene=1")
  await page.getByRole("tab", { name: "Sessions", exact: true }).click()
  const pane = page.getByRole("tabpanel", { name: "Sessions", exact: true })
  const viewport = sessionsViewport(pane)
  const rows = viewport.locator('[data-virtual-kind="row"]')
  await expect(rows.first()).toHaveAttribute("aria-setsize", "50")
  await pane.getByRole("button", { name: "Filters", exact: true }).click()
  await expect(
    page.getByRole("menuitemcheckbox", { name: /^Claude Code, 0 matching/ }),
  ).toBeEnabled()
  await page.getByRole("menuitemradio", { name: /^Failed, 50 matching/ }).click()
  await page.keyboard.press("Escape")

  await scrollToBottom(viewport)
  await expect(
    pane.getByText("100 loaded sessions, 50 matching", { exact: true }),
  ).toBeAttached()
  await expect(rows.last()).toHaveAttribute("aria-setsize", "50")
  await expect(
    pane.getByText("100 loaded sessions, 100 matching", { exact: true }),
  ).toBeAttached()
})

test("one scroll searches three batches before offering to find older matches", async ({
  page,
}) => {
  await page.clock.setFixedTime(new Date("2026-09-15T00:00:00Z"))
  await page.goto(
    "/tests/visual/?surface=main&onboarding=complete&infinite=1&slow-hygiene=sparse",
  )
  await page.getByRole("tab", { name: "Sessions", exact: true }).click()
  const pane = page.getByRole("tabpanel", { name: "Sessions", exact: true })
  const viewport = sessionsViewport(pane)
  await pane.getByRole("button", { name: "Filters", exact: true }).click()
  await page.getByRole("menuitemradio", { name: /^Failed, 50 matching/ }).click()
  await page.keyboard.press("Escape")
  await scrollToBottom(viewport)
  const count = pane.getByText("200 loaded sessions, 50 matching", { exact: true })
  await expect(count).toBeAttached()
  await pane.getByRole("button", { name: "Filters", exact: true }).click()
  await expect(
    page.getByRole("menuitemradio", { name: /^Passed, 150 matching/ }),
  ).toBeAttached()
  await page.keyboard.press("Escape")
  await expect(count).toBeAttached()
  await pane.getByRole("button", { name: "Find older matches" }).click()
  await expect(
    pane.getByText("350 loaded sessions, 50 matching", { exact: true }),
  ).toBeAttached()
  await expect(pane.getByRole("button", { name: "Find older matches" })).toBeEnabled()
})

test("offers manual recovery after an append and its automatic retry fail", async ({
  page,
}) => {
  await page.clock.setFixedTime(new Date("2026-09-15T00:00:00Z"))
  await page.goto(
    "/tests/visual/?surface=main&onboarding=complete&infinite=1&stream-failures=2",
  )
  const pane = page.getByRole("tabpanel", { name: "Sessions", exact: true })
  const viewport = sessionsViewport(pane)
  const firstBatch = pane.getByText("50 loaded sessions, 50 matching", { exact: true })
  await expect(firstBatch).toBeAttached()
  await scrollToBottom(viewport)

  const retry = pane.getByRole("button", { name: "Retry loading sessions" })
  await expect(retry).toBeVisible()
  await expect(firstBatch).toBeAttached()
  await retry.click()
  await expect(
    pane.getByText("100 loaded sessions, 100 matching", { exact: true }),
  ).toBeAttached()
  await expect(retry).toHaveCount(0)
})

for (const { target, agent, key, expected, retry } of [
  { target: 50, agent: "codex", key: "ArrowRight", expected: 51 },
  { target: 75, agent: "claude-code", key: "ArrowRight", expected: 76 },
  { target: 75, agent: "claude-code", key: "ArrowLeft", expected: 74 },
  { target: 75, agent: "claude-code", key: "ArrowRight", expected: 76, retry: true },
]) {
  test(`${key} opens session ${expected} from compact detail for session ${target}${retry ? " after retrying" : ""}`, async ({
    page,
  }) => {
    await page.setViewportSize({ width: 600, height: 650 })
    await page.clock.setFixedTime(new Date("2026-09-15T00:00:00Z"))
    await page.goto(
      `/tests/visual/?surface=main&onboarding=complete&infinite=1${retry ? "&stream-failures=2" : ""}`,
    )
    const pane = page.getByRole("tabpanel", { name: "Sessions", exact: true })
    await expect(
      pane.getByText("50 loaded sessions, 50 matching", { exact: true }),
    ).toBeAttached()

    await page.evaluate(
      ({ agent, target }) => {
        window.__ANTIBURN_VISUAL_EMIT__?.("main:navigation-target", {
          revision: 1,
          destination: {
            section: "activity",
            target: { agent, sessionId: `infinite-${target}`, wslDistro: null },
          },
        })
      },
      { agent, target },
    )
    await expect(page.getByRole("button", { name: "Back to sessions" })).toBeVisible()
    await expect(page.getByText("This item is outside the current list.")).toHaveCount(0)
    const title = page.locator("[data-detail-pane] h2[data-view-heading]")
    await title.press(key)
    if (retry) {
      await expect(
        page.getByText("Could not load more sessions. Press Left or Right Arrow to retry."),
      ).toBeVisible()
      await title.press(key)
    }
    await expect(title).toHaveText(`Streamed session ${expected}`)
    await expect(
      pane.getByText("100 loaded sessions, 100 matching", { exact: true }),
    ).toBeAttached()
  })
}
