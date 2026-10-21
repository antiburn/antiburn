import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import { writeClipboardText } from "../../lib/clipboard"
import {
  ClaudePluginInstallDetail,
  ClaudePluginSidebarCard,
  claudePluginInstalledLine,
} from "./ClaudePluginOffer"

const noteInteraction = vi.hoisted(() => vi.fn())
const openClaudePluginPage = vi.hoisted(() => vi.fn(async () => {}))
vi.mock("../../lib/ipc", () => ({ noteInteraction, openClaudePluginPage }))
vi.mock("../../lib/clipboard", () => ({ writeClipboardText: vi.fn(async () => {}) }))

beforeEach(() => {
  noteInteraction.mockReset()
  openClaudePluginPage.mockClear()
  vi.mocked(writeClipboardText).mockReset().mockResolvedValue(undefined)
})

describe("ClaudePluginSidebarCard", () => {
  it("shows the picked card: title, one line, Install, and a dismiss cross", () => {
    const onInstall = vi.fn()
    const onDismiss = vi.fn()
    render(<ClaudePluginSidebarCard onInstall={onInstall} onDismiss={onDismiss} />)
    const card = screen.getByRole("status", { name: "Claude Code Plugin" })
    expect(card).toHaveTextContent(
      "Usage, resets and pace above your prompt. Free plugin, no account.",
    )
    expect(card).not.toHaveTextContent("Takes 10 seconds")
    fireEvent.click(screen.getByRole("button", { name: "Install" }))
    expect(onInstall).toHaveBeenCalledTimes(1)
    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }))
    expect(onDismiss).toHaveBeenCalledTimes(1)
  })
})

describe("ClaudePluginInstallDetail", () => {
  it("copies the session command and reports the copy with its placement", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    render(<ClaudePluginInstallDetail placement="agents" />)
    expect(
      screen.getByText("/plugin install antiburn --marketplace antiburn/antiburn"),
    ).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Copy session command" }))
    await waitFor(() =>
      expect(writeClipboardText).toHaveBeenCalledWith(
        "/plugin install antiburn --marketplace antiburn/antiburn",
      ),
    )
    expect(noteInteraction).toHaveBeenCalledWith({
      kind: "claudePluginOffer",
      action: "copy_session_command",
      placement: "agents",
    })
    expect(screen.getByRole("button", { name: "Copied" })).toBeInTheDocument()
    act(() => {
      vi.advanceTimersByTime(2_100)
    })
    expect(screen.getByRole("button", { name: "Copy session command" })).toBeInTheDocument()
    vi.useRealTimers()
  })

  it("copies both shell lines together", async () => {
    render(<ClaudePluginInstallDetail placement="usage_settings" />)
    fireEvent.click(screen.getByRole("button", { name: "Copy shell commands" }))
    await waitFor(() =>
      expect(writeClipboardText).toHaveBeenCalledWith(
        "claude plugin marketplace add https://github.com/antiburn/antiburn.git\nclaude plugin install antiburn@antiburn --scope user",
      ),
    )
    expect(noteInteraction).toHaveBeenCalledWith({
      kind: "claudePluginOffer",
      action: "copy_shell_command",
      placement: "usage_settings",
    })
  })

  it("does not claim a copy that failed", async () => {
    vi.mocked(writeClipboardText).mockRejectedValue(new Error("no clipboard"))
    render(<ClaudePluginInstallDetail placement="agents" />)
    fireEvent.click(screen.getByRole("button", { name: "Copy session command" }))
    await waitFor(() => expect(writeClipboardText).toHaveBeenCalled())
    expect(noteInteraction).not.toHaveBeenCalled()
    expect(screen.queryByRole("button", { name: "Copied" })).toBeNull()
  })

  it("opens the plugin page through the shell and reports it", () => {
    render(<ClaudePluginInstallDetail placement="agents" />)
    expect(screen.getByText(/No model calls, no antiburn account\./)).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Plugin page" }))
    expect(openClaudePluginPage).toHaveBeenCalledTimes(1)
    expect(noteInteraction).toHaveBeenCalledWith({
      kind: "claudePluginOffer",
      action: "open_plugin_page",
      placement: "agents",
    })
  })
})

describe("claudePluginInstalledLine", () => {
  it("names the version and a disabled state", () => {
    expect(
      claudePluginInstalledLine({ state: "installed", version: "0.2.1", enabled: true }),
    ).toBe("Installed 0.2.1.")
    expect(
      claudePluginInstalledLine({ state: "installed", version: null, enabled: false }),
    ).toBe("Installed. Turned off in Claude Code's settings.")
  })
})
