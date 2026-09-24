import { act, fireEvent, screen, waitFor, within } from "@testing-library/react"
import { useSyncExternalStore } from "react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { BurnCheckTargetPayload, ChecksReportPayload } from "../../../../lib/insightsIpc"
import type * as ClipboardModule from "../../../../lib/clipboard"
import type * as InsightsIpcModule from "../../../../lib/insightsIpc"
import type * as IpcModule from "../../../../lib/ipc"
import * as SnoozedBurnChecks from "../../../../lib/snoozedBurnChecks"
import { BurnChecksView } from "../../BurnChecksView"

import {
  report,
  namedTargetReport,
  passedTargetReport,
  target,
  aggregate,
  deferred,
  setWindowWidth,
  setup,
  installBurnChecksCommandMocks,
  restoreBurnChecksTestWindow,
} from "./burnChecksTestSupport"

const commands = vi.hoisted(() => ({
  prepare: vi.fn(),
  apply: vi.fn(),
  copy: vi.fn(),
  copyFallback: vi.fn(),
  copyBatch: vi.fn(),
  writeClipboardText: vi.fn(),
  openSample: vi.fn(),
  noteInteraction: vi.fn(),
}))

vi.mock("../../../../lib/insightsIpc", async (importOriginal) => ({
  ...(await importOriginal<typeof InsightsIpcModule>()),
  prepareAutoFixBurnCheckTarget: commands.prepare,
  applyPreparedBurnCheckOperation: commands.apply,
  copyPromptFixBurnCheckTarget: commands.copy,
  copyPromptFixBurnCheck: commands.copyFallback,
  copyPromptFixBurnCheckTargets: commands.copyBatch,
  openBurnCheckSample: commands.openSample,
}))

vi.mock("../../../../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof IpcModule>()),
  noteInteraction: commands.noteInteraction,
}))

vi.mock("../../../../lib/clipboard", async (importOriginal) => ({
  ...(await importOriginal<typeof ClipboardModule>()),
  writeClipboardText: commands.writeClipboardText,
}))

beforeEach(() => {
  installBurnChecksCommandMocks(commands)
})

afterEach(() => {
  restoreBurnChecksTestWindow()
})

