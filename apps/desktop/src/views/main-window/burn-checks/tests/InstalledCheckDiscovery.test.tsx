import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { beforeEach, expect, it, vi } from "vitest"

import { searchApp } from "../../../../lib/appSearch"
import { emptyCheckAvailability } from "../../../../lib/checkAvailability"
import type {
  ChecksReportPayload,
  InstalledCheckReportPayload,
} from "../../../../lib/insightsIpc"
import { checkRows } from "../../../../lib/presentation/checkRows"
import type { SnoozedBurnCheck } from "../../../../lib/snoozedBurnChecks"
import { BurnChecksView } from "../../BurnChecksView"
import { BurnChecksController } from "../BurnChecksController"
import { deferred, report, setup } from "./burnChecksTestSupport"
import {
  installedCheckDiagnosticFixture,
  installedCheckEvidenceFixture,
  installedCheckFixture,
} from "./installedCheckFixture"

type Handler = (event: { payload: unknown }) => void
const shell = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn(),
  handlers: new Map<string, Set<Handler>>(),
}))
vi.mock("@tauri-apps/api/core", () => ({ invoke: shell.invoke, isTauri: () => true }))
vi.mock("@tauri-apps/api/event", () => ({ listen: shell.listen }))

let currentReport: ChecksReportPayload
let currentSnoozes: SnoozedBurnCheck[]

function emit(event: string, payload: unknown = null) {
  const handlers = shell.handlers.get(event)
  expect(handlers?.size).toBeGreaterThan(0)
  for (const handler of handlers!) handler({ payload })
}

beforeEach(() => {
  HTMLElement.prototype.scrollIntoView = vi.fn()
  currentReport = { ...report, categories: [], installedChecks: [installedCheckFixture] }
  currentSnoozes = []
  shell.handlers.clear()
  shell.listen.mockImplementation(async (event: string, handler: Handler) => {
    const handlers = shell.handlers.get(event) ?? new Set<Handler>()
    shell.handlers.set(event, handlers)
    handlers.add(handler)
    return () => handlers.delete(handler)
  })
  shell.invoke.mockReset().mockImplementation(async (command: string) => {
    switch (command) {
      case "get_main_window_visible":
        return true
      case "get_checks_report":
        return currentReport
      case "list_burn_check_snoozes":
        return currentSnoozes
      case "get_burn_check_aggregate_wins":
        return { wins: [] }
      case "list_burn_check_targets":
        return { targets: [], samples: [], truncated: false }
      case "get_installed_check_evidence":
        return installedCheckEvidenceFixture
      case "cancel_checks_report":
      case "note_interaction":
        return undefined
      default:
        throw new Error(`Unexpected command: ${command}`)
    }
  })
})

it("keeps native checks but adds no YAML cards or navigation without discovery", async () => {
  const snapshot = { ...emptyCheckAvailability, configured: false }
  const nativeReport = { ...report, installedChecks: [] }
  setup(null, false, { wins: [] }, nativeReport)
  expect(await screen.findByRole("button", { name: /Old model usage/ })).toBeVisible()
  expect(checkRows(nativeReport)).toEqual(report.categories)
  expect(document.querySelector('[aria-controls^="burn-check-installed:"]')).toBeNull()
  expect(
    searchApp("", "macos", snapshot.configured, snapshot.installedChecks).filter(
      ({ target }) => target.kind === "installedCheck",
    ),
  ).toEqual([])
  expect(
    searchApp("Model-based Checks", "macos", snapshot.configured, snapshot.installedChecks),
  ).toContainEqual(
    expect.objectContaining({
      target: { kind: "setting", control: "smartChecksEnabled" },
    }),
  )
})

