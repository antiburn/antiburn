import { expect, test } from "@playwright/test"

for (const theme of ["light", "dark"] as const) {
  test(`remote hosts support focused editing in ${theme}`, async ({ page }, testInfo) => {
    await page.clock.setFixedTime(new Date("2026-09-15T00:00:00Z"))
    await page.emulateMedia({ colorScheme: theme })
    await page.goto(`/tests/visual/?surface=settings&remote=1&theme=${theme}`)
    await page.getByRole("tab", { name: "Sources", exact: true }).click()
    await expect(page.getByText("Remote hosts", { exact: true })).toBeVisible()
    await expect(page.getByRole("heading", { level: 2 }).first()).toHaveText("Remote hosts")
    const hostsCard = page.locator(".remote-hosts-card")
    const syncBlock = hostsCard.locator(":scope > *").first()
    await expect(
      syncBlock.getByText(
        "Sync sessions from your other computers over SSH and browse them alongside your local sessions. Synced sessions are saved on this computer so you can view them offline.",
      ),
    ).toBeVisible()
    await expect(syncBlock.getByRole("combobox", { name: "Sync frequency" })).toBeVisible()
    await expect(
      syncBlock.getByText("Enabled hosts sync automatically while Antiburn is running."),
    ).toBeVisible()
    const footer = hostsCard.locator(":scope > *").last()
    await expect(footer.getByRole("button", { name: "Add host", exact: true })).toBeVisible()
    await expect(footer.locator("p")).toHaveCount(0)
    const geometry = await hostsCard.evaluate((card) => {
      const dropdown = card.querySelector("select")!.getBoundingClientRect()
      const button = card.querySelector(".remote-host-toggle")!.getBoundingClientRect()
      const row = card.querySelector(".remote-sync-row")!
      const label = row.querySelector("p")!.getBoundingClientRect()
      const helper = row.querySelector("p:nth-of-type(2)")!.getBoundingClientRect()
      const title = card.querySelector(".remote-host-name")!.getBoundingClientRect()
      const metadata = card.querySelector(".remote-host-metadata")!.getBoundingClientRect()
      const actions = card.querySelector(".remote-host-actions")!.getBoundingClientRect()
      return {
        right: Math.abs(dropdown.right - button.right),
        center: Math.abs(
          (dropdown.top + dropdown.bottom) / 2 - (label.top + helper.bottom) / 2,
        ),
        identityCenter: Math.abs(
          (title.top + metadata.bottom - actions.top - actions.bottom) / 2,
        ),
        actionRight: Math.abs(actions.right - button.right),
        buttonGap:
          card.querySelector(".remote-host-overflow")!.getBoundingClientRect().left -
          card.querySelector(".remote-host-sync")!.getBoundingClientRect().right,
        toggleGap:
          button.left -
          card.querySelector(".remote-host-overflow")!.getBoundingClientRect().right,
        metadataGap: metadata.top - title.bottom,
        linkColor: getComputedStyle(card.querySelector(".remote-host-sessions")!).color,
        primaryColor: getComputedStyle(card.querySelector(".remote-host-name")!).color,
        separatorCount: card
          .querySelector(".remote-host-metadata")!
          .querySelectorAll('[aria-hidden="true"]').length,
        statusIcons: card.querySelector(".remote-host-metadata")!.querySelectorAll("svg")
          .length,
      }
    })
    expect(geometry.right).toBeLessThan(0.5)
    expect(geometry.center).toBeLessThan(0.5)
    expect(geometry.identityCenter).toBeLessThan(0.5)
    expect(geometry.actionRight).toBeLessThan(0.5)
    expect(geometry.buttonGap).toBe(12)
    expect(geometry.toggleGap).toBe(geometry.buttonGap)
    expect(geometry.metadataGap).toBe(2)
    expect(geometry.linkColor).toBe(geometry.primaryColor)
    expect(geometry.separatorCount).toBe(1)
    expect(geometry.statusIcons).toBe(0)
    await expect(hostsCard.getByText("Auto-sync", { exact: true })).toHaveCount(0)
    await expect(hostsCard.locator(".remote-host-alias").first()).toHaveClass(
      /bg-surface-tertiary\/40/,
    )
    await expect(syncBlock.getByText("Sync frequency", { exact: true })).toBeVisible()
    await expect(
      page.getByRole("heading", { name: "Automatic sync", exact: true }),
    ).toHaveCount(0)
    await expect(page.getByText("SSH connection timed out", { exact: false })).toBeVisible()
    const automaticSync = page.getByRole("switch", {
      name: "Sync for Studio Linux",
      exact: true,
    })
    await expect(automaticSync).toBeChecked()
    await automaticSync.click()
    await expect(automaticSync).not.toBeChecked()
    const paused = page.locator(".remote-host-metadata [tabindex='0']")
    await expect(paused).toContainText("Sync off")
    await paused.focus()
    await expect(page.getByRole("tooltip")).toContainText("Synced")
    await page.keyboard.press("Escape")
    await expect(page.getByRole("button", { name: "Sync now", exact: true })).toBeDisabled()
    await automaticSync.click()
    await expect(automaticSync).toBeChecked()
    await page.getByRole("button", { name: "More actions for Studio Linux" }).click()
    await page.getByRole("menuitem", { name: "Edit", exact: true }).click()
    const dialog = page.getByRole("dialog", { name: "Edit remote host" })
    await expect(dialog.getByRole("textbox", { name: "Name (optional)" })).toBeFocused()
    await dialog.getByRole("textbox", { name: "Name (optional)" }).fill("Studio server")
    await page.keyboard.press("Escape")
    await expect(dialog).toBeVisible()
    await expect(dialog.getByRole("alert")).toContainText("Use Cancel")
    await page.screenshot({ path: testInfo.outputPath(`remote-edit-${theme}.png`) })
    await dialog.getByRole("button", { name: "Save changes" }).click()
    await expect(dialog).not.toBeVisible()
    await expect(
      page.getByRole("button", { name: "More actions for Studio server" }),
    ).toBeFocused()
    await page.getByRole("button", { name: "Add host", exact: true }).click()
    const add = page.getByRole("dialog", { name: "Add remote host" })
    await add.getByRole("textbox", { name: "SSH host alias" }).fill("dev-box")
    await add.getByRole("button", { name: "Check connection" }).click()
    await expect(add.getByRole("status")).toContainText("ARM64")
    await add.getByRole("button", { name: "Add host", exact: true }).click()
    await expect(page.getByRole("button", { name: "More actions for dev-box" })).toBeVisible()
    await page.screenshot({ path: testInfo.outputPath(`remote-hosts-${theme}.png`) })
  })

  test(`remote facets combine hosts and preserve local selection in ${theme}`, async ({
    page,
  }, testInfo) => {
    await page.clock.setFixedTime(new Date("2026-09-15T00:00:00Z"))
    await page.emulateMedia({ colorScheme: theme })
    await page.goto(`/tests/visual/?surface=main&remote=1&theme=${theme}`)
    await page.getByRole("tab", { name: "Sessions", exact: true }).click()
    const rows = page
      .getByRole("tabpanel", { name: "Sessions", exact: true })
      .locator("[data-session-row]:visible")
    await expect(rows).toHaveCount(4)
    await expect(
      page.getByRole("img", { name: "Remote session from Studio Linux" }),
    ).toBeVisible()
    const iconOffset = await page
      .getByRole("img", { name: "Remote session from Studio Linux" })
      .evaluate((icon) => {
        const model = icon.parentElement!.querySelector(":scope > span")!
        const iconBox = icon.getBoundingClientRect()
        const modelBox = model.getBoundingClientRect()
        return Math.abs(iconBox.y + iconBox.height / 2 - modelBox.y - modelBox.height / 2)
      })
    expect(iconOffset).toBeLessThan(0.5)
    await page.getByRole("button", { name: "Filters", exact: true }).click()
    const remoteParent = page.getByRole("menuitemcheckbox", { name: /^Remote,/ })
    const studioHost = page.getByRole("menuitemcheckbox", { name: /^Studio Linux,/ })
    await expect(remoteParent.locator(":scope > svg")).toHaveCount(1)
    await expect(studioHost.locator(":scope > svg")).toHaveCount(0)
    const [parentIndicatorX, hostIndicatorX, allLabelX, localLabelX] = await page
      .getByRole("menu")
      .evaluate((menu) => {
        const row = (name: string) =>
          menu.querySelector(`[role="menuitemcheckbox"][aria-label^="${name},"]`)!
        const indicatorX = (name: string) =>
          row(name).querySelector(":scope > span")!.getBoundingClientRect().x
        const labelX = (name: string) =>
          Array.from(row(name).querySelectorAll("span"))
            .find((span) => span.textContent === name)!
            .getBoundingClientRect().x
        return [
          indicatorX("Remote"),
          indicatorX("Studio Linux"),
          labelX("All sources"),
          labelX("Local"),
        ]
      })
    expect(hostIndicatorX).toBeGreaterThan(parentIndicatorX + 20)
    expect(Math.abs(localLabelX - allLabelX)).toBeLessThan(0.5)
    await studioHost.click()
    await expect(remoteParent).toHaveAttribute("aria-checked", "mixed")
    await expect(
      page.getByRole("button", { name: "Remove Studio Linux filter" }),
    ).toContainText("Studio Linux")
    await page.getByRole("menuitemcheckbox", { name: /^Build server,/ }).click()
    await expect(page.getByRole("menuitemcheckbox", { name: /^Remote,/ })).toHaveAttribute(
      "aria-checked",
      "true",
    )
    await page.getByRole("menuitemcheckbox", { name: /^Local,/ }).click()
    await page.screenshot({ path: testInfo.outputPath(`remote-facets-${theme}.png`) })
    await page.getByRole("menuitemcheckbox", { name: /^Remote,/ }).click()
    await expect(page.getByRole("menuitemcheckbox", { name: /^Local,/ })).toHaveAttribute(
      "aria-checked",
      "true",
    )
    await page.keyboard.press("Escape")
    await expect(rows).toHaveCount(2)
    await expect(page.getByRole("img", { name: /Remote session from/ })).toHaveCount(0)
  })

  for (const scale of [100, 200]) {
    test.describe(`connection check at ${scale}% in ${theme}`, () => {
      test.use({
        viewport: {
          width: Math.round(1280 / (scale / 100)),
          height: Math.round(860 / (scale / 100)),
        },
        deviceScaleFactor: scale / 100,
      })
      test(`remote check actions stay fixed in ${theme} at ${scale}%`, async ({
        page,
      }, testInfo) => {
        await page.emulateMedia({ colorScheme: theme })
        await page.goto(
          `/tests/visual/?surface=settings&remote=1&remoteCheck=pending&theme=${theme}&scale=${scale}`,
        )
        await expect(page.getByRole("tabpanel", { name: "General" })).toBeVisible()
        const navigation = page.getByRole("button", { name: "Open Settings navigation" })
        if (await navigation.isVisible()) await navigation.click()
        await page.getByRole("tab", { name: "Sources", exact: true }).click()
        await page.screenshot({
          path: testInfo.outputPath(`remote-settings-${theme}-${scale}.png`),
          animations: "disabled",
        })
        if (scale === 200) {
          await page.setViewportSize({ width: 400, height: 720 })
          const geometry = await page
            .locator(".remote-host-row")
            .first()
            .evaluate((row) => {
              const title = row.querySelector(".remote-host-name")!.getBoundingClientRect()
              const metadata = row
                .querySelector(".remote-host-metadata")!
                .getBoundingClientRect()
              const actions = row.querySelector(".remote-host-actions")!.getBoundingClientRect()
              return {
                left: Math.abs(title.left - metadata.left),
                below: actions.top >= metadata.bottom,
                overflow: row.scrollWidth > row.clientWidth,
              }
            })
          expect(geometry.left).toBeLessThan(0.5)
          expect(geometry.below).toBe(true)
          expect(geometry.overflow).toBe(false)
          const alias = page.locator(".remote-host-alias").first()
          await alias.locator("span").evaluate((text) => {
            text.textContent = "engineering-worker-eu-central-1-staging"
          })
          const aliasFits = await alias.evaluate((badge) => {
            const metadata = badge.closest(".remote-host-metadata")!
            return (
              badge.getBoundingClientRect().right <= metadata.getBoundingClientRect().right &&
              metadata.scrollWidth <= metadata.clientWidth
            )
          })
          expect(aliasFits).toBe(true)
          await page.screenshot({
            path: testInfo.outputPath(`remote-hosts-narrow-${theme}.png`),
          })
        }
        await page.getByRole("button", { name: "Add host", exact: true }).click()
        const dialog = page.getByRole("dialog", { name: "Add remote host" })
        await dialog.getByRole("textbox", { name: "SSH host alias" }).fill("test-host")
        const cancel = dialog.getByRole("button", { name: "Cancel", exact: true })
        const check = dialog.getByRole("button", { name: "Check connection", exact: true })
        const cancelBefore = (await cancel.boundingBox())!
        const primaryBefore = (await check.boundingBox())!
        await check.click()
        const busy = dialog.getByRole("button", { name: "Checking…", exact: true })
        await expect(busy).toBeDisabled()
        await expect(cancel).toHaveCount(1)
        await expect(cancel).toBeDisabled()
        expect(await cancel.boundingBox()).toEqual(cancelBefore)
        expect(await busy.boundingBox()).toEqual(primaryBefore)
        await page.screenshot({
          path: testInfo.outputPath(`remote-checking-${theme}-${scale}.png`),
        })
        await page.evaluate(() => window.__ANTIBURN_VISUAL_FINISH_REMOTE_CHECK__?.())
        const add = dialog.getByRole("button", { name: "Add host", exact: true })
        await expect(add).toBeEnabled()
        expect(await cancel.boundingBox()).toMatchObject({
          x: cancelBefore.x,
          width: cancelBefore.width,
          height: cancelBefore.height,
        })
        expect(await add.boundingBox()).toMatchObject({
          x: primaryBefore.x,
          width: primaryBefore.width,
          height: primaryBefore.height,
        })
      })
    })
  }
}
