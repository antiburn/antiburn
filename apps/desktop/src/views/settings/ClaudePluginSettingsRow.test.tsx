import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import type { ClaudePluginStatusPayload } from "../../lib/claudePluginOffer"
import { resetClaudePluginReports } from "../../lib/claudePluginStore"
import type { LiveUsageSummaryPayload } from "../../lib/providerUsageIpc"
import { ClaudePluginSettingsRow } from "./ClaudePluginSettingsRow"

const getClaudePluginStatus = vi.hoisted(() =>
  vi.fn<() => Promise<ClaudePluginStatusPayload | null>>(),
)
const getLiveUsage = vi.hoisted(() => vi.fn<() => Promise<LiveUsageSummaryPayload>>())
const noteInteraction = vi.hoisted(() => vi.fn())
const openClaudePluginPage = vi.hoisted(() => vi.fn(async () => {}))
vi.mock("../../lib/ipc", () => ({
  getClaudePluginStatus,
  getLiveUsage,
  noteInteraction,
  openClaudePluginPage,
}))

const CLAUDE_HERE: LiveUsageSummaryPayload = {
  providers: [],
  errors: [],
  meters: [
    { provider: "anthropic", displayName: "Claude", shown: true, detection: "signedIn" },
  ],
  generatedAt: "",
}

beforeEach(() => {
  localStorage.clear()
  resetClaudePluginReports()
  getClaudePluginStatus.mockReset()
  getLiveUsage.mockReset().mockResolvedValue(CLAUDE_HERE)
  noteInteraction.mockReset()
  openClaudePluginPage.mockClear()
})

describe("ClaudePluginSettingsRow", () => {
  it("is the searchable row and opens the install detail on Install", async () => {
    getClaudePluginStatus.mockResolvedValue({ state: "absent", version: null, enabled: true })
    const { container } = render(<ClaudePluginSettingsRow />)
    const row = container.querySelector('[data-settings-control="claudePlugin"]')
    expect(row).not.toBeNull()
    expect(row).toHaveTextContent("Claude Code plugin")
    await waitFor(() => expect(row).toHaveTextContent("Not installed."))
    expect(noteInteraction).toHaveBeenCalledWith({
      kind: "claudePluginOffer",
      action: "shown",
      placement: "usage_settings",
    })

    fireEvent.click(screen.getByRole("button", { name: "Install" }))
    expect(screen.getByRole("button", { name: "Copy session command" })).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Hide" }))
    expect(screen.queryByRole("button", { name: "Copy session command" })).toBeNull()
  })

  it("shows the version and the plugin page once installed", async () => {
    getClaudePluginStatus.mockResolvedValue({
      state: "installed",
      version: "0.2.1",
      enabled: false,
    })
    render(<ClaudePluginSettingsRow />)
    await screen.findByText(/Installed 0\.2\.1\. Turned off in Claude Code's settings\./)
    expect(screen.queryByRole("button", { name: "Install" })).toBeNull()
    fireEvent.click(screen.getByRole("button", { name: "Plugin page" }))
    expect(openClaudePluginPage).toHaveBeenCalledTimes(1)
    expect(noteInteraction).toHaveBeenCalledWith({
      kind: "claudePluginOffer",
      action: "open_plugin_page",
      placement: "usage_settings",
    })
    expect(noteInteraction).not.toHaveBeenCalledWith(
      expect.objectContaining({ action: "installed_detected" }),
    )
  })

  it("says so when the plugin list could not be read, and still offers the steps", async () => {
    getClaudePluginStatus.mockResolvedValue({ state: "unknown", version: null, enabled: true })
    render(<ClaudePluginSettingsRow />)
    await screen.findByText(/could not read Claude Code's plugin list/)
    expect(screen.getByRole("button", { name: "Install" })).toBeInTheDocument()
    expect(noteInteraction).not.toHaveBeenCalled()
  })
})
