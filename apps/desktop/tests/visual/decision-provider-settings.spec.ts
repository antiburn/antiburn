import { expect, test, type Page } from "@playwright/test"
import type { ProviderSettings } from "../../src/lib/smartCheckProviders"

async function openChecks(page: Page) {
  await expect(page.getByRole("heading", { name: "General", exact: true })).toBeVisible({
    timeout: 30_000,
  })
  const navigation = page.getByRole("button", { name: "Open Settings navigation" })
  if (await navigation.isVisible()) await navigation.click()
  await page.getByRole("tab", { name: "Checks", exact: true }).click()
  return page.getByRole("tabpanel", { name: "Checks", exact: true })
}
async function providerCalls(page: Page) {
  return page.evaluate(() => window.__ANTIBURN_VISUAL_PROVIDER_CALLS__ ?? [])
}
async function connectionAction(page: Page, label: string, action: "Edit" | "Remove") {
  const trigger = page.getByRole("button", { name: `More actions for ${label}`, exact: true })
  await trigger.click()
  await expect(trigger).toHaveAttribute("aria-expanded", "true")
  await page.getByRole("menuitem", { name: action, exact: true }).click()
  return trigger
}
async function readOnlyBaseline(page: Page, ready = true) {
  if (ready)
    await expect(
      page.getByRole("button", { name: "Add connection", exact: true }),
    ).toBeEnabled()
  const calls = await providerCalls(page)
  expect(calls.length).toBeGreaterThan(0)
  expect(calls.every((command) => command === "get_system_one_settings")).toBe(true)
  return calls
}
async function expectNoOverflow(page: Page) {
  expect(
    await page.locator("#root").evaluate((node) => node.scrollWidth - node.clientWidth),
  ).toBeLessThanOrEqual(1)
  const dialog = page.getByRole("dialog")
  if (await dialog.count()) {
    expect(
      await dialog.evaluate((node) => node.scrollWidth - node.clientWidth),
    ).toBeLessThanOrEqual(1)
    const save = dialog.getByRole("button", { name: /^(Save|Add connection)$/ })
    await save.scrollIntoViewIfNeeded()
    await expect(save).toBeInViewport()
  }
}

