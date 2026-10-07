import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import { DEFAULT_SETTINGS } from "../../../../lib/ipc"
import { SessionsStepSettings } from "./SessionsStepSettings"

/**
 * The Sessions step owns the activity window, monitoring, scanning, folders,
 * repositories, remote hosts, the historical scan, indexed sessions, and
 * retention now that the General, Privacy, and Sources panes are gone: this
 * file keeps the coverage those panes' tests used to hold, plus the coverage
 * `AgentsStepSettings.test.tsx` held for the sections that moved here.
 */

const invoke = vi.hoisted(() => vi.fn())
const confirmDialog = vi.hoisted(() => vi.fn())
const openDialog = vi.hoisted(() => vi.fn())

vi.mock("@tauri-apps/api/core", () => ({ invoke, isTauri: () => true }))
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
}))
vi.mock("@tauri-apps/plugin-dialog", () => ({ confirm: confirmDialog, open: openDialog }))

const SCAN_STATUS = {
  running: false,
  completedAgents: 11,
  totalAgents: 11,
  sessions: 42,
  finishedAt: new Date(Date.now() - 5 * 60_000).toISOString(),
  cancelled: false,
  error: null,
  agents: [],
}

const INFO = {
  appVersion: "0.1.0",
  debugBuild: false,
  arch: "aarch64",
  pricingCatalogVersion: "2026-08-12",
  schemaVersion: 1,
  dataDir: "/home/avery/Library/Application Support/ai.antiburn.desktop",
  indexedSessions: 42,
  databaseBytes: 3_670_016,
  updatesSupported: false,
  analyticsSupported: true,
  analyticsEnvironmentDisabled: false,
  analyticsOperator: "Cadence AI (Vic) Pty Ltd",
}

function mockCommands(overrides: Record<string, unknown> = {}) {
  invoke.mockImplementation((command: string, args?: Record<string, unknown>) => {
    if (command in overrides) {
      const override = overrides[command]
      return Promise.resolve(typeof override === "function" ? override(args) : override)
    }
    switch (command) {
      case "get_settings":
        return Promise.resolve(DEFAULT_SETTINGS)
      case "get_scan_status":
        return Promise.resolve(SCAN_STATUS)
      case "app_info":
        return Promise.resolve(INFO)
      case "list_scan_roots":
      case "list_repositories":
      case "refresh_repositories":
        return Promise.resolve([])
      case "get_folder_permissions":
        return Promise.resolve({ supported: false, deferred: [], granted: [] })
      default:
        return Promise.resolve(null)
    }
  })
}

beforeEach(() => {
  vi.clearAllMocks()
  confirmDialog.mockReset()
  openDialog.mockReset()
  mockCommands()
})

