import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type {
  ApplyPreparedBurnCheckOperationOutcome,
  CopyPromptFixBurnCheckOutcome,
  PrepareAutoFixBurnCheckTargetOutcome,
} from "../../../../lib/insightsIpc"
import type * as ClipboardModule from "../../../../lib/clipboard"
import type * as InsightsIpcModule from "../../../../lib/insightsIpc"
import type * as IpcModule from "../../../../lib/ipc"
import { BurnCheckDetail, CheckDetailActions, CheckPromptAction } from "../BurnCheckDetail"
import { BurnCheckTargetActions } from "../BurnCheckTargetActions"

import {
  target,
  deferred,
  recurredTarget,
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

// This file holds the action handles and fallback prompts tests for the Burn Checks view.
// The suite is split across files in this folder by theme. Each test renders
// the full view. CI runs this file next to the other heavy view suites, so
// one test can take five times its local run time. 15 s is the bound, not a
// target.
describe("BurnChecksView action handles", { timeout: 15_000 }, () => {
  it("keeps the click-again message after a stale id refreshes the list", async () => {
    commands.copyBatch.mockResolvedValueOnce({ outcome: "unavailable" })
    const refresh = vi.fn()
    const { rerender } = render(
      <CheckPromptAction detector="unusedSkills" targets={[target]} refresh={refresh} />,
    )

    fireEvent.click(await screen.findByRole("button", { name: "Copy fix prompt" }))
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "The list was refreshed. Click again to copy.",
    )
    expect(refresh).toHaveBeenCalledOnce()
    expect(commands.writeClipboardText).not.toHaveBeenCalled()

    // The refresh lists the check again with new ids. The message must survive that.
    rerender(
      <CheckPromptAction
        detector="unusedSkills"
        targets={[{ ...target, actionId: "action-new" }]}
        refresh={refresh}
      />,
    )
    expect(screen.getByRole("alert")).toHaveTextContent(
      "The list was refreshed. Click again to copy.",
    )
    const copy = screen.getByRole("button", { name: "Copy fix prompt" })
    expect(copy).toBeEnabled()

    fireEvent.click(copy)
    expect(await screen.findByRole("button", { name: "Copied" })).toBeEnabled()
    expect(commands.copyBatch).toHaveBeenLastCalledWith(["action-new"])
    expect(commands.writeClipboardText).toHaveBeenCalledWith("Batch backend prompt")
    expect(screen.queryByRole("alert")).not.toBeInTheDocument()
  })

  it("clears an error message when the list refreshes with new ids", async () => {
    commands.copyBatch.mockRejectedValueOnce(new Error("prepare failed"))
    const { rerender } = render(
      <CheckPromptAction detector="unusedSkills" targets={[target]} refresh={vi.fn()} />,
    )

    fireEvent.click(await screen.findByRole("button", { name: "Copy fix prompt" }))
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Could not prepare the prompt. Try again.",
    )

    // The error was about the old list. A relist with new ids drops it.
    rerender(
      <CheckPromptAction
        detector="unusedSkills"
        targets={[{ ...target, actionId: "action-new" }]}
        refresh={vi.fn()}
      />,
    )
    expect(screen.queryByRole("alert")).not.toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Copy fix prompt" })).toBeEnabled()
  })

  it("keeps an open review when the expiring action handle rotates", async () => {
    const { rerender } = render(<BurnCheckTargetActions target={target} refresh={vi.fn()} />)
    fireEvent.click(screen.getByRole("button", { name: "Fix" }))
    const dialog = await screen.findByRole("dialog", { name: "Review change" })

    // The finding is unchanged (same findingId, no watch), so the action
    // handle rotation carries the open review forward.
    rerender(
      <BurnCheckTargetActions
        target={{ ...target, actionId: "action-rotated" }}
        refresh={vi.fn()}
      />,
    )

    expect(dialog).toBeVisible()
    fireEvent.click(within(dialog).getByRole("button", { name: "Apply change" }))
    expect(commands.apply).toHaveBeenCalledWith("prepared-1")
  })

  it("accepts a deferred prepare after the action handle rotates for the same attempt", async () => {
    const pending = deferred<PrepareAutoFixBurnCheckTargetOutcome | null>()
    commands.prepare.mockReturnValueOnce(pending.promise)
    const { rerender } = render(<BurnCheckTargetActions target={target} refresh={vi.fn()} />)
    fireEvent.click(screen.getByRole("button", { name: "Fix" }))

    rerender(
      <BurnCheckTargetActions
        target={{ ...target, actionId: "action-rotated" }}
        refresh={vi.fn()}
      />,
    )
    await act(async () => {
      pending.resolve({
        outcome: "reviewReady",
        review: {
          preparedOperationId: "prepared-rotated",
          expiresAtEpoch: 100,
          agent: "claude-code",
          scope: "global",
          setting: "model",
          configFile: "~/.claude/settings.json",
          selectorLabel: "model",
          currentValue: "claude-opus-4-6",
          proposedValue: "claude-sonnet-5",
          effect: "modelSelection",
          sideEffect: "modelBehaviorMayChange",
          behaviorOverrideWarning: false,
        },
      })
    })

    expect(screen.getByRole("dialog", { name: "Review change" })).toBeVisible()
  })

  it("ignores a deferred prepare from an attempt that recurred", async () => {
    const pending = deferred<PrepareAutoFixBurnCheckTargetOutcome | null>()
    commands.prepare.mockReturnValueOnce(pending.promise)
    const { rerender } = render(<BurnCheckTargetActions target={target} refresh={vi.fn()} />)
    fireEvent.click(screen.getByRole("button", { name: "Fix" }))

    rerender(
      <BurnCheckTargetActions
        target={recurredTarget("action-after-recurrence")}
        refresh={vi.fn()}
      />,
    )
    expect(screen.getByRole("button", { name: "Fix" })).toBeEnabled()
    await act(async () => {
      pending.resolve({
        outcome: "reviewReady",
        review: {
          preparedOperationId: "prepared-stale",
          expiresAtEpoch: 100,
          agent: "claude-code",
          scope: "global",
          setting: "model",
          configFile: "~/.claude/settings.json",
          selectorLabel: "model",
          currentValue: "claude-opus-4-6",
          proposedValue: "claude-sonnet-5",
          effect: "modelSelection",
          sideEffect: "modelBehaviorMayChange",
          behaviorOverrideWarning: false,
        },
      })
    })

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
    expect(commands.noteInteraction).toHaveBeenCalledWith({
      kind: "burnCheckAutoFixReviewed",
      outcome: "ready",
    })
    expect(
      commands.noteInteraction.mock.calls.filter(
        ([interaction]) => interaction.kind === "burnCheckAutoFixReviewed",
      ),
    ).toHaveLength(1)
  })

  it("ignores a deferred prompt from an attempt that recurred", async () => {
    const pending = deferred<CopyPromptFixBurnCheckOutcome | null>()
    commands.copyBatch.mockReturnValueOnce(pending.promise)
    const { rerender } = render(
      <CheckPromptAction detector="unusedMcpServers" targets={[target]} refresh={vi.fn()} />,
    )
    fireEvent.click(screen.getByRole("button", { name: "Copy fix prompt" }))

    rerender(
      <CheckPromptAction
        detector="unusedMcpServers"
        targets={[recurredTarget("action-after-recurrence")]}
        refresh={vi.fn()}
      />,
    )
    expect(screen.getByRole("button", { name: "Copy fix prompt" })).toBeEnabled()
    await act(async () => {
      pending.resolve({
        outcome: "promptReady",
        prompt: "Stale backend prompt",
      })
    })

    expect(commands.writeClipboardText).not.toHaveBeenCalled()
    expect(screen.getByRole("button", { name: "Copy fix prompt" })).toBeEnabled()
    expect(commands.noteInteraction).toHaveBeenCalledWith({
      kind: "burnCheckPromptPrepared",
      outcome: "ready",
    })
    expect(
      commands.noteInteraction.mock.calls.filter(
        ([interaction]) => interaction.kind === "burnCheckPromptPrepared",
      ),
    ).toHaveLength(1)
  })

  it("keeps a stale apply pending, then ignores its completion after recurrence", async () => {
    const pending = deferred<ApplyPreparedBurnCheckOperationOutcome | null>()
    commands.apply.mockReturnValueOnce(pending.promise)
    const { rerender } = render(<BurnCheckTargetActions target={target} refresh={vi.fn()} />)
    fireEvent.click(screen.getByRole("button", { name: "Fix" }))
    const dialog = await screen.findByRole("dialog", { name: "Review change" })
    fireEvent.click(within(dialog).getByRole("button", { name: "Apply change" }))

    rerender(
      <BurnCheckTargetActions
        target={recurredTarget("action-after-recurrence")}
        refresh={vi.fn()}
      />,
    )
    expect(within(dialog).getByText("Applying…")).toBeVisible()
    expect(dialog).toHaveAttribute("aria-busy", "true")
    await act(async () => {
      pending.resolve({ outcome: "appliedAwaitingVerification", watchId: "watch-old" })
    })

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
    const currentFix = screen.getByRole("button", { name: "Fix" })
    expect(currentFix).toBeEnabled()
    expect(currentFix).toHaveFocus()
    expect(screen.queryByRole("button", { name: "Change applied" })).not.toBeInTheDocument()
    expect(commands.noteInteraction).toHaveBeenCalledWith({
      kind: "burnCheckAutoFixCompleted",
      outcome: "applied_awaiting_verification",
    })
    expect(
      commands.noteInteraction.mock.calls.filter(
        ([interaction]) => interaction.kind === "burnCheckAutoFixCompleted",
      ),
    ).toHaveLength(1)
  })

  it("resets copied state when the same watch records a recurrence", async () => {
    const { rerender } = render(
      <CheckPromptAction detector="unusedMcpServers" targets={[target]} refresh={vi.fn()} />,
    )
    fireEvent.click(screen.getByRole("button", { name: "Copy fix prompt" }))
    await screen.findByRole("button", { name: "Copied" })

    rerender(
      <CheckPromptAction
        detector="unusedMcpServers"
        targets={[recurredTarget("action-after-recurrence")]}
        refresh={vi.fn()}
      />,
    )

    expect(screen.getByRole("button", { name: "Copy fix prompt" })).toBeEnabled()
  })

  it("copies a fallback prompt for an empty failed check without showing Auto Fix", async () => {
    render(
      <BurnCheckDetail
        detector="unusedMcpServers"
        targets={[]}
        samples={[]}
        failedSessionCount={0}
        refresh={vi.fn()}
        contained
      />,
    )

    const emptyState = await screen.findByText("Some MCP servers were loaded but not used.")
    expect(emptyState).toBeVisible()
    expect(emptyState.closest("article")).toHaveClass(
      "rounded-control",
      "bg-surface-card/75",
      "p-4",
    )
    expect(
      screen.queryByText(/bounded view|exact target|nothing safe/i),
    ).not.toBeInTheDocument()
    expect(screen.queryByText(/Auto Fix unavailable/i)).not.toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "Fix" })).not.toBeInTheDocument()

    fireEvent.click(screen.getByRole("button", { name: "Copy fix prompt" }))

    await waitFor(() =>
      expect(commands.writeClipboardText).toHaveBeenCalledWith(
        "Inspect representative evidence.",
      ),
    )
    expect(commands.copyFallback).toHaveBeenCalledWith("unusedMcpServers")
    const copied = screen.getByRole("button", { name: "Copied" })
    expect(copied).toBeEnabled()
    expect(copied).not.toHaveClass("text-token-in")
    expect(copied.querySelector(".lucide-check")).toHaveClass("text-token-in")
  })

  it("repeats a target prompt copy while its success state is visible", async () => {
    render(<BurnCheckTargetActions target={target} refresh={vi.fn()} />)

    fireEvent.click(await screen.findByRole("button", { name: "Copy fix prompt" }))
    const copied = await screen.findByRole("button", { name: "Copied" })
    expect(copied).toBeEnabled()

    fireEvent.click(copied)
    expect(await screen.findByRole("button", { name: "Copying…" })).toBeDisabled()
    expect(await screen.findByRole("button", { name: "Copied" })).toBeEnabled()
    expect(commands.writeClipboardText).toHaveBeenCalledTimes(2)
    expect(commands.copy).toHaveBeenCalledOnce()
  })

  it("uses a fallback prompt when exact targets are unavailable", async () => {
    render(
      <CheckPromptAction
        detector="oldModelUsage"
        targets={[
          {
            ...target,
            promptFix: { status: "unavailable", reason: "unsupportedSourceFormat" },
          },
        ]}
        refresh={vi.fn()}
      />,
    )

    fireEvent.click(screen.getByRole("button", { name: "Copy fix prompt" }))

    await waitFor(() => expect(commands.copyFallback).toHaveBeenCalledWith("oldModelUsage"))
    expect(commands.writeClipboardText).toHaveBeenCalledWith("Inspect representative evidence.")
    expect(commands.copyBatch).not.toHaveBeenCalled()
  })

  it("shows a retryable fallback prompt error", async () => {
    commands.copyFallback.mockRejectedValueOnce(new Error("Private backend error"))
    render(
      <BurnCheckDetail
        detector="unusedMcpServers"
        targets={[]}
        samples={[]}
        failedSessionCount={0}
        refresh={vi.fn()}
        contained
      />,
    )

    fireEvent.click(await screen.findByRole("button", { name: "Copy fix prompt" }))

    expect(await screen.findByRole("alert")).toHaveTextContent("Could not prepare the prompt")
    expect(screen.getByRole("button", { name: "Copy fix prompt" })).toBeEnabled()
    expect(commands.writeClipboardText).not.toHaveBeenCalled()
    expect(JSON.stringify(commands.noteInteraction.mock.calls)).not.toContain(
      "Private backend error",
    )
  })

  it("ignores a fallback prompt that completes after an exact target appears", async () => {
    const pending = deferred<{ outcome: "promptReady"; prompt: string } | null>()
    commands.copyFallback.mockReturnValueOnce(pending.promise)
    const view = render(
      <CheckPromptAction
        key="fallback"
        detector="unusedMcpServers"
        targets={[]}
        refresh={vi.fn()}
      />,
    )
    fireEvent.click(await screen.findByRole("button", { name: "Copy fix prompt" }))
    view.rerender(
      <CheckPromptAction
        key="exact"
        detector="unusedMcpServers"
        targets={[target]}
        refresh={vi.fn()}
      />,
    )
    await act(async () => pending.resolve({ outcome: "promptReady", prompt: "Stale prompt" }))

    expect(commands.writeClipboardText).not.toHaveBeenCalled()
    expect(screen.queryByRole("button", { name: "Copied" })).not.toBeInTheDocument()
    expect(commands.noteInteraction).toHaveBeenCalledWith({
      kind: "burnCheckPromptPrepared",
      outcome: "ready",
    })
  })

  it("hides the implied auto-fix reason when a prompt action is available", () => {
    render(
      <CheckPromptAction
        detector="unusedMcpServers"
        targets={[
          { ...target, autoFix: { status: "unavailable", reason: "safetyCheckFailed" } },
        ]}
        refresh={vi.fn()}
      />,
    )

    expect(screen.getByRole("button", { name: "Copy fix prompt" })).toBeVisible()
    expect(screen.queryByText(/write safety check/)).not.toBeInTheDocument()
  })

  it("keeps one whole-check prompt when target actions are unavailable", async () => {
    render(
      <CheckDetailActions
        detector="unusedMcpServers"
        targets={[
          {
            ...target,
            autoFix: { status: "unavailable", reason: "safetyCheckFailed" },
            promptFix: { status: "unavailable", reason: "unsupportedSourceFormat" },
          },
        ]}
        refresh={vi.fn()}
        reportRow
      />,
    )

    const prompt = await screen.findByRole("button", { name: "Copy fix prompt" })
    await screen.findByRole("button", { name: "Snooze" })
    expect(prompt).toBeEnabled()
    expect(screen.getAllByRole("button", { name: "Copy fix prompt" })).toHaveLength(1)
    expect(screen.getAllByRole("button", { name: "Snooze" })).toHaveLength(1)
    expect(screen.queryByText(/write safety check/)).not.toBeInTheDocument()
    expect(
      screen.queryByText(/does not support a safe automatic change/),
    ).not.toBeInTheDocument()
    expect(screen.queryByText(/^Automatic fix:/)).not.toBeInTheDocument()
    expect(screen.queryByText(/^Prompt fix:/)).not.toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "Fix" })).not.toBeInTheDocument()

    fireEvent.click(prompt)
    await waitFor(() => expect(commands.copyBatch).toHaveBeenCalledWith(["action-fresh"]))
  })

  it.each([
    ["conflict", { outcome: "conflict" }],
    ["unavailable", { outcome: "unavailable", reason: "safetyCheckFailed" }],
  ] as const)(
    "keeps the %s apply outcome in the review with feedback",
    async (name, outcome) => {
      commands.apply.mockResolvedValueOnce(outcome)
      render(<BurnCheckTargetActions target={target} refresh={vi.fn()} />)
      fireEvent.click(screen.getByRole("button", { name: "Fix" }))
      const dialog = await screen.findByRole("dialog", { name: "Review change" })
      fireEvent.click(within(dialog).getByRole("button", { name: "Apply change" }))

      await waitFor(() =>
        expect(within(dialog).getByRole("button", { name: "Close" })).toBeEnabled(),
      )
      expect(within(dialog).getByRole("alert")).toHaveTextContent(
        name === "conflict"
          ? "Another change now conflicts with this operation. Close this review and check the setting."
          : "The current setting no longer passes the write safety check.",
      )
      expect(within(dialog).getByRole("button", { name: "Apply change" })).toBeDisabled()
    },
  )

  it("does not repeat target opportunities in a check-level detail", () => {
    render(
      <BurnCheckDetail
        detector="oldModelUsage"
        targets={[
          {
            ...target,
            display: {
              ...target.display,
              estimatedOpportunity: { value: 12.5, unit: "apiEquivalentUsd" },
            },
          },
        ]}
        samples={target.samples}
        failedSessionCount={1}
        refresh={vi.fn()}
      />,
    )

    expect(
      screen.getByText("Some sessions used an older model when a newer one was available."),
    ).toBeVisible()
    expect(screen.queryByText(/API-equivalent cost opportunity/)).not.toBeInTheDocument()
    expect(screen.queryByText("Estimated opportunity:")).not.toBeInTheDocument()
    expect(screen.queryByText("Estimate method:")).not.toBeInTheDocument()
  })
})