it.each([false, true])(
  "shows discovered global and repository checks without a provider when enablement is %s",
  async (yamlEnabled) => {
    const definitions: Pick<InstalledCheckReportPayload, "effectiveId" | "name">[] = [
      { effectiveId: "installed:global-retries", name: "Global retries" },
      { effectiveId: "installed:repo-retries", name: "Repository retries" },
    ]
    const discovered: InstalledCheckReportPayload[] = definitions.map((definition) => ({
      ...installedCheckFixture,
      ...definition,
      aliases: [],
      availability: { status: "paused" },
      finding: 0,
      clean: 0,
      hasStarted: false,
      checkingCount: 0,
      targets: [],
    }))
    const snapshot = {
      ...emptyCheckAvailability,
      yamlEnabled,
      configured: false,
      installedChecks: discovered,
    }
    const mixedReport = { ...report, installedChecks: discovered }
    const { session, view } = setup(null, false, { wins: [] }, mixedReport)
    await screen.findByRole("button", { name: /Old model usage/ })
    expect(checkRows(mixedReport).map(({ id }) => id)).toEqual([
      ...report.categories.map(({ id }) => id),
      ...discovered.map(({ effectiveId }) => effectiveId),
    ])

    for (const [index, check] of discovered.entries()) {
      const result = searchApp(
        check.name,
        "macos",
        snapshot.configured,
        snapshot.installedChecks,
      )[0]!
      expect(result.target).toEqual({ kind: "installedCheck", check: check.effectiveId })
      view.rerender(
        <BurnChecksView
          active
          session={session}
          focusedCheck={check.effectiveId}
          focusRevision={index + 1}
        />,
      )
      const row = await screen.findByRole("button", { name: `${check.name}, Not assessed` })
      expect(row).toBeVisible()
      await waitFor(() => expect(row).toHaveFocus())
      expect(screen.getByRole("heading", { name: check.name })).toBeVisible()
    }
  },
)

it.each([false, true])(
  "closes a removed searched detail and evidence, including snoozed: %s",
  async (snoozed) => {
    const installed = {
      ...installedCheckFixture,
      diagnostics: [installedCheckDiagnosticFixture],
    }
    currentReport = { ...currentReport, installedChecks: [installed] }
    if (snoozed)
      currentSnoozes = [{ detector: installed.effectiveId, scope: "check", until: null }]
    const session = new BurnChecksController()
    const view = render(
      <BurnChecksView
        active
        session={session}
        focusedCheck={installed.effectiveId}
        focusRevision={1}
      />,
    )
    const row = await screen.findByRole("button", { name: /Failed retry loop, / })
    expect(row).toHaveFocus()
    expect(
      screen.getByLabelText("1 check type in total, including snoozed checks"),
    ).toBeVisible()
    expect(screen.getByText("Input too large")).toBeVisible()
    fireEvent.click(screen.getByRole("button", { name: "Show source details" }))
    expect(await screen.findByRole("heading", { name: "Assessed input" })).toBeVisible()
    const detail = document.getElementById(`burn-check-${installed.effectiveId}-detail`)!
    act(() => detail.focus())
    expect(detail).toHaveFocus()

    currentReport = { ...currentReport, installedChecks: [] }
    await act(async () => emit("checks:report-changed"))

    expect(
      screen.queryByRole("button", { name: /Failed retry loop, / }),
    ).not.toBeInTheDocument()
    expect(detail).not.toBeInTheDocument()
    expect(screen.queryByText("Input too large")).not.toBeInTheDocument()
    expect(screen.queryByRole("heading", { name: "Assessed input" })).not.toBeInTheDocument()
    expect(screen.queryByRole("heading", { name: "failed-retry-loop" })).not.toBeInTheDocument()
    expect(
      screen.queryByText("This check has no result in the current report."),
    ).not.toBeInTheDocument()
    expect(screen.getByText("Select a check to see its result and explanation.")).toBeVisible()
    expect(
      screen.getByLabelText("0 check types in total, including snoozed checks"),
    ).toBeVisible()
    expect(screen.getByRole("button", { name: "Snoozed 0" })).toBeVisible()
    expect(screen.getByRole("region", { name: "Burn check collection" })).toHaveFocus()

    view.rerender(
      <BurnChecksView
        active={false}
        session={session}
        focusedCheck={installed.effectiveId}
        focusRevision={1}
      />,
    )
    view.rerender(
      <BurnChecksView
        active
        session={session}
        focusedCheck={installed.effectiveId}
        focusRevision={1}
      />,
    )
    await act(async () => {})
    expect(screen.getByText("Select a check to see its result and explanation.")).toBeVisible()

    view.rerender(
      <BurnChecksView
        active
        session={session}
        focusedCheck={installed.effectiveId}
        focusRevision={2}
      />,
    )
    expect(screen.getByRole("heading", { name: "failed-retry-loop" })).toBeVisible()
  },
)