for (const theme of ["light", "dark"] as const) {
  for (const layout of [
    { name: "normal", width: 1100, height: 800, scale: 100 },
    { name: "constrained", width: 640, height: 600, scale: 200 },
  ]) {
    test.describe(`${theme}, ${layout.name}`, () => {
      const factor = layout.scale / 100
      test.use({
        viewport: {
          width: Math.round(layout.width / factor),
          height: Math.round(layout.height / factor),
        },
        deviceScaleFactor: factor,
        colorScheme: theme,
      })

      test("visible connections and in-card disclosures with Add and Edit previews", async ({
        page,
      }, testInfo) => {
        await page.goto(
          `/tests/visual/?surface=settings&theme=${theme}&scale=${layout.scale}&directories=long`,
        )
        const pane = await openChecks(page)
        const baseline = await readOnlyBaseline(page)
        const section = pane.locator("section").filter({
          has: page.getByRole("heading", { name: "Model-based checks", exact: true }),
        })
        const card = section.locator(".rounded-popover")
        await expect(card).toHaveCount(1)
        await expect(card.getByRole("listitem")).toHaveCount(4)
        await expect(
          card
            .getByRole("listitem", { name: "Jev", exact: true })
            .getByText("Selected", { exact: true }),
        ).toBeVisible()
        await expect(
          card.getByRole("button", { name: "Add connection", exact: true }),
        ).toBeVisible()
        await expect(
          card.getByRole("button", { name: "Check files", exact: true }),
        ).toHaveAttribute("aria-expanded", "false")
        await expect(
          card.getByRole("button", { name: "Model usage", exact: true }),
        ).toHaveAttribute("aria-expanded", "false")
        await expect(page.getByLabel("Provider", { exact: true })).toHaveCount(0)
        await section
          .getByRole("heading", { name: "Model-based checks", exact: true })
          .evaluate((node) => node.scrollIntoView({ block: "start" }))
        await page.screenshot({
          path: testInfo.outputPath(`checks-existing-${theme}-${layout.scale}.png`),
          animations: "disabled",
        })
        await card.getByRole("button", { name: "Add connection", exact: true }).click()
        let dialog = page.getByRole("dialog", { name: "Add connection", exact: true })
        await expect(dialog.getByLabel("Provider", { exact: true })).toBeFocused()
        await expectNoOverflow(page)
        await dialog.getByLabel("Provider", { exact: true }).scrollIntoViewIfNeeded()
        await page.screenshot({
          path: testInfo.outputPath(`checks-add-${theme}-${layout.scale}.png`),
          animations: "disabled",
        })
        if (layout.scale === 200) {
          await dialog
            .getByRole("button", { name: "Add connection", exact: true })
            .scrollIntoViewIfNeeded()
          await page.screenshot({
            path: testInfo.outputPath(`checks-add-${theme}-200-footer.png`),
            animations: "disabled",
          })
        }
        await page.keyboard.press("Escape")
        await expect(dialog).toHaveCount(0)
        await expect(
          card.getByRole("button", { name: "Add connection", exact: true }),
        ).toBeFocused()
        const menuTrigger = card.getByRole("button", {
          name: "More actions for Cloudflare · Clef",
          exact: true,
        })
        await menuTrigger.click()
        await expect(menuTrigger).toHaveAttribute("aria-expanded", "true")
        await expect(page.getByRole("menuitem", { name: "Edit", exact: true })).toBeVisible()
        await expect(page.getByRole("menuitem", { name: "Remove", exact: true })).toBeVisible()
        await page.screenshot({
          path: testInfo.outputPath(`checks-menu-${theme}-${layout.scale}.png`),
          animations: "disabled",
        })
        await expect(menuTrigger).toHaveAttribute("aria-expanded", "true")
        await page.getByRole("menuitem", { name: "Edit", exact: true }).click()
        dialog = page.getByRole("dialog", {
          name: "Edit Cloudflare · Clef connection",
          exact: true,
        })
        await expect(dialog.getByLabel("Cloudflare account ID", { exact: true })).toBeFocused()
        await expect(dialog.getByLabel("Provider", { exact: true })).toHaveCount(0)
        expect(await dialog.locator("label").allTextContents()).toEqual([
          "Cloudflare account ID",
          "Cloudflare model",
          "Cloudflare API token",
        ])
        await expectNoOverflow(page)
        await dialog
          .getByLabel("Cloudflare account ID", { exact: true })
          .scrollIntoViewIfNeeded()
        await page.screenshot({
          path: testInfo.outputPath(`checks-edit-${theme}-${layout.scale}.png`),
          animations: "disabled",
        })
        if (layout.scale === 200) {
          await dialog
            .getByRole("button", { name: "Save", exact: true })
            .scrollIntoViewIfNeeded()
          await page.screenshot({
            path: testInfo.outputPath(`checks-edit-${theme}-200-footer.png`),
            animations: "disabled",
          })
        }
        await page.keyboard.press("Escape")
        await expect(menuTrigger).toBeFocused()
        await card.getByRole("button", { name: "Check files", exact: true }).click()
        await expect(card.getByText("6 check files", { exact: true })).toBeVisible()
        await expect(card.getByText("2 check files", { exact: true })).toBeVisible()
        await card.getByRole("button", { name: /^Open in .*This device/ }).click()
        expect(await page.evaluate(() => window.__ANTIBURN_VISUAL_DIRECTORY_OPENS__)).toEqual([
          "fixture-global-checks",
        ])
        for (const path of await card.locator("p.wrap-anywhere").all())
          expect(
            await path.evaluate((node) => node.scrollWidth - node.clientWidth),
          ).toBeLessThanOrEqual(1)
        expect(await providerCalls(page)).toEqual(baseline)
      })

      test("empty setup adds the first connection without turning checks on", async ({
        page,
      }, testInfo) => {
        await page.goto(
          `/tests/visual/?surface=settings&theme=${theme}&scale=${layout.scale}&connections=empty`,
        )
        const pane = await openChecks(page)
        const baseline = await readOnlyBaseline(page)
        await expect(
          pane.getByRole("switch", { name: "Run checks", exact: true }),
        ).toBeDisabled()
        await expect(
          pane.getByText("Add a connection to set up model-based checks.", { exact: true }),
        ).toBeVisible()
        await pane
          .getByRole("heading", { name: "Model-based checks", exact: true })
          .evaluate((node) => node.scrollIntoView({ block: "start" }))
        await page.screenshot({
          path: testInfo.outputPath(`checks-empty-${theme}-${layout.scale}.png`),
          animations: "disabled",
        })
        await pane.getByRole("button", { name: "Add connection", exact: true }).click()
        const dialog = page.getByRole("dialog", { name: "Add connection" })
        const save = dialog.getByRole("button", { name: "Add connection", exact: true })
        await expect(save).toBeDisabled()
        await dialog.getByLabel("Jev API key", { exact: true }).fill("synthetic-key")
        await dialog.getByRole("button", { name: "Test connection", exact: true }).click()
        await expect(dialog.getByText("Connection test passed.", { exact: true })).toBeVisible()
        await save.click()
        await expect(dialog).toHaveCount(0)
        await expect(
          pane
            .getByRole("listitem", { name: "Jev", exact: true })
            .getByText("Selected", { exact: true }),
        ).toBeVisible()
        await expect(
          pane.getByRole("switch", { name: "Run checks", exact: true }),
        ).not.toBeChecked()
        await expect(
          pane.getByRole("switch", { name: "Run checks", exact: true }),
        ).toBeEnabled()
        await pane.getByRole("switch", { name: "Run checks", exact: true }).click()
        await expect(
          pane.getByRole("switch", { name: "Run checks", exact: true }),
        ).toBeChecked()
        expect(await providerCalls(page)).toEqual([
          ...baseline,
          "test_system_one_connection",
          "save_system_one_connection",
        ])
      })

      test("saved Jev and Cloudflare credentials use compact Saved and Change controls", async ({
        page,
      }, testInfo) => {
        await page.goto(
          `/tests/visual/?surface=settings&theme=${theme}&scale=${layout.scale}&credentials=saved`,
        )
        await openChecks(page)
        const baseline = await readOnlyBaseline(page)
        for (const [provider, label, field] of [
          ["jev", "Jev", "Jev API key"],
          ["cloudflare", "Cloudflare · Clef", "Cloudflare API token"],
        ]) {
          const opener = await connectionAction(page, label!, "Edit")
          const dialog = page.getByRole("dialog", {
            name: `Edit ${label} connection`,
            exact: true,
          })
          await expect(dialog.getByText("Saved", { exact: true })).toBeVisible()
          await expect(
            dialog.getByRole("button", { name: "Change", exact: true }),
          ).toBeVisible()
          await expect(dialog.getByLabel(field!, { exact: true })).toHaveCount(0)
          await expectNoOverflow(page)
          await dialog.getByRole("heading").scrollIntoViewIfNeeded()
          await page.screenshot({
            path: testInfo.outputPath(`checks-saved-${provider}-${theme}-${layout.scale}.png`),
            animations: "disabled",
          })
          if (layout.scale === 200) {
            await dialog
              .getByRole("button", { name: "Save", exact: true })
              .scrollIntoViewIfNeeded()
            await page.screenshot({
              path: testInfo.outputPath(`checks-saved-${provider}-${theme}-200-footer.png`),
              animations: "disabled",
            })
          }
          await dialog.getByRole("button", { name: "Cancel", exact: true }).click()
          await expect(dialog).toHaveCount(0)
          await expect(opener).toBeFocused()
        }
        expect(await providerCalls(page)).toEqual(baseline)
      })

      test("usage omits unknown counts and missing costs but keeps known data", async ({
        page,
      }, testInfo) => {
        for (const usage of ["unknown", "unpriced", "unknown-only"]) {
          await page.goto(
            `/tests/visual/?surface=settings&theme=${theme}&scale=${layout.scale}&checkUsage=${usage}`,
          )
          const pane = await openChecks(page)
          await readOnlyBaseline(page)
          const disclosure = pane.getByRole("button", { name: "Model usage", exact: true })
          await expect(pane).not.toContainText("unknown usage")
          await expect(pane).not.toContainText("Cost estimate unavailable")
          if (usage === "unknown-only") {
            await expect(disclosure).toHaveCount(0)
            await pane
              .getByRole("heading", { name: "Model-based checks", exact: true })
              .evaluate((node) => node.scrollIntoView({ block: "start" }))
          } else {
            await expect(disclosure).toHaveAttribute("aria-expanded", "false")
            await expect(disclosure).toContainText("12 requests")
            if (usage === "unknown") await expect(disclosure).toContainText("$0.000017")
            else await expect(disclosure).not.toContainText("$")
            await disclosure.click()
            await expect(pane.getByText("12,000", { exact: true })).toBeVisible()
            await expect(pane.getByText("1,800", { exact: true })).toBeVisible()
            await disclosure.evaluate((node) => node.scrollIntoView({ block: "start" }))
          }
          await expectNoOverflow(page)
          await page.screenshot({
            path: testInfo.outputPath(`checks-usage-${usage}-${theme}-${layout.scale}.png`),
            animations: "disabled",
          })
        }
      })
    })
  }
}

