import { createElement } from "react"
import { act, fireEvent, screen, waitFor, within } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

import { searchApp } from "../../../lib/appSearch"
import { BurnChecksView } from "../BurnChecksView"
import { installedCheckFixture } from "./tests/installedCheckFixture"
import { aggregate, report, setup } from "./tests/burnChecksTestSupport"

describe("BurnChecksReport", () => {
  it.each(["macos", "windows", "linux"] as const)(
    "focuses the %s local check search destination without changing values",
    async (platform) => {
      HTMLElement.prototype.scrollIntoView = vi.fn()
      const result = searchApp("sessionsOverDepth", platform)[0]!
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
      const row = await screen.findByRole("button", { name: "Session overdepth, Not assessed" })
      await waitFor(() => expect(row).toHaveFocus())
      expect(row).toHaveAttribute("aria-pressed", "true")
      expect(categories).toEqual(before)
      expect(adapter.getReport).toHaveBeenCalledOnce()
      expect(screen.queryByRole("button", { name: "Fix" })).not.toBeInTheDocument()
      expect(screen.queryByRole("button", { name: "Copy fix prompt" })).not.toBeInTheDocument()
    },
  )

  it("keeps a search-selected check reachable when it moves into Passed", async () => {
    const check = { ...report.categories[2]!, id: "sessionsOverDepth" as const }
    const { adapter, session, view } = setup(null, false, aggregate, {
      ...report,
      categories: [check],
    })
    view.rerender(
      createElement(BurnChecksView, {
        active: true,
        session,
        focusedCheck: check.id,
        focusRevision: 1,
      }),
    )
    await screen.findByRole("button", { name: "Session overdepth, Not assessed" })
    vi.mocked(adapter.getReport).mockResolvedValue({
      ...report,
      categories: [{ ...check, clean: 1, lifecycle: "passing" }],
    })
    await act(async () => session.refresh())
    expect(await screen.findByRole("button", { name: "Passed checks 1" })).toHaveAttribute(
      "aria-expanded",
      "true",
    )
    const row = screen.getByRole("button", { name: "Session overdepth, Passed" })
    expect(row).toHaveAttribute("aria-pressed", "true")
    expect(row).toHaveFocus()
  })

  it("opens Not assessed when a search-selected check moves there", async () => {
    const check = {
      ...report.categories[2]!,
      id: "sessionsOverDepth" as const,
      clean: 1,
      lifecycle: "passing" as const,
    }
    const { adapter, session, view } = setup(null, false, aggregate, {
      ...report,
      categories: [check],
    })
    view.rerender(
      createElement(BurnChecksView, {
        active: true,
        session,
        focusedCheck: check.id,
        focusRevision: 1,
      }),
    )
    await screen.findByRole("button", { name: "Session overdepth, Passed" })
    vi.mocked(adapter.getReport).mockResolvedValue({
      ...report,
      categories: [{ ...check, clean: 0, lifecycle: null, finding: 0 }],
    })
    await act(async () => session.refresh())
    expect(await screen.findByRole("button", { name: "Not assessed (1)" })).toHaveAttribute(
      "aria-expanded",
      "true",
    )
    const row = screen.getByRole("button", { name: "Session overdepth, Not assessed" })
    expect(row).toHaveAttribute("aria-pressed", "true")
    expect(row).toHaveFocus()
  })

  it("searches an installed check into the normal failure group and current detail pane", async () => {
    HTMLElement.prototype.scrollIntoView = vi.fn()
    const installedCheck = { ...installedCheckFixture }
    const { session, view, adapter } = setup(null, false, aggregate, {
      ...report,
      installedChecks: [installedCheck],
    })
    const destination = searchApp("repeated failures", "macos", true, [
      {
        effectiveId: installedCheck.effectiveId,
        name: installedCheck.name,
        aliases: installedCheck.aliases,
        availability: installedCheck.availability,
      },
    ])[0]!
    expect(destination.target).toEqual({
      kind: "installedCheck",
      check: installedCheck.effectiveId,
    })
    view.rerender(
      createElement(BurnChecksView, {
        active: true,
        session,
        focusedCheck: installedCheck.effectiveId,
        focusRevision: 1,
      }),
    )
    const row = await screen.findByRole("button", {
      name: "Failed retry loop, 1 failed · 1 passed",
    })
    await waitFor(() => expect(row).toHaveFocus())
    expect(adapter.getReport).toHaveBeenCalledOnce()
    fireEvent.click(row)
    expect(row).toHaveAttribute("aria-pressed", "true")
    const detail = within(screen.getByRole("region", { name: "Burn check details" }))
    expect(detail.getByRole("heading", { name: installedCheck.name, level: 2 })).toBeVisible()
    expect(detail.getAllByText("Review the recorded work.")[0]).toBeVisible()
    expect(screen.queryByText("Installed checks")).not.toBeInTheDocument()
    expect(
      screen.queryByText(/Probabilities:|Assessment history|Generation:/),
    ).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: /Old model usage, / }))
    expect(detail.getByRole("heading", { name: "Old model usage", level: 2 })).toBeVisible()
  })
})
