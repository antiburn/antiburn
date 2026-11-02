import { expect, test } from "@playwright/test"
import type { CheckAvailability } from "../../src/lib/checkAvailability"
import type { BurnCheckDetectorId } from "../../src/lib/insightsIpc"
import { CHECK_DEFINITIONS } from "../../src/lib/presentation/checkDefinitions"

const removedAvailability: CheckAvailability = {
  revision: 0,
  yamlEnabled: true,
  installedChecks: [],
  checks: (Object.keys(CHECK_DEFINITIONS) as BurnCheckDetectorId[]).map((id) => ({
    id,
    enabled: false,
  })),
  configured: true,
  savedKey: false,
  error: null,
  usage: {
    inputTokens: 0,
    outputTokens: 0,
    confirmedCalls: 0,
    cacheHits: 0,
    unknownOutcomes: 0,
    estimatedUsd: "$0.00",
    lastUsedAtEpoch: null,
  },
}

for (const keepSearchOpen of [false, true]) {
  test(`removed definitions leave search and details${keepSearchOpen ? " without taking later focus" : ""}`, async ({
    page,
  }) => {
    await page.goto(
      "/tests/visual/?surface=main&onboarding=complete&checks=installed&theme=light",
    )
    const search = page.getByRole("button", { name: "Search antiburn", exact: true })
    await expect(search).toBeVisible({ timeout: 30_000 })
    await page.getByRole("tab", { name: "Checks", exact: true }).click()
    const collection = page.getByRole("region", { name: "Burn check collection", exact: true })
    const details = page.getByRole("region", {
      name: "Burn check details",
      exact: true,
      includeHidden: true,
    })
    await collection
      .getByRole("button", { name: "Saved pass, Passed · Checking", exact: true })
      .click()
    await expect(
      details.getByRole("heading", { name: "Saved pass", exact: true }),
    ).toBeVisible()
    await page.getByRole("tab", { name: "Overview", exact: true }).click()
    await expect(page.getByRole("tabpanel", { name: "Overview", exact: true })).toBeVisible()
    await search.click()
    const dialog = page.getByRole("dialog", { name: "Search antiburn" })
    const query = dialog.getByRole("combobox", { name: "Search antiburn" })
    await query.fill("Saved failure")
    await expect(dialog.getByRole("option", { name: /Saved failure/ })).toBeVisible()
    await query.press("Enter")
    await expect(dialog).toHaveCount(0)
    await expect(page.getByRole("tabpanel", { name: "Checks", exact: true })).toBeVisible()
    await expect(
      details.getByRole("heading", { name: "Saved failure", exact: true }),
    ).toBeVisible()
    const failed = collection.getByRole("button", {
      name: "Saved failure, 1 failed · Checking",
      exact: true,
    })
    await expect(failed).toHaveAttribute("aria-pressed", "true")
    await expect(failed).toBeFocused()
    await expect(
      details.getByText("The same failed command ran again without a change.", { exact: true }),
    ).toBeVisible()
    if (keepSearchOpen) {
      await search.click()
      await query.fill("Saved failure")
      await expect(query).toBeFocused()
    }
    await page.evaluate((snapshot) => {
      const url = new URL(window.location.href)
      url.searchParams.set("checks", "removed")
      history.replaceState(null, "", url)
      window.__ANTIBURN_VISUAL_EMIT__?.("checks:availability-changed", {
        status: "updated",
        snapshot,
      })
      window.__ANTIBURN_VISUAL_EMIT__?.("checks:report-changed", undefined)
    }, removedAvailability)
    await expect(
      page.getByRole("button", {
        name: "Saved failure, 1 failed · Checking",
        includeHidden: true,
      }),
    ).toHaveCount(0)
    await expect(
      details.getByRole("heading", { name: "Saved failure", exact: true, includeHidden: true }),
    ).toHaveCount(0)
    await expect(
      details.getByText("The same failed command ran again without a change.", { exact: true }),
    ).toHaveCount(0)
    if (keepSearchOpen) await expect(query).toBeFocused()
    else {
      await expect(collection).toBeFocused()
      await search.click()
      await query.fill("Saved failure")
    }
    await expect(dialog.getByRole("option", { name: /Saved failure/ })).toHaveCount(0)
    await expect(dialog.getByText("No matching destinations.", { exact: true })).toBeVisible()
    await query.press("Escape")
    await expect(dialog).toHaveCount(0)
    await expect(
      details.getByRole("heading", { name: "Saved failure", exact: true, includeHidden: true }),
    ).toHaveCount(0)
  })
}