test("saved inactive profiles with null selection cannot enable checks until Use", async ({
  page,
}) => {
  await page.goto("/tests/visual/?surface=settings&connections=unselected")
  const pane = await openChecks(page)
  const baseline = await readOnlyBaseline(page)
  const run = pane.getByRole("switch", { name: "Run checks", exact: true })
  await expect(run).toBeDisabled()
  await expect(pane.getByText("Selected", { exact: true })).toHaveCount(0)
  await pane.getByRole("button", { name: "Use Jev", exact: true }).click()
  await expect(run).toBeEnabled()
  await run.click()
  await expect(run).toBeChecked()
  expect(await providerCalls(page)).toEqual([...baseline, "switch_system_one_connection"])
})

test("focus browsing does not switch and inactive Save never replaces explicit Use", async ({
  page,
}) => {
  await page.goto("/tests/visual/?surface=settings&connections=multiple-jev")
  const pane = await openChecks(page)
  const baseline = await readOnlyBaseline(page)
  const work = pane.getByRole("listitem", { name: "Jev 2", exact: true })
  await work.getByRole("button", { name: "Use Jev 2", exact: true }).focus()
  await page.keyboard.press("ArrowDown")
  expect(await providerCalls(page)).toEqual(baseline)
  await connectionAction(page, "Jev 2", "Edit")
  const dialog = page.getByRole("dialog", { name: "Edit Jev 2 connection", exact: true })
  await expect(dialog.getByText("Saved", { exact: true })).toBeVisible()
  await expect(dialog.getByLabel("Jev API key", { exact: true })).toHaveCount(0)
  await dialog.getByRole("button", { name: "Change", exact: true }).click()
  await dialog.getByLabel("Jev API key", { exact: true }).fill("synthetic-work-key")
  await dialog.getByRole("button", { name: "Save", exact: true }).click()
  await expect(dialog).toHaveCount(0)
  await expect(
    pane
      .getByRole("listitem", { name: "Jev", exact: true })
      .getByText("Selected", { exact: true }),
  ).toBeVisible()
  expect(await providerCalls(page)).toEqual([...baseline, "save_system_one_connection"])
  await work.getByRole("button", { name: "Use Jev 2", exact: true }).click()
  await expect(work.getByText("Selected", { exact: true })).toBeVisible()
  await expect(
    work.getByRole("button", { name: "More actions for Jev 2", exact: true }),
  ).toBeFocused()
  expect(await providerCalls(page)).toEqual([
    ...baseline,
    "save_system_one_connection",
    "switch_system_one_connection",
  ])
})

