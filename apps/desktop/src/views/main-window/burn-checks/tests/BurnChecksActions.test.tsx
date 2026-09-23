import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type * as ClipboardModule from "../../../../lib/clipboard"
import type * as InsightsIpcModule from "../../../../lib/insightsIpc"
import type * as IpcModule from "../../../../lib/ipc"
import { BurnCheckDetail, CheckPromptAction } from "../BurnCheckDetail"

import {
  target,
  deferred,
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

// This file holds the actions and review tests for the Burn Checks view.
// The suite is split across files in this folder by theme. Each test renders
// the full view. CI runs this file next to the other heavy view suites, so
// one test can take five times its local run time. 15 s is the bound, not a
// target.
describe("BurnChecksView actions", { timeout: 15_000 }, () => {
  it("uses backend prepare, apply, and prompt commands without duplicate actions", async () => {
    const { adapter, session } = setup()
    const fix = await screen.findByRole("button", { name: "Fix" })
    vi.useFakeTimers()
    fireEvent.click(fix)
    fireEvent.click(fix)
    expect(commands.prepare).toHaveBeenCalledOnce()
    await act(async () => undefined)
    const dialog = screen.getByRole("dialog", { name: "Review change" })
    expect(dialog).toHaveTextContent("Claude Code · Model · Global configuration")
    expect(dialog).toHaveTextContent("~/.claude/settings.json · model")
    expect(dialog).toHaveTextContent("claude-opus-4-6 → claude-sonnet-5")
    expect(dialog).toHaveTextContent("Responses can change")
    expect(dialog).toHaveTextContent("previous content in a sibling .bak file")
    fireEvent.click(within(dialog).getByRole("button", { name: "Apply change" }))
    await act(async () => undefined)
    expect(screen.getByRole("button", { name: "Change applied" })).toBeDisabled()
    expect(commands.apply).toHaveBeenCalledWith("prepared-1")
    expect(fix).toHaveFocus()

    fireEvent.click(screen.getByRole("button", { name: "Copy fix prompt" }))
    await act(async () => undefined)
    expect(commands.writeClipboardText).toHaveBeenCalledWith("Batch backend prompt")
    expect(commands.copyBatch).toHaveBeenCalledWith(["action-fresh"])
    expect(commands.copy).not.toHaveBeenCalled()
    const copied = screen.getByRole("button", { name: "Copied" })
    const applied = screen.getByRole("button", { name: "Change applied" })
    expect(copied).toBeDisabled()
    expect(copied).not.toHaveClass("text-token-in")
    expect(applied).not.toHaveClass("text-token-in")
    expect(copied.querySelector(".lucide-check")).toHaveClass("text-token-in")
    expect(applied.querySelector(".lucide-check")).toHaveClass("text-token-in")
    expect(commands.noteInteraction.mock.calls).toEqual(
      expect.arrayContaining([
        [{ kind: "burnCheckAutoFixReviewed", outcome: "ready" }],
        [{ kind: "burnCheckAutoFixConfirmed" }],
        [
          {
            kind: "burnCheckAutoFixCompleted",
            outcome: "applied_awaiting_verification",
          },
        ],
        [{ kind: "burnCheckPromptPrepared", outcome: "ready" }],
        [{ kind: "burnCheckPromptCopied" }],
      ]),
    )

    vi.mocked(adapter.getTargets).mockResolvedValueOnce({
      targets: [{ ...target, actionId: "action-new" }],
      samples: [],
      truncated: false,
    })
    const refreshCount = vi.mocked(adapter.getTargets).mock.calls.length
    await act(async () => session.loadTargets("unusedMcpServers", true))
    expect(vi.mocked(adapter.getTargets).mock.calls.length).toBeGreaterThan(refreshCount)
    expect(session.getSnapshot().targets.unusedMcpServers?.data?.targets[0]?.actionId).toBe(
      "action-new",
    )
    expect(screen.getByRole("button", { name: "Copy fix prompt" })).toBeEnabled()
    expect(screen.getByRole("button", { name: "Change applied" })).toBeDisabled()
    expect(screen.queryByRole("button", { name: "Fix" })).not.toBeInTheDocument()

    vi.mocked(adapter.getTargets).mockResolvedValueOnce({
      targets: [
        {
          ...target,
          actionId: "action-next-attempt",
          watch: {
            watchId: "watch-2",
            origin: "action",
            lifecycle: "watching",
            verification: { status: "watching" },
            savings: { status: "pending" },
          },
        },
      ],
      samples: [],
      truncated: false,
    })
    await act(async () => session.loadTargets("unusedMcpServers", true))
    expect(session.getSnapshot().targets.unusedMcpServers?.data?.targets[0]?.actionId).toBe(
      "action-next-attempt",
    )
    expect(screen.getByRole("button", { name: "Copy fix prompt" })).toBeEnabled()
    expect(screen.getByRole("button", { name: "Fix" })).toBeEnabled()
  }, 10_000)

  it("reviews and applies all selected automatic fixes", async () => {
    const targets = Array.from({ length: 3 }, (_, index) => ({
      ...target,
      findingId: `finding-${index}`,
      actionId: `action-${index}`,
      display: {
        ...target.display,
        resourceIdentity: `model-${index}`,
        currentValue: `model-${index}`,
      },
    }))
    render(
      <BurnCheckDetail
        detector="oldModelUsage"
        targets={targets}
        samples={[]}
        failedSessionCount={3}
        refresh={vi.fn()}
      />,
    )

    const fix = screen.getByRole("button", { name: "Fix" })
    const prompt = screen.getByRole("button", { name: "Copy fix prompt" })
    expect(fix.parentElement).not.toHaveClass("mt-3")
    expect(fix.parentElement?.parentElement).toBe(prompt.parentElement?.parentElement)
    fireEvent.click(fix)
    let dialog = await screen.findByRole("dialog", { name: "Choose changes" })
    expect(within(dialog).getByRole("button", { name: "Review 0 changes" })).toBeDisabled()
    fireEvent.click(within(dialog).getByRole("button", { name: "Select all" }))
    fireEvent.click(within(dialog).getByRole("button", { name: "Review 3 changes" }))

    dialog = await screen.findByRole("dialog", { name: "Review 3 changes" })
    expect(within(dialog).getByText("model-0")).toBeVisible()
    expect(within(dialog).getByText("model-1")).toBeVisible()
    expect(within(dialog).getByText("model-2")).toBeVisible()
    fireEvent.click(within(dialog).getByRole("button", { name: "Apply 3 changes" }))

    dialog = await screen.findByRole("dialog", { name: "Changes applied" })
    expect(within(dialog).getByRole("status")).toHaveTextContent("3 changes applied")
    expect(commands.apply).toHaveBeenCalledTimes(3)
  })

  it("keeps long config values readable in the batch review", async () => {
    const currentValue = "copy-value-with-a-long-config-selector-".repeat(8)
    const proposedValue = "claude-sonnet-5-replacement-value-".repeat(8)
    commands.prepare.mockResolvedValue({
      outcome: "reviewReady",
      review: {
        preparedOperationId: "prepared-long-value",
        expiresAtEpoch: 100,
        agent: "claude-code",
        scope: "project",
        setting: "model",
        configFile: "~/Sites/pickleheads/.claude/settings.local.json",
        selectorLabel: "copy-cluade-local",
        currentValue,
        proposedValue,
        effect: "modelSelection",
        sideEffect: "modelBehaviorMayChange",
      },
    })
    const targets = [0, 1].map((index) => ({
      ...target,
      findingId: `finding-long-${index}`,
      actionId: `action-long-${index}`,
      display: { ...target.display, resourceIdentity: `model-long-${index}` },
    }))
    render(
      <BurnCheckDetail
        detector="oldModelUsage"
        targets={targets}
        samples={[]}
        failedSessionCount={2}
        refresh={vi.fn()}
      />,
    )

    fireEvent.click(screen.getByRole("button", { name: "Fix" }))
    let dialog = await screen.findByRole("dialog", { name: "Choose changes" })
    fireEvent.click(within(dialog).getByRole("button", { name: "Select all" }))
    fireEvent.click(within(dialog).getByRole("button", { name: "Review 2 changes" }))

    dialog = await screen.findByRole("dialog", { name: "Review 2 changes" })
    const values = within(dialog).getAllByText(`${currentValue} → ${proposedValue}`)
    expect(values).toHaveLength(2)
    for (const value of values) {
      expect(value).toHaveClass("block", "max-w-full", "break-all")
      expect(value).toBeVisible()
    }
  })

  it("applies only the selected automatic fixes", async () => {
    const targets = Array.from({ length: 2 }, (_, index) => ({
      ...target,
      findingId: `finding-${index}`,
      actionId: `action-${index}`,
      display: {
        ...target.display,
        resourceIdentity: `model-${index}`,
        currentValue: `model-${index}`,
      },
    }))
    render(
      <BurnCheckDetail
        detector="oldModelUsage"
        targets={targets}
        samples={[]}
        failedSessionCount={2}
        refresh={vi.fn()}
      />,
    )

    fireEvent.click(screen.getByRole("button", { name: "Fix" }))
    let dialog = await screen.findByRole("dialog", { name: "Choose changes" })
    fireEvent.click(within(dialog).getByRole("checkbox", { name: /model-1/ }))
    fireEvent.click(within(dialog).getByRole("button", { name: "Review 1 change" }))
    dialog = await screen.findByRole("dialog", { name: "Review 1 change" })
    expect(within(dialog).getByText("model-1")).toBeVisible()
    expect(within(dialog).queryByText("model-0")).not.toBeInTheDocument()
    fireEvent.click(within(dialog).getByRole("button", { name: "Apply 1 change" }))

    await screen.findByRole("dialog", { name: "Changes applied" })
    expect(commands.prepare).toHaveBeenCalledTimes(2)
    expect(commands.prepare).toHaveBeenNthCalledWith(1, "action-1")
    expect(commands.prepare).toHaveBeenNthCalledWith(2, "action-1")
    expect(commands.apply).toHaveBeenCalledOnce()
  })

  it("names resource targets in the automatic fix chooser", async () => {
    const targets = ["ReportFindings", "Workflow"].map((name, index) => ({
      ...target,
      findingId: `finding-${index}`,
      actionId: `action-${index}`,
      display: {
        ...target.display,
        resourceKind: "builtInTool" as const,
        resourceIdentity: name,
        currentValue: null,
        replacementValue: null,
        scopeKind: "project" as const,
      },
    }))
    render(
      <BurnCheckDetail
        detector="oldModelUsage"
        targets={targets}
        samples={[]}
        failedSessionCount={2}
        refresh={vi.fn()}
      />,
    )

    fireEvent.click(screen.getByRole("button", { name: "Fix" }))
    const chooser = await screen.findByRole("dialog", { name: "Choose changes" })

    expect(
      within(chooser).getByText("Select optional built-in tools to disable."),
    ).toBeVisible()
    expect(within(chooser).getByText("Claude Code · Project configuration")).toBeVisible()
    expect(within(chooser).getByRole("checkbox", { name: "ReportFindings" })).toBeVisible()
    expect(within(chooser).getByRole("checkbox", { name: "Workflow" })).toBeVisible()
    expect(within(chooser).getAllByText(/built-in tools to disable/i)).toHaveLength(1)
  })

  it("restores whole-check actions after their brief success state", async () => {
    render(
      <BurnCheckDetail
        detector="oldModelUsage"
        targets={[target]}
        samples={[]}
        failedSessionCount={1}
        refresh={vi.fn()}
      />,
    )

    const copy = screen.getByRole("button", { name: "Copy fix prompt" })
    vi.useFakeTimers()
    fireEvent.click(copy)
    await act(async () => undefined)
    expect(screen.getByRole("button", { name: "Copied" })).toBeDisabled()
    fireEvent.click(screen.getByRole("button", { name: "Fix" }))
    await act(async () => undefined)
    const dialog = screen.getByRole("dialog", { name: "Review change" })
    fireEvent.click(within(dialog).getByRole("button", { name: "Apply change" }))
    await act(async () => undefined)
    expect(screen.getByRole("button", { name: "Change applied" })).toBeDisabled()

    await act(async () => vi.advanceTimersByTime(3_000))
    expect(screen.getByRole("button", { name: "Copy fix prompt" })).toBeEnabled()
    expect(screen.getByRole("button", { name: "Fix" })).toBeEnabled()
    expect(commands.copyBatch).toHaveBeenCalledWith(["action-fresh"])
    expect(commands.copy).not.toHaveBeenCalled()
    expect(commands.apply).toHaveBeenCalledOnce()
  })

  it("shows a busy label while the check prompt is prepared", async () => {
    const pending = deferred<Awaited<ReturnType<typeof commands.copyBatch>>>()
    commands.copyBatch.mockReturnValueOnce(pending.promise)
    render(<CheckPromptAction detector="unusedSkills" targets={[target]} refresh={vi.fn()} />)

    fireEvent.click(await screen.findByRole("button", { name: "Copy fix prompt" }))
    expect(await screen.findByRole("button", { name: "Preparing…" })).toBeDisabled()

    pending.resolve({ outcome: "promptReady", prompt: "Batch backend prompt" })
    expect(await screen.findByRole("button", { name: "Copied" })).toBeDisabled()
    expect(commands.writeClipboardText).toHaveBeenCalledWith("Batch backend prompt")
  })

  it("retries clipboard failure without creating a second backend action", async () => {
    const prompt = "Batch backend prompt\n\nRemediation reference: ABR-shared-group"
    commands.copyBatch.mockResolvedValueOnce({
      outcome: "promptReady",
      prompt,
    })
    commands.writeClipboardText
      .mockRejectedValueOnce(new Error("Denied"))
      .mockResolvedValueOnce(undefined)
    render(<CheckPromptAction detector="oldModelUsage" targets={[target]} refresh={vi.fn()} />)
    const copy = await screen.findByRole("button", { name: "Copy fix prompt" })
    fireEvent.click(copy)
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not copy")
    fireEvent.click(copy)
    await screen.findByRole("button", { name: "Copied" })
    expect(commands.copyBatch).toHaveBeenCalledOnce()
    expect(commands.writeClipboardText).toHaveBeenCalledTimes(2)
    expect(commands.writeClipboardText).toHaveBeenNthCalledWith(1, prompt)
    expect(commands.writeClipboardText).toHaveBeenNthCalledWith(2, prompt)
    expect(screen.queryByText(/clipboard access/i)).not.toBeInTheDocument()
    expect(commands.noteInteraction.mock.calls).toEqual(
      expect.arrayContaining([
        [{ kind: "burnCheckPromptPrepared", outcome: "ready" }],
        [{ kind: "burnCheckPromptCopied" }],
      ]),
    )
    expect(
      commands.noteInteraction.mock.calls.filter(
        ([interaction]) => interaction.kind === "burnCheckPromptPrepared",
      ),
    ).toHaveLength(1)
  })

  it("records a failed Auto Fix result without claiming success", async () => {
    commands.apply.mockRejectedValueOnce(new Error("Private backend error"))
    render(
      <BurnCheckDetail
        detector="oldModelUsage"
        targets={[target]}
        samples={[]}
        failedSessionCount={1}
        refresh={vi.fn()}
      />,
    )
    fireEvent.click(screen.getByRole("button", { name: "Fix" }))
    const dialog = await screen.findByRole("dialog", { name: "Review change" })
    fireEvent.click(within(dialog).getByRole("button", { name: "Apply change" }))

    await waitFor(() =>
      expect(within(dialog).getByRole("alert")).toHaveTextContent(
        "Could not confirm the result. Check the setting before you try again.",
      ),
    )
    expect(screen.getByRole("dialog", { name: "Review change" })).toBeVisible()
    await waitFor(() =>
      expect(commands.noteInteraction).toHaveBeenCalledWith({
        kind: "burnCheckAutoFixCompleted",
        outcome: "failed",
      }),
    )
    expect(commands.noteInteraction).not.toHaveBeenCalledWith({
      kind: "burnCheckAutoFixCompleted",
      outcome: "applied_awaiting_verification",
    })
    expect(JSON.stringify(commands.noteInteraction.mock.calls)).not.toContain(
      "Private backend error",
    )
  })

  it("keeps a pending apply modal locked until the write completes", async () => {
    let resolveApply!: (value: Awaited<ReturnType<typeof commands.apply>>) => void
    commands.apply.mockReturnValueOnce(
      new Promise((resolve) => {
        resolveApply = resolve
      }),
    )
    render(
      <BurnCheckDetail
        detector="oldModelUsage"
        targets={[target]}
        samples={[]}
        failedSessionCount={1}
        refresh={vi.fn()}
      />,
    )
    fireEvent.click(screen.getByRole("button", { name: "Fix" }))
    const dialog = await screen.findByRole("dialog", { name: "Review change" })
    fireEvent.click(within(dialog).getByRole("button", { name: "Apply change" }))

    expect(within(dialog).getByRole("button", { name: "Cancel" })).toBeDisabled()
    fireEvent.keyDown(dialog, { key: "Escape" })
    fireEvent.mouseDown(dialog.parentElement!)
    expect(screen.getByRole("dialog", { name: "Review change" })).toHaveAttribute(
      "aria-busy",
      "true",
    )
    expect(commands.apply).toHaveBeenCalledOnce()

    resolveApply({ outcome: "appliedAwaitingVerification", watchId: "watch-1" })
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument())
  })

  it("traps keyboard focus in the review and restores it after Escape", async () => {
    render(
      <BurnCheckDetail
        detector="oldModelUsage"
        targets={[target]}
        samples={[]}
        failedSessionCount={1}
        refresh={vi.fn()}
      />,
    )
    const fix = screen.getByRole("button", { name: "Fix" })
    fireEvent.click(fix)
    const dialog = await screen.findByRole("dialog", { name: "Review change" })
    const cancel = within(dialog).getByRole("button", { name: "Cancel" })
    const apply = within(dialog).getByRole("button", { name: "Apply change" })

    expect(dialog).toHaveAttribute("aria-modal", "true")
    expect(cancel).toHaveFocus()
    apply.focus()
    fireEvent.keyDown(dialog, { key: "Tab" })
    expect(cancel).toHaveFocus()
    fireEvent.keyDown(dialog, { key: "Escape" })
    await waitFor(() => expect(fix).toHaveFocus())
  })

  it("keeps review semantics usable with reduced motion, narrow width, and both themes", async () => {
    for (const theme of ["light", "dark"]) {
      document.documentElement.dataset.theme = theme
      const view = render(
        <BurnCheckDetail
          detector="oldModelUsage"
          targets={[target]}
          samples={[]}
          failedSessionCount={1}
          refresh={vi.fn()}
        />,
      )
      fireEvent.click(screen.getByRole("button", { name: "Fix" }))
      const dialog = await screen.findByRole("dialog", { name: "Review change" })

      expect(dialog).toHaveClass("bg-surface-card", "text-label")
      expect(dialog.querySelector("dl")).toBeNull()
      expect(dialog).toHaveTextContent("~/.claude/settings.json · model")
      expect(within(dialog).getByRole("button", { name: "Cancel" }).parentElement).toHaveClass(
        "flex-col-reverse",
        "sm:flex-row",
      )
      expect(dialog.querySelector("[class*='animate-']")).toBeNull()

      fireEvent.keyDown(dialog, { key: "Escape" })
      await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument())
      view.unmount()
    }
    delete document.documentElement.dataset.theme
  })
})