describe("SessionsStepSettings scanning and indexing", () => {
  it("reports what the index holds and starts the older-sessions scan", async () => {
    mockCommands({
      get_scan_status: {
        ...SCAN_STATUS,
        history: { state: "done", completed: 300, total: 300, passRunning: false },
      },
    })
    render(<SessionsStepSettings />)

    expect(await screen.findByText("42 sessions · 3.5 MB")).toBeInTheDocument()
    expect(screen.getByText("Scanned 5m ago")).toBeInTheDocument()
    expect(screen.getByText("300 older sessions processed")).toBeInTheDocument()

    fireEvent.click(screen.getByRole("button", { name: "Scan now" }))
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("scan_history"))
  })

  it("offers Stop only while the older-sessions scan itself runs", async () => {
    mockCommands({
      get_scan_status: {
        ...SCAN_STATUS,
        running: true,
        history: { state: "running", completed: 10, total: 300, passRunning: false },
      },
    })
    const { unmount } = render(<SessionsStepSettings />)

    // A routine pass: the recent row is busy, and there is nothing to stop.
    expect(await screen.findByRole("button", { name: /scanning/i })).toBeDisabled()
    expect(screen.queryByRole("button", { name: "Stop" })).not.toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Scan now" })).toBeInTheDocument()
    unmount()

    mockCommands({
      get_scan_status: {
        ...SCAN_STATUS,
        running: true,
        history: { state: "running", completed: 0, total: 0, passRunning: true },
      },
    })
    render(<SessionsStepSettings />)

    // The historical pass: the recent row keeps its last result.
    fireEvent.click(await screen.findByRole("button", { name: "Stop" }))
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("cancel_scan"))
    expect(screen.getByText("Scanned 5m ago")).toBeInTheDocument()
    expect(screen.getByText("Looking for older sessions…")).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Rescan" })).toBeDisabled()
  })

  it("persists the activity window", async () => {
    render(<SessionsStepSettings />)

    const slider = await screen.findByRole("slider", { name: "Days of activity to show" })
    fireEvent.change(slider, { target: { value: "14" } })

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("set_settings", {
        settings: expect.objectContaining({ activityWindowDays: 14 }),
      }),
    )
  })

  it("does not let an older failed write replace a newer saved value", async () => {
    confirmDialog.mockResolvedValue(true)
    let rejectFirst: (error: Error) => void = () => {}
    let resolveSecond: (settings: typeof DEFAULT_SETTINGS) => void = () => {}
    let write = 0
    mockCommands({
      set_settings: () => {
        write += 1
        if (write === 1) {
          return new Promise((_, reject) => {
            rejectFirst = reject
          })
        }
        return new Promise((resolve) => {
          resolveSecond = resolve
        })
      },
    })
    render(<SessionsStepSettings />)

    const slider = await screen.findByRole("slider", { name: "Days of activity to show" })
    const retention = await screen.findByRole("radiogroup", { name: "Session data retention" })
    fireEvent.change(slider, { target: { value: "14" } })
    fireEvent.click(within(retention).getByRole("radio", { name: "90 days" }))
    await waitFor(() => expect(write).toBe(2))

    await act(async () => {
      resolveSecond({
        ...DEFAULT_SETTINGS,
        activityWindowDays: 14,
        sessionDataRetentionDays: 90,
      })
    })
    await act(async () => {
      rejectFirst(new Error("disk full"))
    })

    expect(slider).toHaveValue("14")
    expect(within(retention).getByRole("radio", { name: "90 days" })).toHaveAttribute(
      "aria-checked",
      "true",
    )
  })
})

describe("SessionsStepSettings retention", () => {
  it("defaults session retention to forever and confirms a shorter period", async () => {
    confirmDialog.mockResolvedValue(true)
    render(<SessionsStepSettings />)

    const retention = await screen.findByRole("radiogroup", { name: "Session data retention" })
    expect(within(retention).getByRole("radio", { name: "Forever" })).toHaveAttribute(
      "aria-checked",
      "true",
    )

    fireEvent.click(within(retention).getByRole("radio", { name: "30 days" }))

    await waitFor(() => expect(confirmDialog).toHaveBeenCalledTimes(1))
    const [message] = confirmDialog.mock.calls[0] as [string]
    expect(message).toMatch(/providers retain session history for only 30 days/i)
    expect(message).toMatch(/transcript files are not touched/i)
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("set_settings", {
        settings: expect.objectContaining({ sessionDataRetentionDays: 30 }),
      }),
    )
  })

  it("keeps retention unchanged when shortening is declined", async () => {
    confirmDialog.mockResolvedValue(false)
    render(<SessionsStepSettings />)

    const retention = await screen.findByRole("radiogroup", { name: "Session data retention" })
    invoke.mockClear()
    fireEvent.click(within(retention).getByRole("radio", { name: "90 days" }))

    await waitFor(() => expect(confirmDialog).toHaveBeenCalledTimes(1))
    expect(invoke).not.toHaveBeenCalledWith("set_settings", expect.anything())
    expect(within(retention).getByRole("radio", { name: "Forever" })).toHaveAttribute(
      "aria-checked",
      "true",
    )
  })

  it("widens retention to 90 days without confirmation", async () => {
    mockCommands({ get_settings: { ...DEFAULT_SETTINGS, sessionDataRetentionDays: 30 } })
    render(<SessionsStepSettings />)

    const retention = await screen.findByRole("radiogroup", { name: "Session data retention" })
    await waitFor(() =>
      expect(within(retention).getByRole("radio", { name: "30 days" })).toHaveAttribute(
        "aria-checked",
        "true",
      ),
    )
    invoke.mockClear()
    fireEvent.click(within(retention).getByRole("radio", { name: "90 days" }))

    expect(confirmDialog).not.toHaveBeenCalled()
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("set_settings", {
        settings: expect.objectContaining({ sessionDataRetentionDays: 90 }),
      }),
    )
  })
})