test("row menus support keyboard open, navigation, dismissal, and Edit focus", async ({
  page,
}) => {
  await page.goto("/tests/visual/?surface=settings&connections=multiple-jev")
  await openChecks(page)
  const baseline = await readOnlyBaseline(page)
  const trigger = page.getByRole("button", { name: "More actions for Jev 2", exact: true })
  for (const key of ["Enter", "Space", "ArrowDown"]) {
    await trigger.focus()
    await page.keyboard.press(key)
    const edit = page.getByRole("menuitem", { name: "Edit", exact: true })
    await expect(edit).toBeFocused()
    await expect(page.getByRole("menuitem")).toHaveCount(2)
    await page.keyboard.press("ArrowDown")
    await expect(page.getByRole("menuitem", { name: "Remove", exact: true })).toBeFocused()
    await page.keyboard.press("ArrowUp")
    await expect(edit).toBeFocused()
    await page.keyboard.press("Escape")
    await expect(page.getByRole("menu")).toHaveCount(0)
    await expect(trigger).toBeFocused()
  }
  await page.keyboard.press("Enter")
  await expect(page.getByRole("menuitem", { name: "Edit", exact: true })).toBeFocused()
  await page.keyboard.press("Enter")
  const dialog = page.getByRole("dialog", { name: "Edit Jev 2 connection", exact: true })
  await expect(dialog.getByRole("button", { name: "Change", exact: true })).toBeFocused()
  await page.keyboard.press("Escape")
  await expect(dialog).toHaveCount(0)
  await expect(trigger).toBeFocused()
  expect(await providerCalls(page)).toEqual(baseline)
})

test("failed Use keeps the saved selection and its error outside the editor", async ({
  page,
}) => {
  await page.goto(
    "/tests/visual/?surface=settings&connections=multiple-jev&providerSwitch=error",
  )
  const pane = await openChecks(page)
  const baseline = await readOnlyBaseline(page)
  const use = pane.getByRole("button", { name: "Use Jev 2", exact: true })
  await use.click()
  await expect(pane.getByRole("alert")).toHaveText(
    "The selected provider needs a valid credential.",
  )
  await expect(page.getByRole("dialog")).toHaveCount(0)
  await expect(use).toBeFocused()
  await expect(
    pane.getByRole("listitem", { name: "Jev", exact: true }),
  ).toHaveAccessibleDescription("Selected")
  expect(await providerCalls(page)).toEqual([
    ...baseline,
    "switch_system_one_connection",
    "get_system_one_settings",
  ])
})

for (const outcome of ["cancel", "failure"]) {
  test(`Remove ${outcome} restores the same row menu trigger`, async ({ page }) => {
    await page.goto(
      `/tests/visual/?surface=settings&connections=multiple-jev${outcome === "failure" ? "&confirm=accept&providerRemove=error" : ""}`,
    )
    const pane = await openChecks(page)
    const baseline = await readOnlyBaseline(page)
    const trigger = await connectionAction(page, "Jev 2", "Remove")
    await expect(trigger).toBeFocused()
    await expect(trigger).toBeEnabled()
    await expect(pane.getByRole("listitem")).toHaveCount(2)
    if (outcome === "failure") {
      await expect(pane.getByRole("alert")).toHaveText(
        "Could not remove the connection. Reload connections and try again.",
      )
      expect(await providerCalls(page)).toEqual([
        ...baseline,
        "remove_system_one_connection",
        "get_system_one_settings",
      ])
    } else expect(await providerCalls(page)).toEqual(baseline)
  })
}

test("inactive, selected, and last removal preserve usage and leave an empty usable list", async ({
  page,
}) => {
  await page.goto(
    "/tests/visual/?surface=settings&connections=multiple-jev&confirm=accept&checkUsage=unknown",
  )
  const pane = await openChecks(page)
  const baseline = await readOnlyBaseline(page)
  const run = pane.getByRole("switch", { name: "Run checks", exact: true })
  await run.click()
  await expect(run).toBeChecked()
  await connectionAction(page, "Jev 2", "Remove")
  await expect(pane.getByRole("listitem")).toHaveCount(1)
  await expect(pane.getByRole("button", { name: "Add connection", exact: true })).toBeFocused()
  await expect(run).toBeChecked()
  await connectionAction(page, "Jev", "Remove")
  await expect(pane.getByRole("listitem")).toHaveCount(0)
  await expect(pane.getByRole("button", { name: "Add connection", exact: true })).toBeFocused()
  await expect(run).not.toBeChecked()
  await expect(run).toBeDisabled()
  await expect(pane.getByRole("button", { name: "Add connection", exact: true })).toBeEnabled()
  await expect(pane.getByRole("button", { name: "Model usage", exact: true })).toContainText(
    "12 requests",
  )
  await expect(pane).not.toContainText("unknown usage")
  expect(await page.evaluate(() => window.__ANTIBURN_VISUAL_CONFIRMATIONS__)).toEqual([
    expect.stringContaining("does not pause checks"),
    expect.stringContaining("pauses model-based checks"),
  ])
  expect(await providerCalls(page)).toEqual([
    ...baseline,
    "remove_system_one_connection",
    "remove_system_one_connection",
  ])
})

test("retained legacy key has explicit use and forget actions even with no profiles", async ({
  page,
}) => {
  await page.goto(
    "/tests/visual/?surface=settings&connections=empty&legacyKey=saved&confirm=accept",
  )
  const pane = await openChecks(page)
  const baseline = await readOnlyBaseline(page)
  await pane.getByRole("button", { name: "Add connection", exact: true }).click()
  const dialog = page.getByRole("dialog", { name: "Add connection", exact: true })
  await expect(
    dialog.getByRole("button", { name: "Add connection", exact: true }),
  ).toBeDisabled()
  await dialog.getByRole("button", { name: "Use saved key", exact: true }).click()
  await expect(
    dialog.getByRole("button", { name: "Add connection", exact: true }),
  ).toBeEnabled()
  await dialog.getByRole("button", { name: "Forget shared TypeSafe key", exact: true }).click()
  await expect(dialog.getByRole("button", { name: "Use saved key", exact: true })).toHaveCount(
    0,
  )
  await expect(
    dialog.getByRole("button", { name: "Add connection", exact: true }),
  ).toBeDisabled()
  expect(await providerCalls(page)).toEqual([
    ...baseline,
    "remove_typesafe_api_key",
    "get_system_one_settings",
  ])
})

