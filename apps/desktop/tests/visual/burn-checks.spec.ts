import { expect, test } from "@playwright/test"

for (const theme of ["light", "dark"] as const) {
  for (const { width, scale } of [
    { width: 1280, scale: 100 },
    { width: 640, scale: 100 },
    { width: 640, scale: 200 },
  ]) {
    test(`installed check results stay clear in ${theme} at ${width} and ${scale}%`, async ({
      page,
    }, testInfo) => {
      await page.setViewportSize({ width, height: width === 640 ? 480 : 860 })
      await page.clock.setFixedTime(new Date("2026-09-15T00:00:00Z"))
      await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" })
      await page.goto(
        `/tests/visual/?surface=main&onboarding=complete&checks=installed&theme=${theme}&scale=${scale}`,
      )
      const overview = page.getByRole("tabpanel", { name: "Overview", exact: true })
      await expect(overview).toBeVisible({ timeout: 30_000 })
      await expect(
        overview.getByRole("button", { name: "First check, Not assessed · Checking" }),
      ).toBeVisible()
      await expect(
        overview.getByRole("button", { name: "First check, Passed · Checking" }),
      ).toHaveCount(0)
      await expect(
        overview.getByRole("button", {
          name: "Connection unavailable, Not assessed",
        }),
      ).toBeVisible()

      const navigation = page.getByRole("button", { name: "Open Main navigation" })
      if (await navigation.isVisible()) await navigation.click()
      await page.getByRole("tab", { name: "Checks", exact: true }).click()
      const collection = page.getByRole("region", { name: "Burn check collection" })
      const failed = collection.getByRole("button", {
        name: "Saved failure, 1 failed · Checking",
      })
      const passed = collection.getByRole("button", { name: "Saved pass, Passed · Checking" })
      await expect(collection.getByRole("region", { name: "Failed checks 1" })).toContainText(
        "Saved failure",
      )
      await expect(collection.getByRole("region", { name: "Passed checks 2" })).toContainText(
        "Saved pass",
      )
      await expect(failed).toHaveAttribute("data-outcome", "failed")
      await expect(passed).toHaveAttribute("data-outcome", "passed")
      await expect(collection.getByRole("region", { name: /^Checking/ })).toHaveCount(0)

      const neutralGroup = collection.getByRole("button", { name: "Not assessed (3)" })
      await neutralGroup.focus()
      await neutralGroup.press("ArrowDown")
      const first = collection.getByRole("button", {
        name: "First check, Not assessed · Checking",
      })
      await expect(first).toBeFocused()
      await expect(first).toHaveAttribute("data-outcome", "unassessed")
      await expect(first).not.toContainText("Passed")
      await first.press("Enter")
      const details = page.getByRole("region", { name: "Burn check details" })
      const selectedDetail = details.locator(".burn-check-detail:visible")
      const detailBody = selectedDetail.locator(".burn-check-detail-body")
      await expect(details.getByRole("status")).toHaveText(
        "First check: Not assessed · Checking",
      )
      const status = details.getByRole("region", { name: "Check status", exact: true })
      await expect(status).toHaveCount(1)
      await expect(status.getByRole("heading", { name: "Checking", exact: true })).toBeVisible()
      await expect(status.getByText("No check result yet.", { exact: true })).toBeVisible()
      await expect(detailBody.locator(".text-burn-check-pass-fill")).toHaveCount(0)
      await expect(detailBody.locator(".bg-surface-card\\/75")).toHaveCount(1)
      await status.scrollIntoViewIfNeeded()
      await page.screenshot({
        path: testInfo.outputPath(`burn-checks-first-check-${theme}-${width}-${scale}.png`),
        animations: "disabled",
      })

      for (const { name, summary, explanation, capture } of [
        {
          name: "Saved pass, Passed · Checking",
          summary: "Saved check result: no concern found.",
          explanation: null,
          capture: "saved-pass",
        },
        {
          name: "Saved pass with explanation, Passed · Checking",
          summary: "Saved check result",
          explanation: "The saved check found no concern.",
          capture: "saved-pass-explanation",
        },
      ]) {
        await collection.getByRole("button", { name, exact: true }).click()
        await expect(status).toHaveCount(1)
        await expect(
          status.getByRole("heading", { name: "Checking", exact: true }),
        ).toBeVisible()
        await expect(status.getByText(summary, { exact: true })).toBeVisible()
        await expect(selectedDetail.locator("header")).toContainText("Passed")
        if (explanation) {
          await expect(detailBody.getByText(explanation, { exact: true })).toHaveCount(1)
          await expect(status.getByText(explanation, { exact: true })).toBeVisible()
        }
        await expect(detailBody.locator(".text-burn-check-pass-fill")).toHaveCount(0)
        await expect(detailBody.locator(".bg-surface-card\\/75")).toHaveCount(1)
        expect(
          await details.evaluate((node) => node.scrollWidth - node.clientWidth),
        ).toBeLessThanOrEqual(1)
        await status.scrollIntoViewIfNeeded()
        await page.screenshot({
          path: testInfo.outputPath(`burn-checks-${capture}-${theme}-${width}-${scale}.png`),
          animations: "disabled",
        })
      }
      await collection
        .getByRole("button", { name: "Waiting for activity, Not assessed" })
        .click()
      await expect(details.getByText("Needs 3 more tool results.")).toBeVisible()
      await collection
        .getByRole("button", { name: "Connection unavailable, Not assessed" })
        .click()
      await expect(
        details.getByRole("region", { name: "Decision connection needed" }),
      ).toBeVisible()
      await expect(details.getByRole("status")).toHaveText(
        "Connection unavailable: Not assessed. Decision connection needed",
      )

      await failed.click()
      const alert = details.getByRole("region", {
        name: "OpenCode live checks are not running",
      })
      await expect(alert).toBeVisible()
      await expect(status.getByRole("heading", { name: "Checking", exact: true })).toBeVisible()
      await expect(status.getByText("Saved check results: 1 failed.")).toBeVisible()
      await expect(
        details.getByRole("region", { name: "OpenCode live checks are not running" }),
      ).toHaveCount(1)
      await expect(
        details.getByText("The same failed command ran again without a change."),
      ).toBeVisible()
      await expect(details.getByText(/OpenCode V2|Missing information/i)).toHaveCount(0)
      await expect(
        alert.getByText(
          "Live activity is not available for this session. Keep OpenCode open, then rescan sessions. Live checks cannot recover missed activity. Saved results cover only their recorded input.",
          { exact: true },
        ),
      ).toBeVisible()
      const technical = alert.locator("summary", { hasText: "Technical details" })
      await expect(technical.locator("..")).not.toHaveAttribute("open", "")
      await technical.focus()
      await technical.press("Enter")
      await expect(alert.getByText("opencode-v2-capture-gap", { exact: true })).toBeVisible()
      const cause = alert.getByText(
        "The connection for live OpenCode activity is not active.",
        {
          exact: true,
        },
      )
      await expect(cause).toBeVisible()
      await expect(cause).not.toHaveCSS("font-family", /monospace/)
      await expect(
        alert.getByText(/affected sessions?|Stored analysis|earlier events/i),
      ).toHaveCount(0)
      await expect(alert.getByText("Diagnostic code", { exact: true })).toHaveCount(0)
      await expect(technical).toBeFocused()
      const diagnosticCode = alert.getByText("opencode-v2-capture-gap", { exact: true })
      await diagnosticCode.scrollIntoViewIfNeeded()
      await expect(cause).toBeInViewport()
      await expect(diagnosticCode).toBeInViewport()
      await page.screenshot({
        path: testInfo.outputPath(`burn-checks-technical-details-${theme}-${width}.png`),
        animations: "disabled",
      })
      await technical.press("Enter")
      await expect(
        alert.getByText("opencode-v2-capture-gap", { exact: true }),
      ).not.toBeVisible()
      const moreIssues = details.locator("summary", { hasText: "2 more issues" })
      await expect(moreIssues).toBeVisible()
      await expect(moreIssues.locator("..")).not.toHaveAttribute("open", "")
      await alert.scrollIntoViewIfNeeded()
      await page.screenshot({
        path: testInfo.outputPath(`burn-checks-${theme}-${width}.png`),
        animations: "disabled",
      })
      await moreIssues.focus()
      await moreIssues.press("Enter")
      await expect(
        details.getByRole("region", { name: "Session journal unavailable" }),
      ).toBeVisible()
      await expect(
        details.getByRole("region", { name: "Session source unavailable" }),
      ).toBeVisible()
      await expect(technical.locator("..")).not.toHaveAttribute("open", "")
      await page.screenshot({
        path: testInfo.outputPath(`burn-checks-more-issues-${theme}-${width}.png`),
        animations: "disabled",
      })
      expect(
        await details.evaluate((node) => node.scrollWidth - node.clientWidth),
      ).toBeLessThanOrEqual(1)
    })
  }
}
