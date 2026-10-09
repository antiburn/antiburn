import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import { DEFAULT_SETTINGS } from "../../../../lib/ipc"
import type { OverviewProgress } from "../overviewProgressStore"
import type { AgentsStepSettings as AgentsStepSettingsComponent } from "./AgentsStepSettings"

/**
 * The Agents step now holds only the Coding agents section: scanning,
 * folders, repositories, and remote hosts moved to the Sessions step and
 * keep their coverage in `SessionsStepSettings.test.tsx`.
 */

const invoke = vi.hoisted(() => vi.fn())

vi.mock("@tauri-apps/api/core", () => ({ invoke, isTauri: () => true }))
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
}))

let snapshot: OverviewProgress
let AgentsStepSettings: typeof AgentsStepSettingsComponent

// The list reads the same `overviewProgress()` rows the icon row and the
// nav reads, so the two never disagree. A fake store stands in for the
// real one, the same way `ProgressNav.test.tsx` and
// `FirstRunTakeover.test.tsx` fake it.
vi.mock("../overviewProgressStore", () => ({
  subscribeOverviewProgress: () => () => undefined,
  overviewProgress: () => snapshot,
}))

function progress(overrides: Partial<OverviewProgress["agents"]> = {}): OverviewProgress {
  return {
    mode: "steady",
    flow: "done",
    openStep: null,
    openStepControl: null,
    openStepControlRevision: 0,
    stepShown: true,
    actionPending: false,
    actionError: null,
    agents: {
      done: true,
      rows: [{ agent: "codex", label: "Codex", sessions: 3, done: true }],
      ...overrides,
    },
    sessions: {
      done: true,
      completed: 3,
      total: 3,
      displayCompleted: 3,
      displayTotal: 3,
      deferred: [],
    },
    checks: { done: true, windowSessions: 3, pendingEvidence: 0, deferredEvidence: 0 },
    categories: [],
    failingCount: 0,
    history: null,
  }
}

function mockCommands(overrides: Record<string, unknown> = {}) {
  invoke.mockImplementation((command: string) => {
    if (command in overrides) {
      const value = overrides[command]
      return value instanceof Error ? Promise.reject(value) : Promise.resolve(value)
    }
    switch (command) {
      case "get_settings":
        return Promise.resolve(DEFAULT_SETTINGS)
      case "agent_session_locations":
        return Promise.resolve([])
      default:
        return Promise.resolve(null)
    }
  })
}

beforeEach(async () => {
  // The locations list is cached at module scope for the main window's
  // whole lifetime (see `agentSessionLocationsStore`), so each test needs
  // its own fresh copy of that module.
  vi.resetModules()
  vi.clearAllMocks()
  mockCommands()
  snapshot = progress()
  ;({ AgentsStepSettings } = await import("./AgentsStepSettings"))
})