test("a modal contains focus and restores its actual Add opener", async ({ page }) => {
  await page.goto("/tests/visual/?surface=settings")
  const pane = await openChecks(page)
  const baseline = await readOnlyBaseline(page)
  for (const action of ["Cancel", "Escape"]) {
    const opener = pane.getByRole("button", { name: "Add connection", exact: true })
    await opener.click()
    const dialog = page.getByRole("dialog", { name: "Add connection" })
    await expect(dialog.getByLabel("Provider", { exact: true })).toBeFocused()
    await page.keyboard.press("Shift+Tab")
    await expect(dialog.getByRole("button", { name: "Cancel", exact: true })).toBeFocused()
    await page.keyboard.press("Tab")
    await expect(dialog.getByLabel("Provider", { exact: true })).toBeFocused()
    if (action === "Escape") await page.keyboard.press("Escape")
    else await dialog.getByRole("button", { name: "Cancel", exact: true }).click()
    await expect(dialog).toHaveCount(0)
    await expect(opener).toBeFocused()
  }
  expect(await providerCalls(page)).toEqual(baseline)
})

test("a pending save keeps its draft until an explicit owned Retry completes", async ({
  page,
}) => {
  await page.goto("/tests/visual/?surface=settings&providerSave=cleanup")
  await openChecks(page)
  const baseline = await readOnlyBaseline(page)
  await connectionAction(page, "Jev", "Edit")
  const dialog = page.getByRole("dialog", { name: "Edit Jev connection" })
  await dialog.getByRole("button", { name: "Change", exact: true }).click()
  await dialog.getByLabel("Jev API key", { exact: true }).fill("synthetic-replacement")
  await dialog.getByRole("button", { name: "Save", exact: true }).click()
  await expect(dialog.getByRole("alert")).toHaveText(
    "Could not finish credential cleanup. Use Retry in Settings → Checks.",
  )
  await expect(dialog.getByLabel("Jev API key", { exact: true })).toHaveValue(
    "synthetic-replacement",
  )
  expect(await providerCalls(page)).toEqual([
    ...baseline,
    "save_system_one_connection",
    "get_system_one_settings",
  ])
  await expect(dialog.getByRole("button", { name: "Save", exact: true })).toBeDisabled()
  await dialog.getByRole("button", { name: "Retry connection change", exact: true }).click()
  await expect(dialog).toHaveCount(0)
  expect(await providerCalls(page)).toEqual([
    ...baseline,
    "save_system_one_connection",
    "get_system_one_settings",
    "retry_system_one_connection_operation",
  ])
})

test("inactive Edit restores its actual opener after Cancel and Escape without a write", async ({
  page,
}) => {
  await page.goto("/tests/visual/?surface=settings&connections=multiple-jev")
  const pane = await openChecks(page)
  const baseline = await readOnlyBaseline(page)
  const opener = pane.getByRole("button", { name: "More actions for Jev 2", exact: true })
  for (const action of ["Cancel", "Escape"]) {
    await connectionAction(page, "Jev 2", "Edit")
    const dialog = page.getByRole("dialog", { name: "Edit Jev 2 connection", exact: true })
    await expect(dialog.getByText("Saved", { exact: true })).toBeVisible()
    await expect(dialog.getByRole("button", { name: "Change", exact: true })).toBeFocused()
    if (action === "Cancel")
      await dialog.getByRole("button", { name: "Cancel", exact: true }).click()
    else await page.keyboard.press("Escape")
    await expect(dialog).toHaveCount(0)
    await expect(opener).toBeFocused()
    await expect(
      pane
        .getByRole("listitem", { name: "Jev", exact: true })
        .getByText("Selected", { exact: true }),
    ).toBeVisible()
  }
  expect(await providerCalls(page)).toEqual(baseline)
})

test("busy Save blocks dismissal, then failed Save can close while Retry stays reachable", async ({
  page,
}) => {
  await page.goto("/tests/visual/?surface=settings&providerSave=loading")
  const pane = await openChecks(page)
  const baseline = await readOnlyBaseline(page)
  await connectionAction(page, "Jev", "Edit")
  const dialog = page.getByRole("dialog", { name: "Edit Jev connection", exact: true })
  await dialog.getByRole("button", { name: "Change", exact: true }).click()
  await dialog.getByLabel("Jev API key", { exact: true }).fill("synthetic-replacement")
  await dialog.getByRole("button", { name: "Save", exact: true }).click()
  await expect(dialog.getByRole("button", { name: "Cancel", exact: true })).toBeDisabled()
  await page.keyboard.press("Escape")
  await expect(dialog).toBeVisible()
  await page.evaluate(() => window.__ANTIBURN_VISUAL_RELEASE_PROVIDER_OPERATION__?.())
  await expect(
    dialog.getByRole("button", { name: "Retry connection change", exact: true }),
  ).toBeVisible()
  await expect(dialog.getByLabel("Jev API key", { exact: true })).toHaveValue(
    "synthetic-replacement",
  )
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click()
  await expect(dialog).toHaveCount(0)
  await expect(
    pane.getByRole("button", { name: "Retry connection change", exact: true }),
  ).toBeVisible()
  expect(await providerCalls(page)).toEqual([
    ...baseline,
    "save_system_one_connection",
    "get_system_one_settings",
  ])
  await pane.getByRole("button", { name: "Retry connection change", exact: true }).click()
  await expect(pane.getByRole("button", { name: "Add connection", exact: true })).toBeFocused()
})

