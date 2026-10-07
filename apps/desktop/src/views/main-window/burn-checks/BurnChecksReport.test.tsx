import { screen, waitFor } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"
import { createElement } from "react"
import { searchApp } from "../../../lib/appSearch"
import { BurnChecksView } from "../BurnChecksView"
import { aggregate, report, setup } from "./tests/burnChecksTestSupport"

describe("smart check report integration", () => {
  it.each([
    ["macos", "over_exploring", "Over-exploring"],
    ["windows", "over_exploring", "Over-exploring"],
    ["linux", "over_exploring", "Over-exploring"],
    ["macos", "scope_creep", "Scope Creep"],
    ["windows", "scope_creep", "Scope Creep"],
    ["linux", "scope_creep", "Scope Creep"],
  ] as const)(
    "focuses %s search destination %s without changing values",
    async (platform, query, label) => {
      HTMLElement.prototype.scrollIntoView = vi.fn()
      const result = searchApp(query, platform)[0]!
      if (result.target.kind !== "check") throw new Error("Expected a check destination")
      const categories = [{ ...report.categories[2]!, id: result.target.check }]
      const before = structuredClone(categories)
      const { session, view, adapter } = setup(null, false, aggregate, {
        ...report,
        categories,
      })
      await screen.findByRole("button", { name: "Not assessed (1)" })
      view.rerender(
        createElement(BurnChecksView, {
          active: true,
          session,
          focusedCheck: result.target.check,
          focusRevision: 1,
        }),
      )
      const row = await screen.findByRole("button", { name: `${label}, Not assessed` })
      await waitFor(() => expect(row).toHaveFocus())
      expect(row).toHaveAttribute("aria-pressed", "true")
      expect(categories).toEqual(before)
      expect(adapter.getReport).toHaveBeenCalledOnce()
      expect(screen.queryByRole("button", { name: "Fix" })).not.toBeInTheDocument()
      expect(screen.queryByRole("button", { name: "Copy fix prompt" })).not.toBeInTheDocument()
    },
  )

  it("hides provider-dependent rows with the same availability as search", async () => {
    setup(null, false, aggregate, {
      ...report,
      smartChecksAvailable: false,
      categories: [
        report.categories[1]!,
        ...(
          ["ignoredInstructions", "skillOpportunities", "overExploring", "scopeCreep"] as const
        ).map((id) => ({ ...report.categories[2]!, id })),
      ],
    })
    await screen.findByRole("button", { name: /Unused skills, Passed/ })
    expect(screen.queryByRole("button", { name: /Not assessed/ })).not.toBeInTheDocument()
    expect(
      searchApp("", "macos", false)
        .filter((result) => result.target.kind === "check")
        .map((result) => result.label),
    ).not.toContain("Over-exploring")
  })

  it.each(["skillOpportunities", "overExploring", "scopeCreep"] as const)(
    "limits a sampled %s pass to assessed evidence",
    async (id) => {
      setup(null, false, aggregate, {
        ...report,
        categories: [{ ...report.categories[1]!, id, sampled: true }],
      })
      expect(
        await screen.findByText(
          "No finding in the assessed sample across 3 sessions. Unassessed work may remain.",
        ),
      ).toBeVisible()
      expect(screen.getByRole("button", { name: "About priority sampling" })).toBeVisible()
      expect(screen.queryByText(/3 complete sessions/)).not.toBeInTheDocument()
    },
  )
})
