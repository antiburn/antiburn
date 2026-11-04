import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
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
    expect(screen.queryByText("3 sessions")).not.toBeInTheDocument()
  })

  it("shows its section title only when titled", () => {
    const { rerender } = render(<AgentsStepSettings />)
    expect(screen.queryByText("Coding agents")).not.toBeInTheDocument()
    rerender(<AgentsStepSettings titled />)
    expect(screen.getByText("Coding agents")).toBeInTheDocument()
  })

  it('shows no switch and "No sessions" for an agent with no sessions', async () => {
    foundByAgent = [
      { agent: "codex", sessions: 3, done: true },
      { agent: "cursor", sessions: 0, done: true },
    ]
    render(<AgentsStepSettings />)

    expect(
      screen.queryByRole("switch", { name: "Show Cursor sessions" }),
    ).not.toBeInTheDocument()
    expect(await screen.findAllByText("No sessions")).toHaveLength(1)
    expect(screen.getByRole("switch", { name: "Show Codex sessions" })).toBeChecked()
  })

  it('shows neither a switch nor "No sessions" while an agent is still searching', () => {
    foundByAgent = [{ agent: "cursor", sessions: 0, done: false }]
    render(<AgentsStepSettings />)
    expect(
      screen.queryByRole("switch", { name: "Show Cursor sessions" }),
    ).not.toBeInTheDocument()
    expect(screen.queryByText("No sessions")).not.toBeInTheDocument()
  })

  it("shows neither before any scan data exists", () => {
    foundByAgent = []
    render(<AgentsStepSettings />)
    expect(screen.queryByRole("switch")).not.toBeInTheDocument()
    expect(screen.queryByText("No sessions")).not.toBeInTheDocument()
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
    // Opening the list asks for one user-initiated check.
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("refresh_live_usage", expect.anything()),
    )
  })

  it("shows the same three login states as the meter", async () => {
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
    expect(await screen.findByText("No sessions")).toBeInTheDocument()
    expect(screen.queryByText("Signed in")).not.toBeInTheDocument()
  })

  it("keeps Claude Desktop detection beside a completed scan with no sessions", async () => {
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

    expect(await screen.findByText("No sessions")).toBeInTheDocument()
    expect(
      screen.getByText("Claude Desktop · Limits need Claude Code signed in"),
    ).toBeInTheDocument()
    expect(screen.queryByText("Not found")).not.toBeInTheDocument()
  })
})