test("incoming outside search waits for the native connection modal and keeps Save recovery", async ({
  page,
}) => {
  await page.goto("/tests/visual/?surface=settings&providerSave=loading")
  const pane = await openChecks(page)
  const baseline = await readOnlyBaseline(page)
  await connectionAction(page, "Jev", "Edit")
  const dialog = page.getByRole("dialog", { name: "Edit Jev connection", exact: true })
  await dialog.getByRole("button", { name: "Change", exact: true }).click()
  const key = dialog.getByLabel("Jev API key", { exact: true })
  await key.fill("synthetic-search-secret")
  await page.evaluate(() =>
    window.__ANTIBURN_VISUAL_EMIT__?.("settings:pane", "checks#checkDirectories"),
  )
  await expect(key).toBeFocused()
  await expect(key).toHaveValue("synthetic-search-secret")
  expect(await dialog.evaluate((node) => node.matches(":modal"))).toBe(true)
  expect(await providerCalls(page)).toEqual(baseline)
  expect(await page.evaluate(() => window.__ANTIBURN_VISUAL_CONFIRMATIONS__ ?? [])).toEqual([])

  await dialog.getByRole("button", { name: "Save", exact: true }).click()
  await page.evaluate(() =>
    window.__ANTIBURN_VISUAL_EMIT__?.("settings:pane", "checks#checkModelUsage"),
  )
  await page.keyboard.press("Escape")
  await expect(dialog.getByRole("button", { name: "Cancel", exact: true })).toBeDisabled()
  expect(await dialog.evaluate((node) => node.matches(":modal"))).toBe(true)
  await page.evaluate(() => window.__ANTIBURN_VISUAL_RELEASE_PROVIDER_OPERATION__?.())
  await expect(
    dialog.getByRole("button", { name: "Retry connection change", exact: true }),
  ).toBeEnabled()
  await expect(key).toHaveValue("synthetic-search-secret")
  await key.fill("synthetic-later-search-secret")
  await dialog.getByRole("button", { name: "Cancel", exact: true }).click()
  await expect(dialog).toHaveCount(0)
  const usage = pane.getByRole("button", { name: "Model usage", exact: true })
  await expect(usage).toBeFocused()
  await expect(usage).toHaveAttribute("aria-expanded", "true")
  await expect(
    pane.getByRole("button", { name: "Retry connection change", exact: true }),
  ).toBeEnabled()
  const files = pane.getByRole("button", { name: "Check files", exact: true })
  await files.focus()
  await page.keyboard.press("Space")
  await expect(files).toBeFocused()
  await connectionAction(page, "Jev", "Edit")
  await expect(key).toHaveValue("synthetic-later-search-secret")
  expect(await dialog.evaluate((node) => node.matches(":modal"))).toBe(true)
  expect(await providerCalls(page)).toEqual([
    ...baseline,
    "save_system_one_connection",
    "get_system_one_settings",
  ])
  expect(await page.evaluate(() => window.__ANTIBURN_VISUAL_CONFIRMATIONS__ ?? [])).toEqual([])
})

test("incoming current-pane search waits for compact Settings navigation to close", async ({
  page,
}) => {
  await page.setViewportSize({ width: 640, height: 720 })
  await page.goto("/tests/visual/?surface=settings")
  const pane = await openChecks(page)
  const baseline = await readOnlyBaseline(page)
  const opener = page.getByRole("button", { name: "Open Settings navigation", exact: true })
  const navigation = page.getByRole("dialog", { name: "Settings navigation", exact: true })
  const files = pane.getByRole("button", { name: "Check files", exact: true })
  for (const action of ["Escape", "Close navigation"]) {
    await opener.click()
    await expect(navigation).toBeVisible()
    await page.evaluate(() =>
      window.__ANTIBURN_VISUAL_EMIT__?.("settings:pane", "checks#checkDirectories"),
    )
    expect(await navigation.evaluate((node) => node.matches(":modal"))).toBe(true)
    expect(await navigation.evaluate((node) => node.contains(document.activeElement))).toBe(
      true,
    )
    if (action === "Escape") await page.keyboard.press("Escape")
    else await navigation.getByRole("button", { name: "Close navigation", exact: true }).click()
    await expect(navigation).toBeHidden()
    await expect(files).toBeFocused()
    await expect(files).toHaveAttribute("aria-expanded", "true")
    const run = pane.getByRole("switch", { name: "Run checks", exact: true })
    await run.focus()
    await expect(run).toBeFocused()
  }
  expect(await providerCalls(page)).toEqual(baseline)
})