describe("SessionsStepSettings scanning", () => {
  it("shows when the index was last refreshed and can rescan on demand", async () => {
    mockCommands({
      get_scan_status: {
        ...SCAN_STATUS,
        finishedAt: new Date(Date.now() - 120_000).toISOString(),
      },
      get_folder_permissions: {
        supported: true,
        deferred: [{ dir: "Documents", pathCount: 1 }],
        granted: [],
      },
    })
    render(<SessionsStepSettings />)

    expect(await screen.findByText("Scanned 2m ago")).toBeInTheDocument()
    // The permission notice leads this step's sections, ahead of Remote
    // hosts, so a blocked folder is the first thing a reader sees here.
    const warning = await screen.findByText("antiburn can’t read Documents yet.")
    const remote = screen.getByRole("heading", { name: "Remote hosts" })
    expect(warning.compareDocumentPosition(remote) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(
      0,
    )

    fireEvent.click(screen.getByRole("button", { name: "Rescan" }))
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("scan_now"))
  })

  it("says so plainly while discovery is paused", async () => {
    mockCommands({ get_settings: { ...DEFAULT_SETTINGS, discoveryPaused: true } })
    render(<SessionsStepSettings />)

    expect(await screen.findByText("Scanning paused")).toBeInTheDocument()
    // Pausing background work never removes the way to ask for a pass.
    expect(screen.getByRole("button", { name: "Rescan" })).toBeInTheDocument()
  })

  it("disables the rescan affordance while a scan is running", async () => {
    mockCommands({
      get_scan_status: { ...SCAN_STATUS, running: true, finishedAt: null },
    })
    render(<SessionsStepSettings />)

    const button = await screen.findByRole("button", { name: /scanning/i })
    expect(button).toBeDisabled()
  })
})

describe("SessionsStepSettings folders without git", () => {
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
        case "app_info":
          return Promise.resolve(INFO)
        case "list_scan_roots":
        case "list_repositories":
          return Promise.resolve([])
        case "get_folder_permissions":
          return Promise.resolve({ supported: false, deferred: [], granted: [] })
        default:
          return Promise.resolve(null)
      }
    })
    render(<SessionsStepSettings />)

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

describe("SessionsStepSettings monitoring", () => {
  it("persists the monitoring switch as the same preference the popover pauses", async () => {
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
        case "app_info":
          return Promise.resolve(INFO)
        case "list_scan_roots":
        case "list_repositories":
          return Promise.resolve([])
        case "get_folder_permissions":
          return Promise.resolve({ supported: false, deferred: [], granted: [] })
        default:
          return Promise.resolve(null)
      }
    })
    render(<SessionsStepSettings />)

    const toggle = await screen.findByRole("switch", {
      name: "Keep looking for new sessions",
    })
    // On by default: the stored preference is `discoveryPaused`, and this
    // control is its inverse, so a reader is never asked to reason about a
    // double negative.
    expect(toggle).toBeChecked()

    fireEvent.click(toggle)
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("set_settings", {
        settings: expect.objectContaining({ discoveryPaused: true }),
      }),
    )
  })
})

describe("SessionsStepSettings scan folders", () => {
  it("adds a scan folder through the directory picker", async () => {
    let roots: string[] = []
    invoke.mockImplementation((command: string, args?: { path?: string }) => {
      switch (command) {
        case "get_settings":
          return Promise.resolve(DEFAULT_SETTINGS)
        case "get_scan_status":
          return Promise.resolve(SCAN_STATUS)
        case "app_info":
          return Promise.resolve(INFO)
        case "list_repositories":
        case "refresh_repositories":
          return Promise.resolve([])
        case "list_scan_roots":
          return Promise.resolve(roots)
        case "add_scan_root":
          roots = [...roots, args!.path!]
          return Promise.resolve(roots)
        case "get_folder_permissions":
          return Promise.resolve({ supported: false, deferred: [], granted: [] })
        default:
          return Promise.resolve(null)
      }
    })
    openDialog.mockResolvedValue("/home/avery/work")
    render(<SessionsStepSettings />)

    fireEvent.click(await screen.findByRole("button", { name: "Add a folder…" }))

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("add_scan_root", { path: "/home/avery/work" }),
    )
    expect(await screen.findByText("/home/avery/work")).toBeInTheDocument()
  })
})
