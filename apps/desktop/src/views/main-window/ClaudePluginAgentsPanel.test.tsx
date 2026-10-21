import { render, screen, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import type { ClaudePluginStatusPayload } from "../../lib/claudePluginOffer"
import { resetClaudePluginReports } from "../../lib/claudePluginStore"
import type { LiveUsageSummaryPayload } from "../../lib/providerUsageIpc"
import { ClaudePluginAgentsPanel } from "./ClaudePluginAgentsPanel"
import { claudeSessionsFrom } from "./mainClaudePluginStore"
import type { OverviewProgress } from "./overview/overviewProgressStore"

const getClaudePluginStatus = vi.hoisted(() =>
  vi.fn<() => Promise<ClaudePluginStatusPayload | null>>(),
)
const getLiveUsage = vi.hoisted(() => vi.fn<() => Promise<LiveUsageSummaryPayload>>())
const noteInteraction = vi.hoisted(() => vi.fn())
const progress = vi.hoisted(() => ({ claudeSessions: 0 }))
vi.mock("../../lib/ipc", () => ({
  getClaudePluginStatus,
  getLiveUsage,
  noteInteraction,
  openClaudePluginPage: vi.fn(async () => {}),
}))
vi.mock("./overview/overviewProgressStore", () => ({
  overviewProgress: () => ({
    agents: {
      rows: [
        { agent: "codex", label: "Codex", sessions: 2, done: true },
        {
          agent: "claude-code",
          label: "Claude Code",
          sessions: progress.claudeSessions,
          done: true,
        },
      ],
    },
  }),
  subscribeOverviewProgress: () => () => undefined,
}))

const EMPTY: LiveUsageSummaryPayload = {
  providers: [],
  errors: [],
  meters: [],
  generatedAt: "",
}

beforeEach(() => {
  localStorage.clear()
  resetClaudePluginReports()
  progress.claudeSessions = 5
  getClaudePluginStatus.mockReset()
  getLiveUsage.mockReset().mockResolvedValue(EMPTY)
  noteInteraction.mockReset()
})

describe("ClaudePluginAgentsPanel", () => {
  it("shows the install detail when Claude Code sessions exist and the plugin is absent", async () => {
    getClaudePluginStatus.mockResolvedValue({ state: "absent", version: null, enabled: true })
    render(<ClaudePluginAgentsPanel active />)
    const panel = await screen.findByRole("region", { name: "Claude Code Plugin" })
    expect(panel).toHaveTextContent("Usage, resets and pace above your prompt.")
    expect(screen.getByRole("button", { name: "Copy session command" })).toBeInTheDocument()
    expect(noteInteraction).toHaveBeenCalledWith({
      kind: "claudePluginOffer",
      action: "shown",
      placement: "agents",
    })
  })

  it("shows the version once the plugin is found, with no install steps", async () => {
    getClaudePluginStatus.mockResolvedValue({
      state: "installed",
      version: "0.2.1",
      enabled: true,
    })
    render(<ClaudePluginAgentsPanel active />)
    const panel = await screen.findByRole("region", { name: "Claude Code Plugin" })
    expect(panel).toHaveTextContent("Installed 0.2.1.")
    expect(screen.getByRole("button", { name: "Plugin page" })).toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "Copy session command" })).toBeNull()
  })

  it("ignores a sidebar dismissal: it is status, not a nudge", async () => {
    localStorage.setItem(
      "antiburn.claudePluginOffer.v1",
      JSON.stringify({ dismissedAt: "2026-10-21T00:00:00.000Z" }),
    )
    getClaudePluginStatus.mockResolvedValue({ state: "absent", version: null, enabled: true })
    render(<ClaudePluginAgentsPanel active />)
    await screen.findByRole("region", { name: "Claude Code Plugin" })
  })

  it("renders nothing without Claude Code, or without a readable status", async () => {
    progress.claudeSessions = 0
    getClaudePluginStatus.mockResolvedValue({ state: "absent", version: null, enabled: true })
    const { container, unmount } = render(<ClaudePluginAgentsPanel active />)
    await waitFor(() => expect(getClaudePluginStatus).toHaveBeenCalled())
    expect(container).toBeEmptyDOMElement()
    unmount()

    progress.claudeSessions = 5
    getClaudePluginStatus.mockResolvedValue({ state: "unknown", version: null, enabled: true })
    const second = render(<ClaudePluginAgentsPanel active />)
    await waitFor(() => expect(getClaudePluginStatus).toHaveBeenCalledTimes(2))
    expect(second.container).toBeEmptyDOMElement()
    expect(noteInteraction).not.toHaveBeenCalled()
  })
})

describe("claudeSessionsFrom", () => {
  it("reads the Claude Code row and defaults to zero", () => {
    const rows = [
      { agent: "codex", label: "Codex", sessions: 2, done: true },
      { agent: "claude-code", label: "Claude Code", sessions: 7, done: true },
    ]
    expect(claudeSessionsFrom({ agents: { rows } } as unknown as OverviewProgress)).toBe(7)
    expect(
      claudeSessionsFrom({ agents: { rows: rows.slice(0, 1) } } as unknown as OverviewProgress),
    ).toBe(0)
  })
})