for (const action of ["Cancel", "Escape"] as const) {
  test(`pending Add returns focus to global Retry after ${action} and keeps its draft`, async ({
    page,
  }) => {
    await page.goto("/tests/visual/?surface=settings&connections=empty&providerSave=rollback")
    const pane = await openChecks(page)
    const baseline = await readOnlyBaseline(page)
    const opener = pane.getByRole("button", { name: "Add connection", exact: true })
    await opener.click()
    const dialog = page.getByRole("dialog", { name: "Add connection", exact: true })
    await dialog.getByLabel("Jev API key", { exact: true }).fill("synthetic-add-secret")
    await dialog.getByRole("button", { name: "Add connection", exact: true }).click()
    await expect(
      dialog.getByRole("button", { name: "Retry connection change", exact: true }),
    ).toBeVisible()
    if (action === "Cancel")
      await dialog.getByRole("button", { name: "Cancel", exact: true }).click()
    else await page.keyboard.press("Escape")
    await expect(dialog).toHaveCount(0)
    const retry = pane.getByRole("button", { name: "Retry connection change", exact: true })
    await expect(retry).toHaveCount(1)
    await expect(retry).toBeFocused()
    await expect(opener).toBeDisabled()
    await expect(pane.getByRole("listitem")).toHaveCount(0)
    expect(await providerCalls(page)).toEqual([
      ...baseline,
      "save_system_one_connection",
      "get_system_one_settings",
    ])
    await retry.click()
    await expect(retry).toHaveCount(0)
    await expect(opener).toBeEnabled()
    await opener.click()
    await expect(
      page.getByRole("dialog").getByLabel("Jev API key", { exact: true }),
    ).toHaveValue("synthetic-add-secret")
    expect(await providerCalls(page)).toEqual([
      ...baseline,
      "save_system_one_connection",
      "get_system_one_settings",
      "retry_system_one_connection_operation",
    ])
  })
}

for (const scenario of [
  {
    kind: "Use",
    query: "providerSwitch=rollback",
    label: "Jev 2",
    command: "switch_system_one_connection",
  },
  {
    kind: "Remove",
    query: "providerRemove=rollback&confirm=accept",
    label: "Jev",
    command: "remove_system_one_connection",
  },
]) {
  test(`precommit ${scenario.kind} failure focuses Retry while its row action stays disabled`, async ({
    page,
  }) => {
    await page.goto(
      `/tests/visual/?surface=settings&connections=multiple-jev&${scenario.query}`,
    )
    const pane = await openChecks(page)
    const baseline = await readOnlyBaseline(page)
    const opener =
      scenario.kind === "Use"
        ? pane.getByRole("button", { name: `Use ${scenario.label}`, exact: true })
        : pane.getByRole("button", { name: `More actions for ${scenario.label}`, exact: true })
    if (scenario.kind === "Use") await opener.click()
    else await connectionAction(page, scenario.label, "Remove")
    const retry = pane.getByRole("button", { name: "Retry connection change", exact: true })
    await expect(retry).toHaveCount(1)
    await expect(retry).toBeFocused()
    await expect(opener).toBeVisible()
    if (scenario.kind === "Use") await expect(opener).toBeDisabled()
    else {
      await expect(opener).toBeEnabled()
      await opener.click()
      await expect(page.getByRole("menuitem", { name: "Remove", exact: true })).toBeDisabled()
      await expect(page.getByRole("menuitem", { name: "Edit", exact: true })).toBeEnabled()
      await page.keyboard.press("Escape")
    }
    await expect(pane.getByRole("listitem")).toHaveCount(2)
    expect(await providerCalls(page)).toEqual([
      ...baseline,
      scenario.command,
      "get_system_one_settings",
    ])
  })
}

test("pending removal survives its deleted row and reload, then Retry restores stable focus", async ({
  page,
}, testInfo) => {
  await page.goto(
    "/tests/visual/?surface=settings&connections=last&providerRemove=cleanup&confirm=accept",
  )
  let pane = await openChecks(page)
  const baseline = await readOnlyBaseline(page)
  await connectionAction(page, "Jev", "Remove")
  const retry = pane.getByRole("button", { name: "Retry connection change", exact: true })
  await expect(retry).toBeFocused()
  await expect(pane.getByRole("listitem")).toHaveCount(0)
  await expect(pane.getByRole("button", { name: "Add connection", exact: true })).toBeDisabled()
  expect(await providerCalls(page)).toEqual([
    ...baseline,
    "remove_system_one_connection",
    "get_system_one_settings",
  ])
  const operationId = await page.evaluate(() => {
    const journal: { settings: ProviderSettings } = JSON.parse(
      sessionStorage.getItem("antiburn-visual-pending-provider")!,
    )
    return journal.settings.pendingOperation!.id
  })
  await page.screenshot({
    path: testInfo.outputPath("checks-pending-removal.png"),
    animations: "disabled",
  })
  await page.reload()
  pane = await openChecks(page)
  await expect(
    pane.getByRole("button", { name: "Retry connection change", exact: true }),
  ).toBeVisible()
  const reloadedBaseline = await readOnlyBaseline(page, false)
  await pane.getByRole("button", { name: "Retry connection change", exact: true }).click()
  await expect(
    pane.getByRole("button", { name: "Retry connection change", exact: true }),
  ).toHaveCount(0)
  await expect(pane.getByRole("button", { name: "Add connection", exact: true })).toBeFocused()
  expect(await providerCalls(page)).toEqual([
    ...reloadedBaseline,
    "retry_system_one_connection_operation",
  ])
  const request = await page.evaluate(() =>
    window.__ANTIBURN_VISUAL_PROVIDER_REQUESTS__?.at(-1),
  )
  expect(request).toEqual({
    command: "retry_system_one_connection_operation",
    args: { operationId },
  })
})