// This file holds the grouping and check state tests for the Burn Checks view.
// The suite is split across files in this folder by theme. Each test renders
// the full view. CI runs this file next to the other heavy view suites, so
// one test can take five times its local run time. 15 s is the bound, not a
// target.
describe("BurnChecksView grouping", { timeout: 15_000 }, () => {
  it("opens passed checks when a live report changes from failed to pass-only", async () => {
    setWindowWidth(1400)
    const { adapter, session } = setup(target, false, aggregate, {
      ...report,
      categories: [report.categories[0]!],
    })

    expect(
      await screen.findByRole("button", { name: /Old model usage.*1 failed/ }),
    ).toHaveAttribute("aria-pressed", "true")
    vi.mocked(adapter.getReport).mockResolvedValue({
      ...report,
      categories: [{ ...report.categories[0]!, finding: 0, clean: 4, lifecycle: "passing" }],
    })
    act(() => session.refresh())

    expect(await screen.findByRole("button", { name: /Passed checks/ })).toHaveAttribute(
      "aria-expanded",
      "true",
    )
    expect(screen.getByRole("button", { name: /Passed checks/ })).toHaveAttribute(
      "aria-controls",
      "burn-checks-passed-body",
    )
    expect(document.getElementById("burn-checks-passed-body")).toBeVisible()
    expect(screen.getByRole("button", { name: /Old model usage.*Passed/ })).toHaveAttribute(
      "aria-pressed",
      "true",
    )
  })

  it("moves focus to the Passed trigger before hiding a focused row", async () => {
    setup(target, false, aggregate, passedTargetReport)
    const trigger = await screen.findByRole("button", { name: "Passed checks 2" })
    const row = screen.getByRole("button", { name: /Old model usage.*Passed/ })
    row.focus()

    fireEvent.click(trigger)

    await waitFor(() => expect(trigger).toHaveFocus())
    expect(row).not.toBeVisible()
  })

  it("selects the first assessed check when unavailable evidence becomes assessed", async () => {
    setWindowWidth(1400)
    const { adapter, session } = setup(null, false, aggregate, {
      ...report,
      categories: [report.categories[2]!],
    })

    expect(screen.queryByText("1 check not assessed.")).not.toBeInTheDocument()
    vi.mocked(adapter.getReport).mockResolvedValue({
      ...report,
      categories: [{ ...report.categories[1]!, finding: 0, clean: 3, lifecycle: "passing" }],
    })
    act(() => session.refresh())

    expect(
      await screen.findByRole("button", { name: /Unused skills.*Passed/ }),
    ).toHaveAttribute("aria-pressed", "true")
  })

  it("does not resurrect a removed selection when its category returns", async () => {
    setWindowWidth(1400)
    const twoFailures: ChecksReportPayload = {
      ...report,
      categories: [
        report.categories[0]!,
        {
          id: "modelOverthinking",
          finding: 2,
          clean: 1,
          unavailable: 0,
          estimatedTokenBurnBasisPoints: 400,
          lifecycle: "failing",
        },
      ],
    }
    const { adapter, session } = setup(target, false, aggregate, twoFailures)
    const overthinking = await screen.findByRole("button", { name: /Model overthinking/ })
    fireEvent.click(overthinking)
    expect(overthinking).toHaveAttribute("aria-pressed", "true")

    vi.mocked(adapter.getReport).mockResolvedValue({
      ...report,
      categories: [report.categories[0]!],
    })
    act(() => session.refresh())
    const oldModel = await screen.findByRole("button", { name: /Old model usage/ })
    await waitFor(() => expect(oldModel).toHaveAttribute("aria-pressed", "true"))

    vi.mocked(adapter.getReport).mockResolvedValue(twoFailures)
    act(() => session.refresh())
    await screen.findByRole("button", { name: /Model overthinking/ })
    expect(screen.getByRole("button", { name: /Old model usage/ })).toHaveAttribute(
      "aria-pressed",
      "true",
    )
  })

  it("shows Snoozed as an empty group", async () => {
    setup(target, false, aggregate, report)
    const snoozed = await screen.findByRole("button", { name: "Snoozed 0" })
    expect(snoozed).toHaveAttribute("aria-expanded", "false")
    fireEvent.click(snoozed)
    expect(snoozed).toHaveAttribute("aria-expanded", "true")
    expect(screen.queryByText(/Reminders are coming soon/)).not.toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Snooze" })).not.toHaveAttribute("aria-disabled")
    fireEvent.click(snoozed)
    expect(snoozed).toHaveAttribute("aria-expanded", "false")
  })

  describe("check state labels", () => {
    const watchingTarget: BurnCheckTargetPayload = {
      ...target,
      watch: {
        watchId: "watch-1",
        origin: "action",
        lifecycle: "watching",
        verification: { status: "watching" },
        savings: { status: "pending" },
      },
    }
    const awaitingReport: ChecksReportPayload = {
      ...report,
      categories: report.categories.map((check) =>
        check.id === "oldModelUsage" ? { ...check, lifecycle: "awaitingVerification" } : check,
      ),
    }

    function mockSnoozes(initial: readonly SnoozedBurnChecks.SnoozedBurnCheck[]) {
      let records = initial
      let snapshot = { status: "ready" as const, records }
      const listeners = new Set<() => void>()
      const subscribe = (listener: () => void) => {
        listeners.add(listener)
        return () => {
          listeners.delete(listener)
        }
      }
      const getSnapshot = () => snapshot
      vi.spyOn(SnoozedBurnChecks, "useSnoozedBurnChecks").mockImplementation(
        function useSnoozedChecksSnapshot() {
          return useSyncExternalStore(subscribe, getSnapshot)
        },
      )
      return (next: readonly SnoozedBurnChecks.SnoozedBurnCheck[]) => {
        records = next
        snapshot = { status: "ready", records }
        for (const listener of listeners) listener()
      }
    }

    afterEach(() => vi.restoreAllMocks())

    it.each([false, true])(
      "search targets an awaiting check once with snoozed=%s",
      async (snoozed) => {
        HTMLElement.prototype.scrollIntoView = vi.fn()
        mockSnoozes(snoozed ? [{ detector: "oldModelUsage", scope: "check", until: null }] : [])
        const { session, view } = setup(watchingTarget, false, aggregate, awaitingReport)
        await waitFor(() => expect(session.getSnapshot().report).not.toBeNull())
        view.rerender(
          <BurnChecksView
            active
            session={session}
            focusedCheck="oldModelUsage"
            focusRevision={1}
          />,
        )
        const rows = screen.getAllByRole("button", { name: /Old model usage/ })
        expect(rows).toHaveLength(1)
        expect(rows[0]).toHaveAttribute("aria-pressed", "true")
        expect(rows[0]).toHaveFocus()
        expect(
          screen.queryByText("This check has not been assessed for the available sessions."),
        ).not.toBeInTheDocument()
        if (snoozed)
          expect(screen.getByRole("button", { name: "Snoozed 1" })).toHaveAttribute(
            "aria-expanded",
            "true",
          )
      },
    )

    it("keeps a zero-count awaiting search result assessed", async () => {
      HTMLElement.prototype.scrollIntoView = vi.fn()
      const { session, view } = setup(watchingTarget, false, aggregate, {
        ...awaitingReport,
        categories: awaitingReport.categories.map((check) =>
          check.id === "oldModelUsage" ? { ...check, finding: 0, clean: 0 } : check,
        ),
      })
      await screen.findByRole("button", { name: /Old model usage/ })
      view.rerender(
        <BurnChecksView
          active
          session={session}
          focusedCheck="oldModelUsage"
          focusRevision={1}
        />,
      )
      const row = screen.getByRole("button", { name: /Old model usage, Awaiting verification/ })
      expect(row).toHaveAttribute("aria-pressed", "true")
      expect(row).toHaveFocus()
      expect(screen.queryByText("Not assessed")).not.toBeInTheDocument()
      expect(
        screen.queryByText("This check has not been assessed for the available sessions."),
      ).not.toBeInTheDocument()
    })

    it("shows awaiting from the report before target details load", async () => {
      const pending = deferred<BurnCheckTargetPayload[]>()
      setup(pending.promise, false, aggregate, awaitingReport)
      const heading = await screen.findByRole("heading", { name: "Old model usage", level: 2 })
      const header = heading.closest("header")!
      const badge = await within(header).findByText("Awaiting verification")
      expect(badge.closest("button")).toBeNull()
      expect(header).toHaveTextContent("1 failed")
      expect(header).toHaveTextContent("2 passed")
      const groupHeading = screen.getByRole("heading", { name: "Awaiting verification 1" })
      expect(groupHeading.querySelector("svg")).toHaveAttribute("aria-hidden", "true")
      expect(
        within(groupHeading.closest("section")!).getByRole("button", {
          name: /Old model usage/,
        }),
      ).toHaveAttribute("aria-pressed", "true")
      expect(
        screen.queryByText("A later complete session confirms each change."),
      ).not.toBeInTheDocument()
      await act(async () => pending.resolve([watchingTarget]))
    })

    it.each([
      { name: "ordinary findings", targets: [target] },
      { name: "empty target details", targets: [] },
      { name: "mixed verification states", targets: [watchingTarget, target] },
    ])("does not show awaiting for $name", async ({ targets }) => {
      const { session } = setup(targets, false, aggregate, report)
      await waitFor(() =>
        expect(session.getSnapshot().targets.oldModelUsage?.data).toBeDefined(),
      )
      const header = screen
        .getByRole("heading", { name: "Old model usage", level: 2 })
        .closest("header")!
      expect(within(header).queryByText("Awaiting verification")).not.toBeInTheDocument()
      expect(
        screen.queryByRole("heading", { name: /Awaiting verification/ }),
      ).not.toBeInTheDocument()
    })

    it.each([null, new Date("2026-09-25T12:00:00Z").getTime()])(
      "shows snooze %s before awaiting and restores awaiting after unsnooze",
      async (until) => {
        const setSnoozes = mockSnoozes([{ detector: "oldModelUsage", scope: "check", until }])
        const unsnooze = vi.spyOn(SnoozedBurnChecks, "unsnoozeBurnCheck").mockResolvedValue()
        const { session } = setup(watchingTarget, false, aggregate, awaitingReport)
        fireEvent.click(await screen.findByRole("button", { name: "Snoozed 1" }))
        fireEvent.click(screen.getByRole("button", { name: /Old model usage/ }))
        await waitFor(() =>
          expect(session.getSnapshot().targets.oldModelUsage?.data).toBeDefined(),
        )
        const heading = screen.getByRole("heading", { name: "Old model usage", level: 2 })
        const header = heading.closest("header")!
        const label = SnoozedBurnChecks.formatSnoozeUntil(until)
        expect(within(header).getByText(label)).toBeInTheDocument()
        expect(within(header).queryByText("Awaiting verification")).not.toBeInTheDocument()
        expect(
          screen.queryByRole("heading", { name: /Awaiting verification/ }),
        ).not.toBeInTheDocument()
        fireEvent.click(within(header).getByRole("button", { name: "Unsnooze" }))
        expect(unsnooze).toHaveBeenCalledWith("oldModelUsage")

        act(() => setSnoozes([]))

        expect(await within(header).findByText("Awaiting verification")).toBeInTheDocument()
        expect(within(header).queryByText(label)).not.toBeInTheDocument()
        expect(within(header).getByRole("button", { name: "Snooze" })).toBeInTheDocument()
        expect(screen.getByRole("button", { name: /Old model usage/ })).toHaveAttribute(
          "aria-pressed",
          "true",
        )
      },
    )

    it("removes the header state when an ordinary finding's snooze expires", async () => {
      const until = new Date("2026-09-25T12:00:00Z").getTime()
      const setSnoozes = mockSnoozes([{ detector: "oldModelUsage", scope: "check", until }])
      setup(target, false, aggregate, report)
      fireEvent.click(await screen.findByRole("button", { name: "Snoozed 1" }))
      fireEvent.click(screen.getByRole("button", { name: /Old model usage/ }))
      const header = screen
        .getByRole("heading", { name: "Old model usage", level: 2 })
        .closest("header")!
      const label = SnoozedBurnChecks.formatSnoozeUntil(until)
      expect(within(header).getByText(label)).toBeInTheDocument()

      act(() => setSnoozes([]))

      expect(within(header).queryByText(label)).not.toBeInTheDocument()
      expect(within(header).queryByText("Awaiting verification")).not.toBeInTheDocument()
      expect(screen.getByRole("button", { name: /Old model usage/ })).toHaveAttribute(
        "aria-pressed",
        "true",
      )
    })

    it("keeps running checks visible when every assessed check is snoozed", async () => {
      mockSnoozes([
        { detector: "oldModelUsage", scope: "check", until: null },
        { detector: "unusedSkills", scope: "check", until: null },
      ])
      setup(target, false, aggregate, report)

      expect(await screen.findByRole("button", { name: "Snoozed 2" })).toBeVisible()
      expect(screen.queryByRole("button", { name: /Old model usage/ })).not.toBeInTheDocument()
      expect(screen.queryByRole("button", { name: /Unused skills/ })).not.toBeInTheDocument()
      expect(screen.queryByRole("heading", { name: "Failed checks 1" })).not.toBeInTheDocument()
      expect(screen.queryByRole("region", { name: "Savings" })).not.toBeInTheDocument()
      expect(screen.queryByText("No active checks.")).not.toBeInTheDocument()
    })

    it("moves focus to the Snoozed trigger before hiding a focused row", async () => {
      mockSnoozes([{ detector: "oldModelUsage", scope: "check", until: null }])
      setup(target, false, aggregate, report)
      const trigger = await screen.findByRole("button", { name: "Snoozed 1" })
      fireEvent.click(trigger)
      const row = screen.getByRole("button", { name: /Old model usage/ })
      row.focus()

      fireEvent.click(trigger)

      await waitFor(() => expect(trigger).toHaveFocus())
      expect(row).not.toBeVisible()
    })

    it("moves focus to the collapsed Snoozed trigger when a focused row is regrouped", async () => {
      const setSnoozes = mockSnoozes([])
      setup(target, false, aggregate, report)
      const row = await screen.findByRole("button", { name: /Old model usage/ })
      row.focus()

      act(() => setSnoozes([{ detector: "oldModelUsage", scope: "check", until: null }]))

      const trigger = screen.getByRole("button", { name: "Snoozed 1" })
      await waitFor(() => expect(trigger).toHaveFocus())
      expect(row).not.toBeInTheDocument()
    })

    it("keeps ordinary passed checks free of state badges", async () => {
      setup(target, false, aggregate, report)
      fireEvent.click(await screen.findByRole("button", { name: "Passed checks 1" }))
      fireEvent.click(screen.getByRole("button", { name: /Unused skills/ }))
      const header = screen
        .getByRole("heading", { name: "Unused skills", level: 2 })
        .closest("header")!
      expect(within(header).queryByText("Awaiting verification")).not.toBeInTheDocument()
      expect(within(header).queryByText(/Snoozed/)).not.toBeInTheDocument()
    })
  })

  it("moves focus to the collapsed Passed trigger when a focused row starts passing", async () => {
    const secondFailure = {
      id: "modelOverthinking" as const,
      finding: 2,
      clean: 1,
      unavailable: 0,
      estimatedTokenBurnBasisPoints: 400,
      lifecycle: "failing" as const,
    }
    const twoFailures = { ...report, categories: [report.categories[0]!, secondFailure] }
    const { adapter, session } = setup(target, false, aggregate, twoFailures)
    const row = await screen.findByRole("button", { name: /Model overthinking/ })
    row.focus()
    vi.mocked(adapter.getReport).mockResolvedValue({
      ...twoFailures,
      categories: [
        report.categories[0]!,
        { ...secondFailure, finding: 0, clean: 3, lifecycle: "passing" },
      ],
    })

    act(() => session.refresh())

    const trigger = await screen.findByRole("button", { name: "Passed checks 1" })
    await waitFor(() => expect(trigger).toHaveFocus())
    expect(row).not.toBeInTheDocument()
  })

  it("shows a pass-only outcome and opens its counted disclosure", async () => {
    setup(
      null,
      false,
      { wins: [] },
      {
        ...report,
        estimatedTokenBurnBasisPoints: 0,
        categories: [
          {
            id: "unusedSkills",
            finding: 0,
            clean: 4,
            unavailable: 0,
            estimatedTokenBurnBasisPoints: 0,
            lifecycle: "passing",
          },
        ],
      },
    )

    expect(await screen.findByRole("button", { name: "Passed checks 1" })).toBeVisible()
    expect(screen.queryByRole("heading", { name: /Failed checks/ })).not.toBeInTheDocument()
    expect(screen.getByRole("button", { name: /Passed checks/ })).toHaveAttribute(
      "aria-expanded",
      "true",
    )
  })

  it("renders assessed checks and concise failed details", async () => {
    setup(target, false, aggregate, report)
    expect(await screen.findByText("1 session affected")).toBeVisible()
    const row = await screen.findByRole("button", {
      name: /Old model usage.*8% estimated burn/,
    })
    expect(row).toBeVisible()
    expect(within(row).getByText("8% estimated burn")).toBeVisible()
    expect(row.querySelector(".lucide-flame")).toBeInTheDocument()
    expect(row.querySelector('[role="meter"]')).not.toBeInTheDocument()
    expect(within(row).getByText("2 passed")).toHaveClass("text-label-secondary")
    expect(screen.getByRole("heading", { name: "Failed checks 1" })).toBeVisible()
    expect(screen.queryByText(/More evidence is needed/)).not.toBeInTheDocument()
    expect(screen.getByRole("heading", { name: /Passed checks/ })).toBeVisible()
    expect(screen.queryByRole("heading", { name: "Not assessed" })).not.toBeInTheDocument()
    expect(screen.queryByText("Excess cache rehydration")).not.toBeInTheDocument()
    expect(screen.queryByText(/check results are assessed/)).not.toBeInTheDocument()
    expect(screen.queryByText(/Evidence work is still in progress/)).not.toBeInTheDocument()
    expect(screen.queryByRole("region", { name: "Your savings" })).not.toBeInTheDocument()
    expect(
      await screen.findByText(
        "Some sessions used an older model when a newer one was available.",
      ),
    ).toBeVisible()
    expect(screen.queryByRole("heading", { name: "claude-opus-4-6" })).not.toBeInTheDocument()
    expect(
      screen.queryByText("The old model handled 8 eligible requests."),
    ).not.toBeInTheDocument()
    expect(screen.queryByText("Exact facts")).not.toBeInTheDocument()
    expect(screen.queryByText("What we found")).not.toBeInTheDocument()
    expect(screen.queryByText("Why it matters")).not.toBeInTheDocument()
    expect(screen.queryByText("Suggested change")).not.toBeInTheDocument()
    expect(screen.queryByText(/Verification requires fresh evidence/)).not.toBeInTheDocument()
  })

  it("groups wide check rows and exposes the selected outcome state", async () => {
    setWindowWidth(1400)
    setup()

    const selected = await screen.findByRole("button", { name: /Unused MCP servers/ })
    expect(selected).toHaveAttribute("aria-pressed", "true")
    expect(selected).toHaveAttribute("data-outcome", "failed")
    expect(selected.querySelector(".lucide-chevron-right")).not.toBeInTheDocument()
    expect(selected.querySelector(".burn-check-category-icon")).toHaveClass("text-check-mcp")
    expect(selected.closest(".burn-checks-group-body")).toBeInTheDocument()
  })

  it("shows the named resource count in the outcome line", async () => {
    setWindowWidth(1400)
    const first = setup(target, true)

    expect(await screen.findByText(/At least 1 affected MCP server$/)).toBeVisible()
    first.view.unmount()

    setup(target)
    expect(await screen.findByText(/1 affected MCP server$/)).toBeVisible()
  })

  it("uses report session totals when named targets share bounded samples", async () => {
    setup(
      [target, { ...target, findingId: "second-target", actionId: "second-action" }],
      true,
      aggregate,
      {
        ...namedTargetReport,
        categories: [{ ...namedTargetReport.categories[0]!, finding: 23 }],
      },
    )

    const resources = await screen.findByText(/At least 2 affected MCP servers$/)
    expect(resources).toHaveTextContent("At least 2 affected MCP servers")
    expect(screen.queryByText("23 sessions affected")).not.toBeInTheDocument()
    expect(screen.getByRole("region", { name: "Burn check details" })).toHaveTextContent(
      "23 failed",
    )
  })
})
