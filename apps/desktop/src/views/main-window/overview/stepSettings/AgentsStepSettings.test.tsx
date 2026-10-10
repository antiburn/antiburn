import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import { DEFAULT_SETTINGS, type AgentFoundCount, type ScanStatus } from "../../../../lib/ipc"
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

let AgentsStepSettings: typeof AgentsStepSettingsComponent
let foundByAgent: AgentFoundCount[]

// The list reads the shared scan status, not the Overview progress store,
// so it also works in the Settings window.
function scanStatus(): ScanStatus {
  return {
    running: false,
    completedAgents: 1,
    totalAgents: 1,
    sessions: 3,
    finishedAt: null,
    cancelled: false,
    error: null,
    agents: [],
    listChanged: false,
    reDescribed: 0,
    phase: "idle",
    foundByAgent,
    read: { completed: 3, total: 3 },
    gate: null,
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
      case "get_live_usage":
        return Promise.resolve({ meters: [], providers: [], errors: [], generatedAt: "" })
      case "agent_session_locations":
        return Promise.resolve([])
      case "get_scan_status":
        return Promise.resolve(scanStatus())
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
  foundByAgent = [{ agent: "codex", sessions: 3, done: true }]
  mockCommands()
  ;({ AgentsStepSettings } = await import("./AgentsStepSettings"))
})

describe("AgentsStepSettings coding agents", () => {
  it("leads with the agent that has the most sessions and shows its count", async () => {
    render(<AgentsStepSettings />)

    expect(await screen.findByText("3 sessions")).toBeInTheDocument()
    const rows = screen.getAllByRole("switch")
    expect(rows[0]).toHaveAccessibleName("Show Codex sessions")
  })

  it("takes its counts from the pass's found counts, not a stale scan-state total", async () => {
    // `AgentsStepSettings` must read the pass's `foundByAgent` counts, not
    // `scan_state.sessionsSeen`: a history pass can leave that column with
    // an agent's older, wider-window count.
    foundByAgent = [{ agent: "cursor", sessions: 0, done: true }]
    render(<AgentsStepSettings />)
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("get_scan_status"))
    expect(screen.queryByText(/sessions$/)).not.toBeInTheDocument()
  })

  it("shows its section title only when titled", () => {
    const { rerender } = render(<AgentsStepSettings />)
    expect(screen.queryByText("Coding agents")).not.toBeInTheDocument()
    rerender(<AgentsStepSettings titled />)
    expect(screen.getByText("Coding agents")).toBeInTheDocument()
  })

  it('shows no switch and "Not found" for an agent with no sessions', async () => {
    foundByAgent = [
      { agent: "codex", sessions: 3, done: true },
      { agent: "cursor", sessions: 0, done: true },
    ]
    render(<AgentsStepSettings />)

    expect(
      screen.queryByRole("switch", { name: "Show Cursor sessions" }),
    ).not.toBeInTheDocument()
    expect(await screen.findAllByText("Not found")).toHaveLength(1)
    expect(screen.getByRole("switch", { name: "Show Codex sessions" })).toBeChecked()
  })

  it('shows neither a switch nor "Not found" while an agent is still searching', () => {
    foundByAgent = [{ agent: "cursor", sessions: 0, done: false }]
    render(<AgentsStepSettings />)
    expect(
      screen.queryByRole("switch", { name: "Show Cursor sessions" }),
    ).not.toBeInTheDocument()
    expect(screen.queryByText("Not found")).not.toBeInTheDocument()
  })

  it("shows neither before any scan data exists", () => {
    foundByAgent = []
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
        case "get_scan_status":
          return Promise.resolve(scanStatus())
        case "set_settings":
          stored = args?.settings ?? stored
          return Promise.resolve(stored)
        case "get_live_usage":
          return Promise.resolve({ meters: [], providers: [], errors: [], generatedAt: "" })
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
        case "get_live_usage":
          return Promise.resolve({ meters: [], providers: [], errors: [], generatedAt: "" })
        case "agent_session_locations":
          return Promise.resolve([])
        case "get_scan_status":
          return Promise.resolve(scanStatus())
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
    foundByAgent = [
      { agent: "claude-code", sessions: 41, done: true },
      { agent: "codex", sessions: 87, done: true },
      { agent: "cursor", sessions: 12, done: true },
    ]
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
    // The list reads the cached snapshot; it never asks providers for usage.
    expect(invoke.mock.calls.some(([command]) => command === "refresh_live_usage")).toBe(false)
  })

  it("ignores a login that only Pi holds", async () => {
    foundByAgent = [{ agent: "codex", sessions: 0, done: true }]
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
    foundByAgent = [{ agent: "claude-code", sessions: 0, done: true }]
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