describe("AgentsStepSettings session locations", () => {
  it("detects an empty agent root after Sessions Rescan and return to Agents", async () => {
    foundByAgent = [{ agent: "codex", sessions: 0, done: true }]
    mockCommands({
      get_agent_integrations: [],
      agent_session_locations: [
        { agent: "codex", locations: [{ path: "~/.codex/sessions", found: false }] },
      ],
    })
    const { rerender } = render(<AgentsStepSettings manageConnections />)
    await waitFor(() => expect(screen.getByText("Searched 1 location")).toBeInTheDocument())
    expect(screen.getByRole("switch", { name: "Show Codex sessions" })).toBeDisabled()

    const { ScanningSection } = await import("./SessionSourcesSections")
    rerender(<ScanningSection discoveryPaused={false} />)
    mockCommands({
      get_agent_integrations: [],
      scan_now: scanStatus(),
      agent_session_locations: [
        { agent: "codex", locations: [{ path: "~/.codex/sessions", found: true }] },
      ],
    })
    fireEvent.click(screen.getByRole("button", { name: "Rescan" }))
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("scan_now"))
    rerender(<AgentsStepSettings manageConnections />)
    await waitFor(() =>
      expect(screen.getByRole("switch", { name: "Show Codex sessions" })).toBeEnabled(),
    )
    expect(
      invoke.mock.calls.filter(([command]) => command === "agent_session_locations"),
    ).toHaveLength(2)
  })

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

describe("Agents settings connections", () => {
  const connection = {
    vendor: "codex",
    phase: "registered",
    reason: "registered",
    hostVersion: "audit",
    profile: "/test/codex",
    nodeVersion: "22",
    owned: true,
  }

  async function renderConnections(overrides: Record<string, unknown> = {}) {
    mockCommands({
      get_agent_integrations: [connection],
      repair_agent_integration: [connection],
      ...overrides,
    })
    render(<AgentsStepSettings titled manageConnections />)
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("get_agent_integrations"))
    await waitFor(() =>
      expect(screen.getByRole("switch", { name: "Show Codex sessions" })).toBeEnabled(),
    )
  }

  it("omits the general rescan action from Agents settings", async () => {
    await renderConnections()
    expect(screen.queryByRole("button", { name: "Rescan agents" })).not.toBeInTheDocument()
    expect(invoke).not.toHaveBeenCalledWith("scan_now")
  })

  it("blocks repeated refresh presses without queueing a second request", async () => {
    await renderConnections()
    let finish!: (value: unknown) => void
    mockCommands({
      get_agent_integrations: new Promise((resolve) => {
        finish = resolve
      }),
    })
    fireEvent.click(screen.getByRole("button", { name: "Expand Codex" }))
    fireEvent.click(screen.getByRole("tab", { name: "Integration" }))
    const refresh = screen.getByRole("button", { name: "Refresh Codex integration status" })
    const before = invoke.mock.calls.filter(
      ([command]) => command === "get_agent_integrations",
    ).length
    fireEvent.click(refresh)
    fireEvent.click(refresh)
    fireEvent.click(refresh)
    expect(refresh).toHaveAttribute("aria-disabled", "true")
    expect(refresh).toHaveAttribute("aria-busy", "true")
    expect(refresh.querySelector("svg")).toHaveClass("motion-safe:animate-spin")
    expect(
      invoke.mock.calls.filter(([command]) => command === "get_agent_integrations"),
    ).toHaveLength(before + 1)
    finish([connection])
    await waitFor(() => expect(refresh).toHaveAttribute("aria-busy", "false"))
    expect(refresh.querySelector("svg")).not.toHaveClass("motion-safe:animate-spin")
    expect(
      invoke.mock.calls.filter(([command]) => command === "get_agent_integrations"),
    ).toHaveLength(before + 1)
  })

  it.each(["registered", "reload_required"])(
    "confirms an unchanged refresh in %s",
    async (phase) => {
      const current = { ...connection, phase, reason: phase }
      await renderConnections({ get_agent_integrations: [current] })
      fireEvent.click(screen.getByRole("button", { name: "Expand Codex" }))
      fireEvent.click(screen.getByRole("tab", { name: "Integration" }))
      fireEvent.click(screen.getByRole("button", { name: "Refresh Codex integration status" }))
      await waitFor(() =>
        expect(screen.getByRole("status")).toHaveTextContent("Status checked — no changes."),
      )
      if (phase === "reload_required")
        expect(screen.getByText("Restart required")).toBeInTheDocument()
    },
  )

  it("clears obsolete completion feedback after a background status change", async () => {
    await renderConnections()
    fireEvent.click(screen.getByRole("button", { name: "Expand Codex" }))
    fireEvent.click(screen.getByRole("tab", { name: "Integration" }))
    fireEvent.click(screen.getByRole("button", { name: "Refresh Codex integration status" }))
    await waitFor(() =>
      expect(screen.getByRole("status")).toHaveTextContent("Status checked — no changes."),
    )
    mockCommands({
      get_agent_integrations: [
        { ...connection, phase: "reload_required", reason: "reload_required" },
      ],
    })
    const { agentIntegrationsSession } =
      await import("../../../../lib/agentIntegrationsSession")
    await act(() => agentIntegrationsSession.refresh())
    expect(screen.getByText("Restart required")).toBeInTheDocument()
    expect(screen.getByRole("status")).toBeEmptyDOMElement()
  })

  it("shows repair progress, blocks repeated presses, and retains a restart requirement", async () => {
    let finish!: (value: unknown) => void
    const current = { ...connection, phase: "reload_required", reason: "reload_required" }
    await renderConnections({
      get_agent_integrations: [current],
      repair_agent_integration: new Promise((resolve) => {
        finish = resolve
      }),
    })
    fireEvent.click(screen.getByRole("button", { name: "Expand Codex" }))
    fireEvent.click(screen.getByRole("tab", { name: "Integration" }))
    const repair = screen.getByRole("button", { name: "Repair Codex integration" })
    fireEvent.click(repair)
    fireEvent.click(repair)
    expect(repair).toHaveAttribute("aria-busy", "true")
    expect(repair).toHaveTextContent("Repairing…")
    expect(repair.querySelector("svg")).toHaveClass(
      "lucide-loader-circle",
      "shrink-0",
      "motion-safe:animate-spin",
    )
    expect(screen.getByRole("status")).toHaveTextContent("Repairing integration setup…")
    expect(
      invoke.mock.calls.filter(([command]) => command === "repair_agent_integration"),
    ).toHaveLength(1)
    finish([current])
    await waitFor(() => expect(repair).toHaveAttribute("aria-busy", "false"))
    expect(repair).toHaveTextContent("Repair setup")
    expect(repair.querySelector("svg")).not.toHaveClass("motion-safe:animate-spin")
    expect(repair.querySelector("svg")).toHaveClass("lucide-wrench")
    expect(screen.getByRole("status")).toHaveTextContent(
      "Repair finished. Restart or reload is still required.",
    )
    expect(screen.getByText("Restart required")).toBeInTheDocument()
  })

  it("opens Session locations by default and keeps Integration in its own tab", async () => {
    await renderConnections({
      agent_session_locations: [
        { agent: "codex", locations: [{ path: "~/.codex/sessions", found: true }] },
      ],
    })
    expect(screen.queryByRole("tablist")).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Expand Codex" }))
    expect(screen.getByRole("tab", { name: "Session locations" })).toHaveAttribute(
      "aria-selected",
      "true",
    )
    expect(screen.getByText("~/.codex/sessions")).toBeInTheDocument()
    expect(screen.getByText("~/.codex/sessions")).toBeInTheDocument()
    expect(
      screen.queryByRole("button", { name: "Remove Codex integration" }),
    ).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole("tab", { name: "Integration" }))
    expect(screen.queryByText("~/.codex/sessions")).not.toBeInTheDocument()
    expect(screen.getByText("Integration connected.", { selector: "p" })).toHaveClass(
      "agent-settings-badge-success",
    )
    expect(screen.queryByRole("heading", { name: /Antiburn/ })).not.toBeInTheDocument()
  })

  it("opens the warning badge directly to Integration without changing the session switch", async () => {
    await renderConnections({
      get_agent_integrations: [{ ...connection, phase: "awaiting_trust" }],
    })
    const badge = screen.getByRole("button", { name: "Open Antiburn integration for Codex" })
    expect(badge).toHaveAttribute("aria-expanded", "false")
    fireEvent.click(badge)
    const tab = screen.getByRole("tab", { name: "Integration" })
    expect(tab).toHaveAttribute("aria-selected", "true")
    expect(tab).toHaveFocus()
    fireEvent.click(badge)
    expect(badge).toHaveAttribute("aria-expanded", "true")
    expect(tab).toHaveFocus()
    expect(screen.getByRole("switch", { name: "Show Codex sessions" })).toBeChecked()
    fireEvent.click(screen.getByRole("button", { name: "Collapse Codex" }))
    expect(screen.queryByRole("tabpanel")).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Expand Codex" }))
    expect(screen.getByRole("tab", { name: "Session locations" })).toHaveAttribute(
      "aria-selected",
      "true",
    )
  })

  function clickMaintenance() {
    const button = screen.getByRole("button", { name: "Repair Codex integration" })
    button.focus()
    fireEvent.click(button)
  }

  it.each(["registered", "awaiting_trust", "user_changed"])(
    "omits integration removal controls for %s",
    async (phase) => {
      await renderConnections({ get_agent_integrations: [{ ...connection, phase }] })
      fireEvent.click(screen.getByRole("button", { name: "Expand Codex" }))
      fireEvent.click(screen.getByRole("tab", { name: "Integration" }))
      expect(
        screen.queryByRole("button", { name: /Remove.*integration/i }),
      ).not.toBeInTheDocument()
      expect(screen.queryByRole("group", { name: /Confirm removal/ })).not.toBeInTheDocument()
      expect(
        screen.getByRole("button", { name: "Refresh Codex integration status" }),
      ).toBeEnabled()
      expect(screen.getByRole("button", { name: "Repair Codex integration" })).toBeEnabled()
      expect(invoke).not.toHaveBeenCalledWith("remove_agent_integration", expect.anything())
    },
  )

  it.each(["not_detected", "pending_native_gate", "disabled", "unavailable"])(
    "keeps Integration reachable without a row badge for %s",
    async (phase) => {
      await renderConnections({
        get_agent_integrations: [{ ...connection, phase, owned: false }],
      })
      const badge = screen.queryByRole("button", {
        name: "Open Antiburn integration for Codex",
      })
      expect(badge).not.toBeInTheDocument()
      fireEvent.click(screen.getByRole("button", { name: "Expand Codex" }))
      fireEvent.click(screen.getByRole("tab", { name: "Integration" }))
      fireEvent.click(screen.getByRole("tab", { name: "Integration" }))
      expect(
        screen.getByRole("region", { name: "Codex Antiburn integration" }),
      ).toBeInTheDocument()
    },
  )

  it("keeps successful integration compact and technical details collapsed", async () => {
    await renderConnections({
      get_agent_integrations: [
        { ...connection, hostVersion: "audit fixture", profile: "/test/profile" },
      ],
    })
    fireEvent.click(screen.getByRole("button", { name: "Expand Codex" }))
    fireEvent.click(screen.getByRole("tab", { name: "Integration" }))
    expect(screen.getByText("Integration connected.", { selector: "p" })).toHaveClass(
      "agent-settings-badge-success",
    )
    expect(screen.queryByText("Antiburn is integrated with Codex.")).not.toBeInTheDocument()
    expect(screen.queryByRole("menuitem")).not.toBeInTheDocument()
    const disclosure = screen.getByText("Technical details").closest("details")!
    expect(disclosure).not.toHaveAttribute("open")
    fireEvent.click(screen.getByText("Technical details"))
    expect(disclosure).toHaveAttribute("open")
    expect(screen.getByText("/test/profile")).toBeVisible()
  })

  it("explains missing setup files without offering repair or an unsupported setup action", async () => {
    await renderConnections({
      get_agent_integrations: [{ ...connection, phase: "not_detected", owned: false }],
    })
    fireEvent.click(screen.getByRole("button", { name: "Expand Codex" }))
    fireEvent.click(screen.getByRole("tab", { name: "Integration" }))
    expect(screen.getByText("Integration isn’t set up.")).toBeInTheDocument()
    expect(screen.getByText(/Setup files for Codex weren’t found/)).toBeInTheDocument()
    expect(
      screen.queryByRole("button", { name: "Manage Codex integration" }),
    ).not.toBeInTheDocument()
    expect(
      screen.getByRole("button", { name: "Refresh Codex integration status" }),
    ).toBeEnabled()
  })

  it("shows approval steps immediately without duplicate guidance or a reveal button", async () => {
    await renderConnections({
      get_agent_integrations: [{ ...connection, phase: "awaiting_trust" }],
    })
    fireEvent.click(screen.getByRole("button", { name: "Expand Codex" }))
    fireEvent.click(screen.getByRole("tab", { name: "Integration" }))
    expect(screen.getByText("Approval required", { selector: "p" })).toHaveClass(
      "agent-settings-badge-attention",
    )
    expect(screen.getByText("Open Codex.")).toBeVisible()
    expect(screen.getByText("Approve Antiburn when prompted.")).toBeVisible()
    expect(screen.getByText("Restart or reload Codex.")).toBeVisible()
    expect(screen.getAllByRole("listitem")).toHaveLength(3)
    expect(screen.queryByRole("button", { name: /approval steps/ })).not.toBeInTheDocument()
    expect(screen.queryByText(/Approve Antiburn in Codex/)).not.toBeInTheDocument()
    const maintenance = screen.getByRole("group", { name: "Codex integration maintenance" })
    expect(maintenance).toContainElement(
      screen.getByRole("button", { name: "Refresh Codex integration status" }),
    )
    expect(maintenance).toContainElement(
      screen.getByRole("button", { name: "Repair Codex integration" }),
    )
    expect(
      screen.queryByRole("button", { name: "Remove Codex integration" }),
    ).not.toBeInTheDocument()
    expect(
      screen.queryByRole("button", { name: "Manage Codex integration" }),
    ).not.toBeInTheDocument()
    clickMaintenance()
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("repair_agent_integration", { vendor: "codex" }),
    )
  })

  it("shows shared guidance once, suppresses badges, and keeps the tab available", async () => {
    await renderConnections({
      get_agent_integrations: ["claude", "codex", "cursor", "open_code", "pi"].map(
        (vendor) => ({
          ...connection,
          vendor,
          owned: false,
          phase: "unavailable",
          reason: "development_build",
        }),
      ),
    })
    expect(
      screen.queryByText("Integration setup is available in packaged builds."),
    ).not.toBeInTheDocument()
    expect(
      screen.queryByRole("button", { name: /Open Antiburn integration/ }),
    ).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Expand Codex" }))
    fireEvent.click(screen.getByRole("tab", { name: "Integration" }))
    expect(
      screen.getAllByText("Integration setup is available in packaged builds."),
    ).toHaveLength(1)
  })

  it("opens CLI settings through a neutral button without repairing it", async () => {
    mockCommands({
      get_agent_integrations: [
        { ...connection, phase: "unavailable", reason: "managed_cli_unavailable" },
      ],
    })
    const open = vi.fn()
    render(<AgentsStepSettings titled manageConnections onOpenCli={open} />)
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("get_agent_integrations"))
    fireEvent.click(screen.getByRole("button", { name: "Expand Codex" }))
    fireEvent.click(screen.getByRole("tab", { name: "Integration" }))
    fireEvent.click(await screen.findByRole("tab", { name: "Integration" }))
    const button = screen.getByRole("button", { name: "Open CLI settings" })
    expect(button).not.toHaveClass("bg-accent-fill")
    fireEvent.click(button)
    expect(open).toHaveBeenCalledOnce()
    expect(invoke).not.toHaveBeenCalledWith("repair_cli_installation")
  })

  it("keeps actions busy together while session visibility stays independent", async () => {
    let finish: (value: unknown) => void = () => {}
    await renderConnections({
      repair_agent_integration: new Promise((resolve) => {
        finish = resolve
      }),
    })
    fireEvent.click(screen.getByRole("button", { name: "Expand Codex" }))
    fireEvent.click(screen.getByRole("tab", { name: "Integration" }))
    clickMaintenance()
    expect(
      screen.getByRole("button", { name: "Refresh Codex integration status" }),
    ).toHaveAttribute("aria-disabled", "true")
    const repair = screen.getByRole("button", { name: "Repair Codex integration" })
    expect(repair).toHaveAttribute("aria-disabled", "true")
    expect(repair).toHaveFocus()
    fireEvent.click(repair)
    expect(
      invoke.mock.calls.filter(([command]) => command === "repair_agent_integration"),
    ).toHaveLength(1)
    expect(screen.queryByRole("group", { name: /Confirm removal/ })).not.toBeInTheDocument()
    const reads = invoke.mock.calls.filter(
      ([command]) => command === "get_agent_integrations",
    ).length
    fireEvent.click(screen.getByRole("button", { name: "Refresh Codex integration status" }))
    expect(
      invoke.mock.calls.filter(([command]) => command === "get_agent_integrations"),
    ).toHaveLength(reads)
    expect(screen.getByRole("switch", { name: "Show Codex sessions" })).toBeEnabled()
    finish([connection])
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Refresh Codex integration status" }),
      ).not.toHaveAttribute("aria-disabled", "true"),
    )
  })

  it.each(["refresh", "repair"])(
    "restores Integration focus when %s changes setup state",
    async (action) => {
      let finish: (value: unknown) => void = () => {}
      const pending = new Promise((resolve) => {
        finish = resolve
      })
      await renderConnections({ repair_agent_integration: pending })
      fireEvent.click(screen.getByRole("button", { name: "Expand Codex" }))
      fireEvent.click(screen.getByRole("tab", { name: "Integration" }))
      if (action === "refresh") {
        mockCommands({ get_agent_integrations: pending })
        const refresh = screen.getByRole("button", { name: "Refresh Codex integration status" })
        refresh.focus()
        fireEvent.click(refresh)
      } else {
        clickMaintenance()
        expect(screen.getByRole("button", { name: "Repair Codex integration" })).toHaveFocus()
      }
      finish([{ ...connection, phase: "awaiting_trust", reason: "approval_required" }])
      expect(await screen.findByText("Approval required")).toBeInTheDocument()
      await waitFor(() =>
        expect(screen.getByRole("status")).toHaveTextContent(
          action === "refresh"
            ? "Status updated."
            : "Repair finished. Approval is still required.",
        ),
      )
      await waitFor(() =>
        expect(screen.getByRole("tab", { name: "Integration" })).toHaveFocus(),
      )
    },
  )

  it("keeps failed repair retryable and clears errors after refresh", async () => {
    await renderConnections({ repair_agent_integration: new Error("private backend details") })
    fireEvent.click(screen.getByRole("button", { name: "Expand Codex" }))
    fireEvent.click(screen.getByRole("tab", { name: "Integration" }))
    clickMaintenance()
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Could not update agent integrations. Try again.",
    )
    expect(screen.queryByText("private backend details")).not.toBeInTheDocument()
    expect(screen.getByRole("status")).toBeEmptyDOMElement()
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "Repair Codex integration" }),
      ).not.toHaveAttribute("aria-disabled", "true"),
    )
    fireEvent.click(screen.getByRole("button", { name: "Refresh Codex integration status" }))
    await waitFor(() => expect(screen.queryByRole("alert")).not.toBeInTheDocument())
  })
})