it("moves focus from a removed row to a surviving row and updates header counts", async () => {
  currentReport = { ...currentReport, categories: report.categories }
  const session = new BurnChecksController()
  render(<BurnChecksView active session={session} />)
  const row = await screen.findByRole("button", { name: /Failed retry loop, / })
  fireEvent.click(row)
  act(() => row.focus())
  expect(
    screen.getByLabelText("4 check types in total, including snoozed checks"),
  ).toBeVisible()
  expect(screen.getByRole("heading", { name: "Failed checks 2" })).toBeVisible()

  currentReport = { ...currentReport, installedChecks: [] }
  await act(async () => emit("checks:report-changed"))

  const survivor = screen.getByRole("button", { name: /Old model usage, / })
  expect(survivor).toHaveAttribute("aria-pressed", "true")
  expect(survivor).toHaveFocus()
  expect(
    screen.getByLabelText("3 check types in total, including snoozed checks"),
  ).toBeVisible()
  expect(screen.getByRole("heading", { name: "Failed checks 1" })).toBeVisible()
})

it.each([false, true])(
  "does not revive a canceled search after fallback removal, with an empty report between: %s",
  async (emptyBetween) => {
    currentReport = {
      ...currentReport,
      installedChecks: [
        {
          ...installedCheckFixture,
          effectiveId: "installed:other-check",
          name: "Other check",
          targets: [],
        },
      ],
    }
    const session = new BurnChecksController()
    const view = render(<BurnChecksView active session={session} />)
    const other = await screen.findByRole("button", { name: /Other check, / })
    view.rerender(
      <BurnChecksView
        active
        session={session}
        focusedCheck={installedCheckFixture.effectiveId}
        focusRevision={1}
      />,
    )
    expect(screen.getByRole("heading", { name: "failed-retry-loop" })).toBeVisible()

    fireEvent.click(other)
    expect(other).toHaveAttribute("aria-pressed", "true")
    expect(screen.getByRole("heading", { name: "Other check" })).toBeVisible()
    expect(screen.queryByRole("heading", { name: "failed-retry-loop" })).not.toBeInTheDocument()
    if (emptyBetween) {
      currentReport = { ...currentReport, installedChecks: [] }
      await act(async () => emit("checks:report-changed"))
      expect(
        screen.getByText("Select a check to see its result and explanation."),
      ).toBeVisible()
    }

    currentReport = { ...currentReport, installedChecks: [installedCheckFixture] }
    await act(async () => emit("checks:report-changed"))
    expect(screen.getByRole("button", { name: /Failed retry loop, / })).toHaveAttribute(
      "aria-pressed",
      "true",
    )
    const detail = document.getElementById(
      `burn-check-${installedCheckFixture.effectiveId}-detail`,
    )!
    expect(detail).toBeVisible()
    expect(screen.getByRole("heading", { name: "Failed retry loop" })).toBeVisible()

    currentReport = { ...currentReport, installedChecks: [] }
    await act(async () => emit("checks:report-changed"))

    expect(detail).not.toBeInTheDocument()
    expect(
      screen.queryByRole("button", { name: /Failed retry loop, / }),
    ).not.toBeInTheDocument()
    expect(screen.queryByRole("heading", { name: "failed-retry-loop" })).not.toBeInTheDocument()
    expect(
      screen.queryByText("This check has no result in the current report."),
    ).not.toBeInTheDocument()
    expect(screen.getByText("Select a check to see its result and explanation.")).toBeVisible()
  },
)

