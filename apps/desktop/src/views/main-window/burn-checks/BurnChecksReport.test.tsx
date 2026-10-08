import { act, fireEvent, screen, waitFor, within } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"
import { createElement } from "react"
import { searchApp } from "../../../lib/appSearch"
import { BurnChecksView } from "../BurnChecksView"
import { aggregate, deferred, report, setup } from "./tests/burnChecksTestSupport"

describe("smart check report integration", () => {
  it("shows running failure counts inline and animates only checking text", async () => {
    setup(null, false, aggregate, {
      ...report,
      categories: [
        {
          ...report.categories[0]!,
          id: "ignoredInstructions",
          finding: 43,
          clean: 0,
          checking: true,
          checkingCount: 2,
        },
      ],
    })
    const row = await screen.findByRole("button", {
      name: "Ignored instructions, 43 failed · 0 passed · 2 checking",
    })
    const checking = within(row).getByText("2 checking")
    expect(checking.closest(".font-mono")).toHaveTextContent("43 failed·0 passed·2 checking")
    expect(row.querySelectorAll(".activity-row-title-shimmer")).toHaveLength(1)
    expect(checking).toHaveAttribute("data-text", "2 checking")
    expect(within(row).getByText("43 failed")).not.toHaveClass("activity-row-title-shimmer")
    expect(row.querySelector(".animate-spin")).toBeNull()
    expect(screen.getByRole("region", { name: "Failed checks 1" })).toContainElement(row)
    expect(screen.queryByRole("region", { name: "Checks in progress" })).toBeNull()
  })

  it("keeps running and passed rows stable across continuation and completion", async () => {
    const check = {
      ...report.categories[2]!,
      id: "ignoredInstructions" as const,
      checking: true,
      checkingCount: 2,
    }
    const initial = { ...report, categories: [check, report.categories[1]!] }
    const { adapter, session } = setup(null, false, aggregate, initial)
    const row = await screen.findByRole("button", { name: "Ignored instructions, Checking" })
    const group = screen.getByRole("region", { name: "Passed checks 2" })
    expect(group).toContainElement(row)
    expect(within(row).getByText("Checking").closest(".font-mono")).toHaveTextContent(
      /^Checking$/,
    )
    expect(row.querySelector(".animate-spin")).toBeNull()
    row.focus()
    const detail = document.getElementById("burn-check-ignoredInstructions-detail")
    for (const updated of [
      { ...check, checkingCount: 1, clean: 1, lifecycle: "passing" as const },
      { ...check, checkingCount: 0, checking: false, clean: 2, lifecycle: "passing" as const },
    ]) {
      vi.mocked(adapter.getReport).mockResolvedValue({
        ...initial,
        categories: [updated, report.categories[1]!],
      })
      await act(async () => session.refresh())
      expect(screen.getByRole("button", { name: /Ignored instructions, / })).toBe(row)
      expect(row).toHaveFocus()
      expect(row).toHaveAttribute("aria-pressed", "true")
      expect(document.getElementById("burn-check-ignoredInstructions-detail")).toBe(detail)
      expect(within(group).getAllByRole("button").slice(1)[0]).toBe(row)
    }
    expect(within(row).getByText("Passed")).toBeVisible()
  })

  it("keeps detail focus when a previously focused checking row fails", async () => {
    const check = {
      ...report.categories[2]!,
      id: "ignoredInstructions" as const,
      checking: true,
      sampled: true,
    }
    const { adapter, session } = setup(null, false, aggregate, {
      ...report,
      categories: [check],
    })
    const row = await screen.findByRole("button", { name: "Ignored instructions, Checking" })
    act(() => row.focus())
    const control = screen.getByLabelText("This check has been sampled")
    act(() => control.focus())
    expect(control).toHaveFocus()

    vi.mocked(adapter.getReport).mockResolvedValue({
      ...report,
      categories: [{ ...check, finding: 1, checking: false, lifecycle: "failing" }],
    })
    await act(async () => session.refresh())

    await screen.findByRole("button", { name: /Ignored instructions, 1 failed/ })
    expect(row).not.toBeInTheDocument()
    expect(control).toHaveFocus()
  })

  it("keeps the Passed disclosure collapsed during continuation", async () => {
    const check = { ...report.categories[2]!, id: "scopeCreep" as const, checking: true }
    const { adapter, session } = setup(null, false, aggregate, {
      ...report,
      categories: [check],
    })
    const trigger = await screen.findByRole("button", { name: "Passed checks 1" })
    fireEvent.click(trigger)
    vi.mocked(adapter.getReport).mockResolvedValue({
      ...report,
      categories: [{ ...check, clean: 1, lifecycle: "passing" }],
    })
    await act(async () => session.refresh())
    expect(trigger).toHaveAttribute("aria-expanded", "false")
    expect(screen.queryByRole("button", { name: "Scope creep, Checking" })).toBeNull()
    fireEvent.click(trigger)
    expect(screen.getByRole("button", { name: "Scope creep, Checking" })).toBeVisible()
  })

  it("keeps passed checks visible when a new finding arrives", async () => {
    const passed = { ...report.categories[1]! }
    const { adapter, session } = setup(null, false, aggregate, {
      ...report,
      categories: [passed],
    })
    expect(await screen.findByRole("button", { name: /Unused skills, Passed/ })).toBeVisible()
    vi.mocked(adapter.getReport).mockResolvedValue({
      ...report,
      evidenceSettled: true,
      categories: [passed, { ...report.categories[0]!, id: "modelOverthinking" }],
    })

    await act(async () => session.refresh())
    expect(screen.getByRole("button", { name: /Unused skills, Passed/ })).toBeVisible()
  })

  it("keeps terminal uncertainty and pending completion separate from unanswered targets", async () => {
    setup(null, false, aggregate, {
      ...report,
      categories: [
        {
          ...report.categories[2]!,
          id: "ignoredInstructions",
          checking: false,
          sampled: true,
          partialContext: true,
          reviewCoverage: {
            reviewed: 2,
            total: 4,
            uncertain: 1,
            pending: 2,
            pendingCompletion: 1,
            continuing: false,
          },
        },
      ],
    })
    fireEvent.click(await screen.findByRole("button", { name: "Not assessed (1)" }))
    fireEvent.click(
      await screen.findByRole("button", { name: "Ignored instructions, Not assessed" }),
    )
    const row = await screen.findByRole("button", {
      name: /Ignored instructions, Not assessed/,
    })
    expect(within(row).queryByText(/reviewed/)).not.toBeInTheDocument()
    expect(screen.queryByText("Checking")).not.toBeInTheDocument()
    fireEvent.focus(screen.getByLabelText("This check has been sampled"))
    const tooltip = await screen.findByRole("tooltip")
    expect(within(tooltip).getByText(/Pending completion is a reviewed answer/)).toBeVisible()
    expect(within(tooltip).queryByText("Review is continuing.")).not.toBeInTheDocument()
  })

  it("shows known review percentages without inventing an outcome breakdown", async () => {
    setup(null, false, aggregate, {
      ...report,
      categories: [
        {
          ...report.categories[2]!,
          id: "ignoredInstructions",
          checking: true,
          sampled: true,
          reviewCoverage: {
            reviewed: 2,
            total: 3,
            uncertain: null,
            pending: 1,
            pendingCompletion: null,
            continuing: false,
          },
        },
      ],
    })
    const row = await screen.findByRole("button", { name: /, Checking/ })
    expect(within(row).getByText("Checking")).toBeVisible()
    expect(within(row).queryByText(/reviewed/)).not.toBeInTheDocument()
    expect(screen.queryByText(/0 uncertain/)).not.toBeInTheDocument()
  })
  it.each([
    "ignoredInstructions",
    "scopeCreep",
    "overExploring",
    "skillOpportunities",
  ] as const)("explains the rounded review percentage for %s in both notices", async (id) => {
    setup(null, false, aggregate, {
      ...report,
      categories: [
        {
          ...report.categories[2]!,
          id,
          checking: true,
          sampled: true,
          partialContext: true,
          reviewCoverage: {
            reviewed: 2,
            total: 3,
            uncertain: 1,
            pending: 1,
            pendingCompletion: 0,
            continuing: false,
          },
        },
      ],
    })
    const row = await screen.findByRole("button", { name: /, Checking/ })
    expect(within(row).getByText("Checking")).toBeVisible()
    expect(within(row).queryByText(/reviewed/)).not.toBeInTheDocument()
    for (const label of ["This check has been sampled", "This check used partial context"]) {
      const notice = screen.getByLabelText(label)
      fireEvent.focus(notice)
      const tooltip = await screen.findByRole("tooltip")
      expect(
        within(tooltip).getByText("67% of review targets have been reviewed."),
      ).toBeVisible()
      expect(
        within(tooltip).getByText(/The review target is 50% of eligible targets/),
      ).toHaveTextContent("not a confidence score or a guarantee that no issues remain")
      fireEvent.blur(notice)
      await waitFor(() => expect(screen.queryByRole("tooltip")).not.toBeInTheDocument())
    }
  })
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
      reviewCoverage: {
        reviewed: 2,
        total: null,
        uncertain: 1,
        pending: 3,
        pendingCompletion: 0,
        continuing: true,
      },
    }
    const { adapter, session } = setup(null, false, aggregate, {
      ...report,
      categories: [check],
    })
    const row = await screen.findByRole("button", { name: /, Checking/ })
    expect(within(row).getByText("Checking")).toHaveClass("activity-row-title-shimmer")
    expect(within(row).queryByText("No issues found yet")).not.toBeInTheDocument()
    expect(within(row).getByText("Checking")).toBeVisible()
    expect(screen.queryByRole("button", { name: /Not assessed/ })).not.toBeInTheDocument()
    expect(adapter.getTargets).not.toHaveBeenCalled()
    const partial = screen.getByLabelText("This check used partial context")
    expect(partial).toHaveAttribute("tabindex", "0")
    fireEvent.focus(partial)
    expect(await screen.findByText(/Missing context can limit the assessment/)).toBeVisible()
    expect(screen.getByText(/The review percentage is unknown/)).toBeVisible()
    fireEvent.blur(partial)
    vi.mocked(adapter.getReport).mockResolvedValue({
      ...report,
      evidenceSettled: true,
      categories: [{ ...check, checking: false, clean: 1, lifecycle: "passing" }],
    })
    await act(async () => session.refresh())
    await screen.findByRole("button", { name: /, Passed/ })
    expect(screen.queryByText("Checking")).not.toBeInTheDocument()
    expect(screen.queryByText("No issues found yet")).not.toBeInTheDocument()
    fireEvent.focus(screen.getByLabelText("This check has been sampled"))
    await screen.findByRole("tooltip")
    expect(screen.queryByText("Review is continuing.")).not.toBeInTheDocument()
    vi.mocked(adapter.getReport).mockResolvedValue({
      ...report,
      categories: [{ ...check, finding: 1, lifecycle: "failing" }],
    })
    await act(async () => session.refresh())
    await screen.findByRole("button", { name: /1 failed · 0 passed · checking/ })
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