describe("AgentsStepSettings coding agents", () => {
  it("leads with the agent that has the most sessions and shows its count", async () => {
    render(<AgentsStepSettings />)

    expect(await screen.findByText("3 sessions")).toBeInTheDocument()
    const rows = screen.getAllByRole("switch")
    expect(rows[0]).toHaveAccessibleName("Show Codex sessions")
  })

  it("takes its counts from the progress store, not a stale scan-state total", () => {
    // `AgentsStepSettings` must read the same rows the icon row renders, not
    // `scan_state.sessionsSeen` — a history pass can leave that column with
    // an agent's older, wider-window count.
    snapshot = progress({
      rows: [{ agent: "cursor", label: "Cursor", sessions: 0, done: true }],
    })
    render(<AgentsStepSettings />)
    expect(screen.queryByText(/sessions$/)).not.toBeInTheDocument()
  })

  it('shows no switch and "Not found" for an agent with no sessions', () => {
    snapshot = progress({
      rows: [
        { agent: "codex", label: "Codex", sessions: 3, done: true },
        { agent: "cursor", label: "Cursor", sessions: 0, done: true },
      ],
    })
    render(<AgentsStepSettings />)

    expect(
      screen.queryByRole("switch", { name: "Show Cursor sessions" }),
    ).not.toBeInTheDocument()
    expect(screen.getAllByText("Not found")).toHaveLength(1)
    expect(screen.getByRole("switch", { name: "Show Codex sessions" })).toBeChecked()
  })

  it('shows neither a switch nor "Not found" while an agent is still searching', () => {
    snapshot = progress({
      rows: [{ agent: "cursor", label: "Cursor", sessions: 0, done: false }],
    })
    render(<AgentsStepSettings />)
    expect(
      screen.queryByRole("switch", { name: "Show Cursor sessions" }),
    ).not.toBeInTheDocument()
    expect(screen.queryByText("Not found")).not.toBeInTheDocument()
  })

  it("shows neither before any scan data exists", () => {
    snapshot = progress({ rows: [] })
    render(<AgentsStepSettings />)
    expect(screen.queryByRole("switch")).not.toBeInTheDocument()
    expect(screen.queryByText("Not found")).not.toBeInTheDocument()
  })

  it("keeps the switch for a detected agent that is switched off, and turns it on", async () => {
    let stored = { ...DEFAULT_SETTINGS, disabledAgents: ["codex"] }
    invoke.mockImplementation((command: string, args?: { settings?: typeof stored }) => {
      switch (command) {
        case "get_settings":
          return Promise.resolve(stored)
        case "set_settings":
          stored = args?.settings ?? stored
          return Promise.resolve(stored)
        case "agent_session_locations":
          return Promise.resolve([])
        default:
          return Promise.resolve(null)
      }
    })
    render(<AgentsStepSettings />)

    const toggle = await screen.findByRole("switch", { name: "Show Codex sessions" })
    await waitFor(() => expect(toggle).not.toBeChecked())
    fireEvent.click(toggle)
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("set_settings", {
        settings: expect.objectContaining({ disabledAgents: [] }),
      }),
    )
  })

  it("switches an agent off and saves the preference", async () => {
    let stored = { ...DEFAULT_SETTINGS }
    invoke.mockImplementation((command: string, args?: { settings?: typeof stored }) => {
      switch (command) {
        case "get_settings":
          return Promise.resolve(stored)
        case "set_settings":
          stored = args?.settings ?? stored
          return Promise.resolve(stored)
        case "agent_session_locations":
          return Promise.resolve([])
        default:
          return Promise.resolve(null)
      }
    })
    render(<AgentsStepSettings />)

    const toggle = await screen.findByRole("switch", { name: "Show Codex sessions" })
    expect(toggle).toBeChecked()

    fireEvent.click(toggle)
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("set_settings", {
        settings: expect.objectContaining({ disabledAgents: ["codex"] }),
      }),
    )
    // A switched-off agent keeps its sessions indexed; only the session list
    // and reports leave it out.
    expect(screen.getByText("Codex")).toBeInTheDocument()
  })

  it("names what each agent has on this computer and leaves the rest blank", async () => {
    snapshot = progress({
      rows: [
        { agent: "claude-code", label: "Claude Code", sessions: 41, done: true },
        { agent: "codex", label: "Codex", sessions: 87, done: true },
        { agent: "cursor", label: "Cursor", sessions: 12, done: true },
      ],
    })
    mockCommands({
      get_live_usage: {
        providers: [],
        errors: [],
        generatedAt: "",
        meters: [
          { provider: "openai", displayName: "Codex", shown: true, detection: "signedIn" },
          {
            provider: "anthropic",
            displayName: "Claude",
            shown: true,
            detection: "notInstalled",
            desktopAppLabel: "Claude Desktop",
          },
        ],
      },
    })
    render(<AgentsStepSettings />)

    expect(
      await screen.findByText("Claude Desktop · Limits need Claude Code signed in"),
    ).toBeInTheDocument()
    expect(screen.getByText("41 sessions")).toBeInTheDocument()
    expect(screen.getByText("Signed in")).toBeInTheDocument()
    expect(screen.getByText("87 sessions")).toBeInTheDocument()
    expect(screen.getByRole("switch", { name: "Show Claude sessions" })).toBeInTheDocument()
    // Nothing found for Devin: its row says nothing beyond its name.
    expect(screen.queryByText("No sessions yet")).not.toBeInTheDocument()
    // Opening the list asks for one user-initiated check.
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("refresh_live_usage", expect.anything()),
    )
  })

  it("shows the same three login states as the meter", async () => {
    snapshot = progress({
      rows: [
        { agent: "claude-code", label: "Claude Code", sessions: 2, done: true },
        { agent: "codex", label: "Codex", sessions: 3, done: true },
      ],
    })
    mockCommands({
      get_live_usage: {
        providers: [],
        errors: [],
        generatedAt: "",
        meters: [
          {
            provider: "anthropic",
            displayName: "Claude",
            shown: true,
            detection: "signInRequired",
          },
          {
            provider: "openai",
            displayName: "Codex",
            shown: true,
            detection: "installedNotSignedIn",
          },
        ],
      },
    })
    render(<AgentsStepSettings />)

    expect(await screen.findByText("Need to sign in again")).toBeInTheDocument()
    expect(screen.getByText("Not signed in")).toBeInTheDocument()
  })

  it("ignores a login that only Pi holds", async () => {
    snapshot = progress({
      rows: [{ agent: "codex", label: "Codex", sessions: 0, done: true }],
    })
    mockCommands({
      get_live_usage: {
        providers: [],
        errors: [],
        generatedAt: "",
        meters: [
          {
            provider: "openai",
            displayName: "Codex",
            shown: true,
            detection: "signedIn",
            carrierLabel: "Pi",
          },
        ],
      },
    })
    render(<AgentsStepSettings />)
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("get_live_usage", expect.anything()),
    )
    // Pi's login is Pi's, so Codex is still not found.
    expect(await screen.findByText("Not found")).toBeInTheDocument()
    expect(screen.queryByText("Signed in")).not.toBeInTheDocument()
  })

  it("names Claude Desktop instead of Not found when Claude has no sessions yet", async () => {
    snapshot = progress({
      rows: [{ agent: "claude-code", label: "Claude Code", sessions: 0, done: true }],
    })
    mockCommands({
      get_live_usage: {
        providers: [],
        errors: [],
        generatedAt: "",
        meters: [
          {
            provider: "anthropic",
            displayName: "Claude",
            shown: true,
            detection: "notInstalled",
            desktopAppLabel: "Claude Desktop",
          },
        ],
      },
    })
    render(<AgentsStepSettings />)

    expect(await screen.findByText("No sessions yet")).toBeInTheDocument()
    expect(
      screen.getByText("Claude Desktop · Limits need Claude Code signed in"),
    ).toBeInTheDocument()
    expect(screen.queryByText("Not found")).not.toBeInTheDocument()
  })
})

