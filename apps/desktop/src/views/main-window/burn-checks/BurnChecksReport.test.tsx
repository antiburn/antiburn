import { act, fireEvent, screen, waitFor, within } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"
import { createElement } from "react"
import { searchApp } from "../../../lib/appSearch"
import { BurnChecksView } from "../BurnChecksView"
import { aggregate, deferred, report, setup } from "./tests/burnChecksTestSupport"

describe("smart check report integration", () => {
  it.each([
    "ignoredInstructions",
    "scopeCreep",
    "overExploring",
    "skillOpportunities",
  ] as const)("shows provisional %s results and follows terminal transitions", async (id) => {
    const check = {
      ...report.categories[2]!,
      id,
      unavailable: 0,
      checking: true,
      sampled: true,
      partialContext: true,
      reviewCoverage: { reviewed: 2, total: null, uncertain: 1, pending: 3, continuing: true },
    }
    const { adapter, session } = setup(null, false, aggregate, {
      ...report,
      categories: [check],
    })
    const row = await screen.findByRole("button", { name: /No issues found yet, Checking…/ })
    expect(within(row).getByText("No issues found yet")).toHaveClass("text-system-green")
    expect(within(row).getByText("2 reviewed · 1 uncertain · 3 pending")).toBeVisible()
    expect(screen.queryByRole("button", { name: /Not assessed/ })).not.toBeInTheDocument()
    expect(adapter.getTargets).not.toHaveBeenCalled()
    const partial = screen.getByLabelText("This check used partial context")
    expect(partial).toHaveAttribute("tabindex", "0")
    fireEvent.focus(partial)
    expect(await screen.findByText(/Missing context can limit the assessment/)).toBeVisible()
    fireEvent.blur(partial)
    vi.mocked(adapter.getReport).mockResolvedValue({
      ...report,
      evidenceSettled: true,
      categories: [{ ...check, checking: false, clean: 1, lifecycle: "passing" }],
    })
    await act(async () => session.refresh())
    await screen.findByRole("button", { name: /, Passed/ })
    expect(screen.queryByText("Checking…")).not.toBeInTheDocument()
    expect(screen.queryByText("No issues found yet")).not.toBeInTheDocument()
    fireEvent.focus(screen.getByLabelText("This check has been sampled"))
    await screen.findByRole("tooltip")
    expect(screen.queryByText("Review is continuing.")).not.toBeInTheDocument()
    vi.mocked(adapter.getReport).mockResolvedValue({
      ...report,
      categories: [{ ...check, finding: 1, lifecycle: "failing" }],
    })
    await act(async () => session.refresh())
    await screen.findByRole("button", { name: /1 failed · 0 passed, Checking…/ })
    expect(screen.queryByText("No issues found yet")).not.toBeInTheDocument()
  })
  it.each([
    "ignoredInstructions",
    "scopeCreep",
    "overExploring",
    "skillOpportunities",
  ] as const)("reserves evidence space while %s target details load", async (id) => {
    const pending = deferred<null>()
    setup(pending.promise, false, aggregate, {
      ...report,
      categories: [{ ...report.categories[0]!, id }],
    })
    const loading = await screen.findByRole("region", { name: "Loading finding details" })
    expect(loading).toHaveAttribute("aria-busy", "true")
    expect(within(loading).getAllByRole("status")).toHaveLength(1)
    const evidence = within(loading).getByRole("status", { name: "Loading evidence" })
    expect(evidence.parentElement).toHaveClass("min-h-72")
    expect(evidence.querySelectorAll("[data-placeholder]")).toHaveLength(6)
    await act(async () => pending.resolve(null))
    expect(
      screen.queryByRole("region", { name: "Loading finding details" }),
    ).not.toBeInTheDocument()
  })

  it.each([
    ["macos", "over_exploring", "Over-exploring"],
    ["windows", "over_exploring", "Over-exploring"],
    ["linux", "over_exploring", "Over-exploring"],
    ["macos", "scope_creep", "Scope creep"],
    ["windows", "scope_creep", "Scope creep"],
    ["linux", "scope_creep", "Scope creep"],
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
      const info = screen.getByLabelText("This check has been sampled")
      expect(info.tagName).toBe("SPAN")
      expect(info).toBeVisible()
      expect(
        screen.queryByRole("button", { name: "This check has been sampled" }),
      ).not.toBeInTheDocument()
      expect(screen.queryByText(/3 complete sessions/)).not.toBeInTheDocument()
    },
  )
})
