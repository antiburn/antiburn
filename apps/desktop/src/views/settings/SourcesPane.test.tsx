import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import { DEFAULT_SETTINGS } from "../../lib/ipc"
import { SourcesPane } from "./SourcesPane"

/**
 * The Sources pane owns scanning now that the popover carries no status line:
 * the status sentence, the on-demand rescan, and the paused wording all live
 * here, so this file keeps the coverage those popover tests used to hold.
 */

const invoke = vi.hoisted(() => vi.fn())
const openDialog = vi.hoisted(() => vi.fn())

vi.mock("@tauri-apps/api/core", () => ({ invoke, isTauri: () => true }))
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
}))
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: openDialog }))

const SCAN_STATUS = {
  running: false,
  completedAgents: 11,
  totalAgents: 11,
  sessions: 4,
  finishedAt: new Date(Date.now() - 120_000).toISOString(),
  cancelled: false,
  error: null,
  agents: [],
}

function mockCommands(overrides: Record<string, unknown> = {}) {
  invoke.mockImplementation((command: string) => {
    if (command in overrides) return Promise.resolve(overrides[command])
    switch (command) {
      case "get_scan_status":
      case "scan_now":
        return Promise.resolve(SCAN_STATUS)
      case "list_scan_roots":
      case "list_repositories":
        return Promise.resolve([])
      default:
        return Promise.resolve(null)
    }
  })
}

beforeEach(() => {
  vi.clearAllMocks()
  mockCommands()
})

describe("SourcesPane scanning", () => {
  it("shows when the index was last refreshed and can rescan on demand", async () => {
    mockCommands({
      get_folder_permissions: {
        supported: true,
        deferred: [{ dir: "Documents", pathCount: 1 }],
        granted: [],
      },
    })
    render(<SourcesPane discoveryPaused={false} />)

    expect(await screen.findByText(/scanned 2m ago/i)).toBeInTheDocument()
    const warning = await screen.findByText("antiburn can’t read Documents yet.")
    const remote = screen.getByRole("heading", { name: "Remote hosts" })
    expect(warning.compareDocumentPosition(remote) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(
      0,
    )
    expect(screen.getAllByRole("heading", { level: 2 })[0]).toBe(remote)

    fireEvent.click(screen.getByRole("button", { name: "Rescan" }))
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("scan_now"))
  })

  it("says so plainly while discovery is paused", async () => {
    render(<SourcesPane discoveryPaused />)

    expect(await screen.findByText("Scanning paused")).toBeInTheDocument()
    // Pausing background work never removes the way to ask for a pass.
    expect(screen.getByRole("button", { name: "Rescan" })).toBeInTheDocument()
  })

  it("disables the rescan affordance while a scan is running", async () => {
    mockCommands({
      get_scan_status: { ...SCAN_STATUS, running: true, finishedAt: null },
    })
    render(<SourcesPane discoveryPaused={false} />)

    const button = await screen.findByRole("button", { name: /scanning/i })
    expect(button).toBeDisabled()
  })
})

describe("SourcesPane folders without git", () => {
  it("saves the switch and leaves it off by default", async () => {
    let stored = { ...DEFAULT_SETTINGS }
    invoke.mockImplementation((command: string, args?: { settings?: typeof stored }) => {
      switch (command) {
        case "get_settings":
          return Promise.resolve(stored)
        case "set_settings":
          stored = args?.settings ?? stored
          return Promise.resolve(stored)
        case "get_scan_status":
          return Promise.resolve(SCAN_STATUS)
        case "list_scan_roots":
        case "list_repositories":
          return Promise.resolve([])
        default:
          return Promise.resolve(null)
      }
    })
    render(<SourcesPane discoveryPaused={false} />)

    const toggle = await screen.findByRole("switch", { name: "Include folders without git" })
    expect(toggle).not.toBeChecked()

    fireEvent.click(toggle)
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("set_settings", {
        settings: expect.objectContaining({ includeNonRepoFolders: true }),
      }),
    )
    await waitFor(() => expect(toggle).toBeChecked())
  })
})

describe("SourcesPane coding agents", () => {
  it("names what each agent has on this computer and leaves the rest blank", async () => {
    mockCommands({
      get_scan_status: {
        ...SCAN_STATUS,
        agents: [
          { agent: "claude-code", lastCompletedAt: null, sessionsSeen: 41 },
          { agent: "codex", lastCompletedAt: null, sessionsSeen: 87 },
          { agent: "cursor", lastCompletedAt: null, sessionsSeen: 12 },
        ],
      },
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
    render(<SourcesPane discoveryPaused={false} />)

    expect(
      await screen.findByText(
        "41 sessions · Claude Desktop · Limits need Claude Code signed in",
      ),
    ).toBeInTheDocument()
    expect(screen.getByText("87 sessions · Signed in")).toBeInTheDocument()
    expect(screen.getByText("12 sessions")).toBeInTheDocument()
    expect(screen.getByRole("switch", { name: "Show Claude sessions" })).toBeInTheDocument()
    const devin = screen.getByRole("switch", { name: "Show Devin sessions" }).closest("div")!
    expect(devin.textContent).toBe("Devin")
    expect(invoke.mock.calls.some(([command]) => command === "refresh_live_usage")).toBe(false)
  })
})