test("explicit Use saved key submits Add successfully", async ({ page }) => {
  await page.goto("/tests/visual/?surface=settings&connections=empty&legacyKey=saved")
  const pane = await openChecks(page)
  const baseline = await readOnlyBaseline(page)
  await pane.getByRole("button", { name: "Add connection", exact: true }).click()
  const dialog = page.getByRole("dialog", { name: "Add connection", exact: true })
  await expect(
    dialog.getByRole("button", { name: "Add connection", exact: true }),
  ).toBeDisabled()
  await dialog.getByRole("button", { name: "Use saved key", exact: true }).click()
  await dialog.getByRole("button", { name: "Add connection", exact: true }).click()
  await expect(dialog).toHaveCount(0)
  await expect(
    pane.getByRole("listitem", { name: "Jev", exact: true }),
  ).toHaveAccessibleDescription("Selected")
  expect(await providerCalls(page)).toEqual([...baseline, "save_system_one_connection"])
})

test("Ollama Find lists installed tags without loading, pulling, testing, or selecting", async ({
  page,
}) => {
  await page.goto("/tests/visual/?surface=settings")
  await openChecks(page)
  const baseline = await readOnlyBaseline(page)
  await page.evaluate(() =>
    window.__ANTIBURN_VISUAL_EMIT__?.("settings:pane", "checks#ollamaModel"),
  )
  const model = page.getByLabel("Ollama model", { exact: true })
  await expect(model).toBeFocused()
  await expect(model).toHaveValue("synthetic-local-model")
  expect(await providerCalls(page)).toEqual(baseline)
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "Find models", exact: true })
    .click()
  await expect(
    model.getByRole("option", { name: "Other local model", exact: true }),
  ).toHaveCount(1)
  await expect(model).toHaveValue("synthetic-local-model")
  expect(await providerCalls(page)).toEqual([...baseline, "list_ollama_models"])
})

test("a non-first search field survives failed-read Retry without provider actions", async ({
  page,
}) => {
  await page.goto("/tests/visual/?surface=settings&providerRead=until-retry")
  await openChecks(page)
  await page.evaluate(() =>
    window.__ANTIBURN_VISUAL_EMIT__?.("settings:pane", "checks#customResponseMode"),
  )
  const retry = page.getByRole("button", { name: "Retry loading connections", exact: true })
  await expect(retry).toBeFocused()
  const baseline = await readOnlyBaseline(page, false)
  await page.evaluate(() => window.__ANTIBURN_VISUAL_RECOVER_PROVIDERS__?.())
  expect(await providerCalls(page)).toEqual(baseline)
  await retry.click()
  await expect(
    page.getByRole("dialog").getByLabel("Response mode", { exact: true }),
  ).toBeFocused()
  expect(await providerCalls(page)).toEqual([...baseline, "get_system_one_settings"])
})

test("every field search focuses the actual labeled field without a write", async ({
  page,
}) => {
  await page.goto("/tests/visual/?surface=settings")
  await openChecks(page)
  const baseline = await readOnlyBaseline(page)
  for (const [control, label] of [
    ["typeSafeApiKey", "Jev API key"],
    ["ollamaUrl", "Ollama URL"],
    ["ollamaModel", "Ollama model"],
    ["ollamaApiKey", "Ollama API key (optional)"],
    ["cloudflareAccountId", "Cloudflare account ID"],
    ["cloudflareApiToken", "Cloudflare API token"],
    ["cloudflareModel", "Cloudflare model"],
    ["customUrl", "Exact inference URL"],
    ["customModel", "Custom model"],
    ["customApiKey", "Custom API key (optional)"],
    ["customResponseMode", "Response mode"],
  ]) {
    await page.evaluate(
      (target) => window.__ANTIBURN_VISUAL_EMIT__?.("settings:pane", `checks#${target}`),
      control,
    )
    await expect(page.getByLabel(label!, { exact: true })).toBeFocused()
    await page.keyboard.press("Escape")
    await expect(page.getByRole("dialog")).toHaveCount(0)
  }
  expect(await providerCalls(page)).toEqual(baseline)
})

test("search opens its field while another row menu is open without selecting a menu action", async ({
  page,
}) => {
  await page.goto("/tests/visual/?surface=settings")
  await openChecks(page)
  const baseline = await readOnlyBaseline(page)
  await page.getByRole("button", { name: "More actions for Jev", exact: true }).click()
  await expect(page.getByRole("menu")).toBeVisible()
  await page.evaluate(() =>
    window.__ANTIBURN_VISUAL_EMIT__?.("settings:pane", "checks#customResponseMode"),
  )
  const field = page.getByRole("dialog").getByLabel("Response mode", { exact: true })
  await expect(field).toBeFocused()
  await expect(page.getByRole("menu")).toHaveCount(0)
  await page.keyboard.press("Tab")
  await expect(field).not.toBeFocused()
  expect(await providerCalls(page)).toEqual(baseline)
})

test("folder errors remain visible while closed and search only reveals the disclosure", async ({
  page,
}) => {
  await page.goto("/tests/visual/?surface=settings&directories=unavailable")
  await openChecks(page)
  const baseline = await readOnlyBaseline(page)
  const files = page.getByRole("button", { name: "Check files", exact: true })
  await expect(files).toContainText(
    "Could not load check folders. Reopen Settings → Checks to try again.",
  )
  await expect(files).toHaveAttribute("aria-expanded", "false")
  await page.evaluate(() =>
    window.__ANTIBURN_VISUAL_EMIT__?.("settings:pane", "checks#checkDirectories"),
  )
  await expect(files).toHaveAttribute("aria-expanded", "true")
  await expect(files).toBeFocused()
  expect(await page.evaluate(() => window.__ANTIBURN_VISUAL_DIRECTORY_OPENS__ ?? [])).toEqual(
    [],
  )
  expect(await providerCalls(page)).toEqual(baseline)
})