it.each(["row", "detail"] as const)(
  "does not take focus from a later control after %s removal",
  async (owner) => {
    const session = new BurnChecksController()
    render(
      <>
        <button>Other control</button>
        <BurnChecksView active session={session} />
      </>,
    )
    const row = await screen.findByRole("button", { name: /Failed retry loop, / })
    const focusOwner =
      owner === "row"
        ? row
        : document.getElementById(`burn-check-${installedCheckFixture.effectiveId}-detail`)!
    act(() => focusOwner.focus())
    const pendingFocus: VoidFunction[] = []
    const microtask = vi.spyOn(globalThis, "queueMicrotask").mockImplementation((callback) => {
      pendingFocus.push(callback)
    })
    try {
      currentReport = { ...currentReport, installedChecks: [] }
      await act(async () => emit("checks:report-changed"))
      expect(pendingFocus.length).toBeGreaterThan(0)
      const control = screen.getByRole("button", { name: "Other control" })
      act(() => control.focus())
      act(() => pendingFocus.splice(0).forEach((callback) => callback()))
      expect(control).toHaveFocus()
    } finally {
      microtask.mockRestore()
    }
  },
)

it("does not restore removed rows from an old read after a new visible consumer loads", async () => {
  const session = new BurnChecksController()
  render(
    <BurnChecksView
      active
      session={session}
      focusedCheck={installedCheckFixture.effectiveId}
      focusRevision={1}
    />,
  )
  await screen.findByRole("button", { name: /Failed retry loop, / })
  const oldReport = currentReport
  const oldRead = deferred<ChecksReportPayload>()
  const invoke = shell.invoke.getMockImplementation()!
  let readStarted = false
  shell.invoke.mockImplementation(async (command) => {
    if (command === "get_checks_report") {
      readStarted = true
      return oldRead.promise
    }
    return invoke(command)
  })
  await act(async () => emit("checks:report-changed"))
  expect(readStarted).toBe(true)
  act(() => emit("main:visibility-changed", false))
  currentReport = { ...currentReport, installedChecks: [] }
  shell.invoke.mockImplementation(invoke)
  await act(async () => {
    emit("checks:report-changed")
    emit("main:visibility-changed", true)
  })
  expect(
    screen.getByLabelText("0 check types in total, including snoozed checks"),
  ).toBeVisible()
  await act(async () => oldRead.resolve(oldReport))
  expect(screen.queryByRole("button", { name: /Failed retry loop, / })).not.toBeInTheDocument()
  expect(
    within(screen.getByRole("region", { name: "Burn check details" })).queryByRole("heading"),
  ).not.toBeInTheDocument()
})

it("keeps the same logical row when removal reveals a global definition", async () => {
  const session = new BurnChecksController()
  render(
    <BurnChecksView
      active
      session={session}
      focusedCheck={installedCheckFixture.effectiveId}
      focusRevision={1}
    />,
  )
  const row = await screen.findByRole("button", { name: /Failed retry loop, / })
  currentReport = {
    ...currentReport,
    installedChecks: [
      {
        ...installedCheckFixture,
        name: "Global retries",
        targets: [
          {
            ...installedCheckFixture.targets[0]!,
            sourcePath: "/global/checks/retries.yaml",
            observation: "Global definition finding.",
          },
        ],
      },
    ],
  }
  await act(async () => emit("checks:report-changed"))
  const global = screen.getByRole("button", { name: /Global retries, / })
  expect(global).toBe(row)
  expect(global).toHaveAttribute("aria-pressed", "true")
  expect(global).toHaveFocus()
  expect(screen.getByRole("heading", { name: "Global retries" })).toBeVisible()
  expect(screen.getByText("Global definition finding.")).toBeVisible()
  expect(screen.getByLabelText("1 check type in total, including snoozed checks")).toBeVisible()
})
