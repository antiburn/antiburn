import { act, fireEvent, screen, waitFor, within } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { BurnCheckTargetPayload, ChecksReportPayload } from "../../../../lib/insightsIpc"
import type * as ClipboardModule from "../../../../lib/clipboard"
import type * as InsightsIpcModule from "../../../../lib/insightsIpc"
import type * as IpcModule from "../../../../lib/ipc"
import * as SnoozedBurnChecks from "../../../../lib/snoozedBurnChecks"

import {
  report,
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

// This file holds the layout, loading, and titlebar behavior tests for the Burn Checks view.
// The suite is split across files in this folder by theme. Each test renders
// the full view. CI runs this file next to the other heavy view suites, so
// one test can take five times its local run time. 15 s is the bound, not a
// target.
describe("BurnChecksView layout", { timeout: 15_000 }, () => {
  it("keeps findings and actions hidden until snoozes are ready", () => {
    const state = vi
      .spyOn(SnoozedBurnChecks, "useSnoozedBurnChecks")
      .mockReturnValue({ status: "loading", records: [] })
    const { view } = setup(target, false, aggregate, report)

    expect(screen.getByRole("region", { name: "Loading Burn checks" })).toBeVisible()
    expect(screen.queryByRole("button", { name: "Snooze" })).not.toBeInTheDocument()
    expect(screen.queryByRole("button", { name: /Old model usage/ })).not.toBeInTheDocument()

    view.unmount()
    state.mockRestore()
  })

  it("keeps anchored selection and session cards visible across window resizes", async () => {
    setWindowWidth(1400)
    const { view } = setup(target, false, aggregate, {
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
        report.categories[1]!,
      ],
    })

    const oldModel = await screen.findByRole("button", { name: /Old model usage/ })
    const overthinking = screen.getByRole("button", { name: /Model overthinking/ })
    expect(screen.getByRole("heading", { name: "Failed checks 2" })).toBeVisible()
    expect(oldModel).toHaveAttribute("aria-pressed", "true")

    fireEvent.keyDown(oldModel, { key: "ArrowDown" })
    await waitFor(() => expect(overthinking).toHaveAttribute("aria-pressed", "true"))
    expect(overthinking).toHaveFocus()
    fireEvent.keyDown(overthinking, { key: "Enter" })
    await waitFor(() =>
      expect(document.getElementById("burn-check-modelOverthinking-detail")).toHaveFocus(),
    )

    expect(await screen.findByRole("button", { name: /Update model/ })).toBeVisible()

    setWindowWidth(1000)
    const resizedOverthinking = await screen.findByRole("button", {
      name: /Model overthinking/,
    })
    expect(resizedOverthinking).toHaveAttribute("aria-pressed", "true")
    expect(resizedOverthinking).not.toHaveAttribute("aria-expanded")
    expect(
      within(document.getElementById("burn-check-modelOverthinking-detail")!).getByRole(
        "button",
        { name: /Update model/ },
      ),
    ).toBeVisible()
    const resizedOldModel = screen.getByRole("button", { name: /Old model usage/ })
    expect(resizedOldModel).toHaveAttribute("aria-pressed", "false")
    expect(resizedOldModel).not.toHaveAttribute("aria-expanded")
    expect(view.container.querySelector(".burn-checks-collection")).toHaveClass(
      "main-window-collection",
    )
    expect(view.container.querySelector(".burn-checks-detail-pane")).toHaveClass(
      "main-window-detail",
    )
  })

  it("keeps the complete collection before the detail at minimum desktop width", async () => {
    setWindowWidth(1000)
    setup(target, false, aggregate, {
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
    })

    const first = await screen.findByRole("button", { name: /Old model usage/ })
    const detail = document.getElementById("burn-check-oldModelUsage-detail")!
    const action = await within(detail).findByRole("button", { name: "Copy fix prompt" })
    const second = screen.getByRole("button", { name: /Model overthinking/ })

    expect(first.compareDocumentPosition(second) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0)
    expect(second.compareDocumentPosition(action) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(
      0,
    )
    expect(first).toHaveAttribute("aria-pressed", "true")
    expect(first).not.toHaveAttribute("aria-expanded")
  })

  it("keeps group counts and the period without an assessment info control", async () => {
    setup(target, false, aggregate, report)
    expect(await screen.findByRole("heading", { name: "Failed checks 1" })).toBeVisible()
    expect(screen.getByRole("button", { name: "Passed checks 1" })).toBeVisible()
    expect(screen.getByText("30 days")).toBeVisible()
    expect(screen.queryByRole("button", { name: "Assessment details" })).not.toBeInTheDocument()
    expect(screen.queryByText("Coverage details")).not.toBeInTheDocument()
  })

  it("keeps processing count out of the collection header", async () => {
    setup(target, false, aggregate, { ...report, pendingEvidence: 2 })

    expect(screen.queryByText("2 sessions processing.")).not.toBeInTheDocument()
  })

  it("leaves shared titlebar ownership to the layout while loading and after load", async () => {
    const userAgent = vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Macintosh")
    try {
      const pending = deferred<ChecksReportPayload>()
      const { view } = setup(target, false, aggregate, pending.promise)
      expect(view.container.querySelector("[data-tauri-drag-region]")).toBeNull()
      await act(async () => pending.resolve(report))
      await screen.findByRole("button", { name: /Old model usage.*8% estimated burn/ })
      expect(view.container.querySelectorAll("[data-tauri-drag-region]")).toHaveLength(0)
    } finally {
      userAgent.mockRestore()
    }
  })

  it("uses native title bars without adding a drag strip on Windows", () => {
    const userAgent = vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Windows")
    try {
      const { view } = setup()
      expect(view.container.querySelector("[data-tauri-drag-region]")).toBeNull()
    } finally {
      userAgent.mockRestore()
    }
  })

  it("leaves the macOS empty error state below the shared titlebar", async () => {
    const userAgent = vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Macintosh")
    try {
      const { view } = setup(target, false, aggregate, Promise.reject(new Error("Unavailable")))
      expect(await screen.findByRole("alert")).toHaveTextContent("Burn checks are unavailable.")
      expect(view.container.querySelector("[data-tauri-drag-region]")).toBeNull()
    } finally {
      userAgent.mockRestore()
    }
  })

  it("uses one busy region and one loading announcement", () => {
    const pending = deferred<ChecksReportPayload>()
    const { view } = setup(target, false, aggregate, pending.promise)

    const loading = screen.getByRole("region", { name: "Loading Burn checks" })
    expect(loading).toHaveAttribute("aria-busy", "true")
    expect(within(loading).getAllByRole("status")).toHaveLength(1)
    expect(view.container.querySelectorAll('[aria-busy="true"]')).toHaveLength(1)
    expect(loading).toHaveTextContent("Loading Burn checks.")
  })

  it("uses one compact skeleton for an expanded check while its details load", async () => {
    const pending = deferred<BurnCheckTargetPayload | null>()
    setup(pending.promise)

    const loading = await screen.findByRole("region", { name: "Loading finding details" })
    expect(loading).toHaveAttribute("aria-busy", "true")
    expect(within(loading).getAllByRole("status")).toHaveLength(1)
    expect(loading.querySelectorAll("[data-placeholder]")).toHaveLength(4)
    expect(loading).not.toHaveTextContent("Loading finding details…")

    await act(async () => pending.resolve(target))
    expect(await screen.findByRole("button", { name: "Copy fix prompt" })).toBeVisible()
  })

  it("shows a target load error and retries the expanded check", async () => {
    const unavailable = Promise.reject(new Error("Unavailable"))
    const { adapter } = setup(unavailable)

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Could not load this check's details.",
    )
    vi.mocked(adapter.getTargets).mockResolvedValueOnce({
      targets: [target],
      samples: [],
      truncated: false,
    })
    fireEvent.click(screen.getByRole("button", { name: "Retry" }))

    expect(await screen.findByRole("button", { name: "Copy fix prompt" })).toBeVisible()
  })
})