describe("AgentsStepSettings session locations", () => {
  it("shows how many locations it searched and lists them on demand", async () => {
    mockCommands({
      agent_session_locations: [
        {
          agent: "codex",
          locations: [
            { path: "~/.codex/sessions", found: true },
            { path: "~/.codex-custom/sessions", found: false },
          ],
        },
      ],
    })
    render(<AgentsStepSettings />)

    const toggle = await screen.findByRole("button", { name: "Searched 2 locations" })
    expect(toggle).toHaveAttribute("aria-expanded", "false")
    expect(screen.queryByText("~/.codex/sessions")).not.toBeInTheDocument()

    fireEvent.click(toggle)
    expect(toggle).toHaveAttribute("aria-expanded", "true")
    expect(screen.getByText("~/.codex/sessions")).toBeInTheDocument()
    expect(screen.getByText("~/.codex-custom/sessions").closest("li")).toHaveTextContent(
      "Not found",
    )
    expect(screen.getByText("~/.codex/sessions").closest("li")).not.toHaveTextContent(
      "Not found",
    )

    fireEvent.click(toggle)
    expect(screen.queryByText("~/.codex/sessions")).not.toBeInTheDocument()
  })

  it("still renders agent rows and toggles when the locations call fails", async () => {
    mockCommands({ agent_session_locations: new Error("no locations") })
    render(<AgentsStepSettings />)

    expect(await screen.findByText("3 sessions")).toBeInTheDocument()
    const toggle = await screen.findByRole("switch", { name: "Show Codex sessions" })
    expect(toggle).toBeChecked()
  })
})
