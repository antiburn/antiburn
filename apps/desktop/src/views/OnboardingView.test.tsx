import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import { OnboardingView } from "./OnboardingView"

const invoke = vi.hoisted(() => vi.fn())
const openDialog = vi.hoisted(() => vi.fn())
const listeners = vi.hoisted(() => new Map<string, ((event: { payload: unknown }) => void)[]>())

vi.mock("@tauri-apps/api/core", () => ({ invoke, isTauri: () => true }))
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (name: string, handler: (event: { payload: unknown }) => void) => {
    listeners.set(name, [...(listeners.get(name) ?? []), handler])
    return () => {
      listeners.set(
        name,
        (listeners.get(name) ?? []).filter((each) => each !== handler),
      )
    }
  }),
}))
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: openDialog }))

const SETTINGS = {
  theme: "system" as const,
  activityWindowDays: 7,
  sessionDataRetentionDays: -1,
  onboardingCompleted: false,
  launchAtLogin: true,
  autoUpdate: true,
  discoveryPaused: false,
  analyticsEnabled: true,
  disabledAgents: [],
  nudgesRespectDnd: false,
}

/** A finished analysis pass over the four scanned sessions. */
const HYGIENE_SUMMARY = {
  totalSessions: 4,
  settledSessions: 4,
  analyzedSessions: 4,
  failingSessions: 1,
  mostCommonFinding: "modelOverthinking",
}

/** An analytics-capable official build. Source builds use the unsupported case. */
const APP_INFO = {
  appVersion: "0.1.0",
  debugBuild: false,
  arch: "aarch64",
  updatesSupported: false,
  analyticsSupported: true,
  analyticsEnvironmentDisabled: false,
  analyticsOperator: "the antiburn team",
}

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

const FAILED_SCAN_STATUS = {
  ...SCAN_STATUS,
  completedAgents: 3,
  sessions: 0,
  error: "Could not read ~/.claude/projects.",
}

const REPOSITORY = {
  key: "/home/avery/code/widgets",
  repoName: "widgets",
  fullName: "avery/widgets",
  status: "accessible",
  repoRoot: "/home/avery/code/widgets",
  suspectedPath: null,
  worktreeCount: 1,
  sessionCount: 3,
  wslDistro: null,
  enabled: true,
}

function mockCommands(overrides: Record<string, unknown> = {}) {
  invoke.mockImplementation((command: string, args?: unknown) => {
    if (command in overrides) {
      const value = overrides[command]
      return Promise.resolve(
        typeof value === "function" ? (value as (args?: unknown) => unknown)(args) : value,
      )
    }
    switch (command) {
      case "get_settings":
        return Promise.resolve(SETTINGS)
      case "app_info":
        return Promise.resolve(APP_INFO)
      case "scan_now":
        return Promise.resolve(SCAN_STATUS)
      // A fresh install: nothing has been scanned, so the repository step has
      // to ask for a pass before it has anything to show.
      case "get_scan_status":
        return Promise.resolve(null)
      case "set_settings":
        return Promise.resolve((args as Record<string, unknown> | undefined)?.["settings"])
      case "finish_onboarding":
        return Promise.resolve({ ...SETTINGS, onboardingCompleted: true })
      case "get_hygiene_summary":
        return Promise.resolve(HYGIENE_SUMMARY)
      case "list_scan_roots":
      case "default_scan_roots":
      case "list_repositories":
      case "set_repository_enabled":
        return Promise.resolve([])
      // Both return the roots as they now stand, not an acknowledgement.
      case "add_scan_root":
        return Promise.resolve(["/home/avery/work"])
      case "remove_scan_root":
        return Promise.resolve([])
      default:
        return Promise.resolve(null)
    }
  })
}

async function advanceToReady() {
  await screen.findByRole("heading", { name: "Stop hitting your token limits." })
  for (let step = 0; step < 3; step += 1) {
    fireEvent.click(screen.getByRole("button", { name: "Continue" }))
  }
  await screen.findByRole("heading", { name: "Ready" })
}

/** Move from Welcome past the agents step to the search-locations step. */
async function advanceToSources() {
  fireEvent.click(await screen.findByRole("button", { name: "Continue" }))
  await screen.findByRole("heading", { name: "Scan Locations: Agents" })
  fireEvent.click(screen.getByRole("button", { name: "Continue" }))
  await screen.findByRole("heading", { name: "Scan Locations: Repos" })
}

describe("OnboardingView", () => {
  beforeEach(() => {
    invoke.mockReset()
    openDialog.mockReset()
    listeners.clear()
    mockCommands()
  })

  it("does not expose controls backed by defaults while settings are loading", async () => {
    let resolveSettings!: (settings: typeof SETTINGS) => void
    const pendingSettings = new Promise<typeof SETTINGS>((resolve) => {
      resolveSettings = resolve
    })
    mockCommands({ get_settings: pendingSettings })

    render(<OnboardingView />)

    expect(await screen.findByText("Preparing antiburn…")).toBeInTheDocument()
    expect(
      screen.queryByRole("heading", { name: "Stop hitting your token limits." }),
    ).not.toBeInTheDocument()

    resolveSettings(SETTINGS)
    expect(
      await screen.findByRole("heading", { name: "Stop hitting your token limits." }),
    ).toBeInTheDocument()
  })

  it("releases its shell subscriptions when the window view unmounts", async () => {
    const { unmount } = render(<OnboardingView />)
    await screen.findByRole("heading", { name: "Stop hitting your token limits." })

    expect(listeners.get("scan:started")).toHaveLength(1)
    expect(listeners.get("scan:progress")).toHaveLength(1)
    expect(listeners.get("scan:finished")).toHaveLength(1)

    unmount()

    await waitFor(() => {
      expect(listeners.get("scan:started")).toHaveLength(0)
      expect(listeners.get("scan:progress")).toHaveLength(0)
      expect(listeners.get("scan:finished")).toHaveLength(0)
    })
  })

  it("runs the four-step first-run flow and records that it finished", async () => {
    mockCommands({ default_scan_roots: ["/home/avery/code"] })
    render(<OnboardingView />)

    // 1 — Welcome. No account, and nothing of the reader's work leaving.
    expect(
      await screen.findByRole("heading", { name: "Stop hitting your token limits." }),
    ).toBeInTheDocument()
    expect(screen.getByText(/nothing from your sessions is ever uploaded/i)).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Continue" }))

    // 2 — Coding agents. Discovery starts on leaving Welcome.
    expect(
      await screen.findByRole("heading", { name: "Scan Locations: Agents" }),
    ).toBeInTheDocument()
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("scan_now", { activityWindowDays: 7 }),
    )
    fireEvent.click(screen.getByRole("button", { name: "Continue" }))

    // 3 — Search locations and repositories share the discovery pass.
    expect(
      await screen.findByRole("heading", { name: "Scan Locations: Repos" }),
    ).toBeInTheDocument()
    expect(screen.getByRole("heading", { name: "Repos found" })).toBeInTheDocument()
    expect(screen.getByText("/home/avery/code")).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Continue" }))

    // 4 — Ready. The analysis numbers arrive from the summary poll.
    expect(await screen.findByRole("heading", { name: "Ready" })).toBeInTheDocument()
    expect(
      screen.getByText(
        "4 sessions from the last 7 days are indexed and waiting in the menu bar.",
      ),
    ).toBeInTheDocument()
    expect(await screen.findByText("sessions analyzed")).toBeInTheDocument()
    expect(screen.getByText("75%")).toBeInTheDocument()
    expect(screen.getByText("Model overthinking")).toBeInTheDocument()
    expect(screen.getByRole("switch", { name: "Launch antiburn on startup" })).toBeChecked()
    expect(
      screen.getByRole("switch", { name: "Nudges respect Do Not Disturb" }),
    ).not.toBeChecked()
    fireEvent.click(screen.getByRole("button", { name: "Start using antiburn" }))

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("finish_onboarding", {
        activityWindowDays: 7,
        launchAtLogin: true,
        disabledAgents: [],
        nudgesRespectDnd: false,
      }),
    )
  })

  it("lists detected agents and persists switched-off agents on finish", async () => {
    mockCommands({
      get_scan_status: {
        ...SCAN_STATUS,
        agents: [
          { agent: "claude-code", lastCompletedAt: null, sessionsSeen: 304 },
          { agent: "codex", lastCompletedAt: null, sessionsSeen: 41 },
          { agent: "cursor", lastCompletedAt: null, sessionsSeen: 12 },
        ],
      },
    })
    render(<OnboardingView />)

    fireEvent.click(await screen.findByRole("button", { name: "Continue" }))
    await screen.findByRole("heading", { name: "Scan Locations: Agents" })

    // Detected agents lead with their evidence and start switched on.
    expect(screen.getByText("304 sessions")).toBeInTheDocument()
    expect(screen.getByRole("switch", { name: "Show Claude Code sessions" })).toBeChecked()
    // An agent with no sessions starts switched off.
    expect(screen.getByRole("switch", { name: "Show Windsurf sessions" })).not.toBeChecked()

    fireEvent.click(screen.getByRole("switch", { name: "Show Codex sessions" }))

    fireEvent.click(screen.getByRole("button", { name: "Continue" }))
    await screen.findByRole("heading", { name: "Scan Locations: Repos" })
    fireEvent.click(screen.getByRole("button", { name: "Continue" }))
    await screen.findByRole("heading", { name: "Ready" })
    fireEvent.click(screen.getByRole("button", { name: "Start using antiburn" }))

    await waitFor(() =>
      expect(invoke.mock.calls.some(([command]) => command === "finish_onboarding")).toBe(true),
    )
    const call = invoke.mock.calls.find(([command]) => command === "finish_onboarding")
    const disabled = (call?.[1] as { disabledAgents: string[] }).disabledAgents
    expect(disabled).toContain("codex")
    expect(disabled).toContain("windsurf")
    expect(disabled).not.toContain("claude-code")
    expect(disabled).not.toContain("cursor")
  })

  it("keeps analytics and privacy copy off the Ready step", async () => {
    // The v5 redesign moved this disclosure out of onboarding. Settings →
    // Privacy still carries the switch and the copy.
    render(<OnboardingView />)
    await advanceToReady()

    expect(screen.queryByRole("switch", { name: /analytics/i })).not.toBeInTheDocument()
    expect(screen.queryByText(/Never prompts, sessions/i)).not.toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "Copy prompt" })).not.toBeInTheDocument()
  })

  it("records each onboarding step once", async () => {
    render(<OnboardingView />)
    await advanceToReady()
    fireEvent.click(screen.getByRole("button", { name: "Back" }))
    await screen.findByRole("heading", { name: "Scan Locations: Repos" })

    for (const step of ["welcome", "agents_detected", "sources_and_repos", "ready"]) {
      expect(invoke).toHaveBeenCalledWith("note_interaction", {
        interaction: { kind: "onboardingStepViewed", step },
      })
    }
    expect(
      invoke.mock.calls.filter(([command]) => command === "note_interaction"),
    ).toHaveLength(4)
  })

  it("skips step analytics for a build without analytics", async () => {
    mockCommands({ app_info: { ...APP_INFO, analyticsSupported: false } })
    render(<OnboardingView />)

    await advanceToReady()

    expect(
      invoke.mock.calls.filter(([command]) => command === "note_interaction"),
    ).toHaveLength(0)
  })

  it("skips step analytics when the environment disables analytics", async () => {
    mockCommands({ app_info: { ...APP_INFO, analyticsEnvironmentDisabled: true } })
    render(<OnboardingView />)

    await advanceToReady()

    expect(
      invoke.mock.calls.filter(([command]) => command === "note_interaction"),
    ).toHaveLength(0)
  })

  it("shows analysis progress while sessions are still settling", async () => {
    mockCommands({
      get_hygiene_summary: { ...HYGIENE_SUMMARY, settledSessions: 2, analyzedSessions: 2 },
    })
    render(<OnboardingView />)
    await advanceToReady()

    expect(await screen.findByText("Analyzing sessions")).toBeInTheDocument()
    expect(screen.getByText("2 of 4")).toBeInTheDocument()
    expect(screen.queryByText("sessions analyzed")).not.toBeInTheDocument()
  })

  it("persists the startup and Do Not Disturb drafts before finishing onboarding", async () => {
    render(<OnboardingView />)
    await advanceToReady()

    const launchAtLogin = screen.getByRole("switch", { name: "Launch antiburn on startup" })
    expect(launchAtLogin).toBeChecked()
    fireEvent.click(launchAtLogin)
    fireEvent.click(screen.getByRole("switch", { name: "Nudges respect Do Not Disturb" }))
    // The choices are drafts until Finish, so there is no earlier settings
    // round-trip for these clicks to race.
    fireEvent.click(screen.getByRole("button", { name: "Start using antiburn" }))

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("finish_onboarding", {
        activityWindowDays: 7,
        launchAtLogin: false,
        disabledAgents: [],
        nudgesRespectDnd: true,
      }),
    )
  })

  it("keeps onboarding open with an actionable error when finishing fails", async () => {
    const commands = invoke.getMockImplementation()
    invoke.mockImplementation((command: string, args?: unknown) =>
      command === "finish_onboarding"
        ? Promise.reject(new Error("store unavailable"))
        : commands?.(command, args),
    )
    render(<OnboardingView />)
    await advanceToReady()

    const finish = screen.getByRole("button", { name: "Start using antiburn" })
    fireEvent.click(finish)

    expect(await screen.findByRole("alert")).toHaveTextContent("store unavailable")
    expect(screen.getByRole("button", { name: "Start using antiburn" })).toBeEnabled()
  })

  it("submits the finish transition only once", async () => {
    let resolveFinish!: (settings: typeof SETTINGS) => void
    const pendingFinish = new Promise<typeof SETTINGS>((resolve) => {
      resolveFinish = resolve
    })
    mockCommands({ finish_onboarding: pendingFinish })
    render(<OnboardingView />)
    await advanceToReady()

    fireEvent.click(screen.getByRole("button", { name: "Start using antiburn" }))
    const finishing = screen.getByRole("button", { name: "Finishing…" })
    expect(finishing).toBeDisabled()
    fireEvent.click(finishing)

    expect(
      invoke.mock.calls.filter(([command]) => command === "finish_onboarding"),
    ).toHaveLength(1)
    resolveFinish({ ...SETTINGS, onboardingCompleted: true })
  })

  it("announces each step and moves focus to its heading", async () => {
    render(<OnboardingView />)

    const welcome = await screen.findByRole("heading", {
      name: "Stop hitting your token limits.",
    })
    await waitFor(() => expect(welcome).toHaveFocus())
    expect(screen.getByText("Step 1 of 4")).toBeInTheDocument()

    fireEvent.click(screen.getByRole("button", { name: "Continue" }))

    const agents = await screen.findByRole("heading", {
      name: "Scan Locations: Agents",
    })
    await waitFor(() => expect(agents).toHaveFocus())
    expect(screen.getByText("Step 2 of 4")).toBeInTheDocument()

    fireEvent.click(screen.getByRole("button", { name: "Continue" }))

    const locations = await screen.findByRole("heading", { name: "Scan Locations: Repos" })
    await waitFor(() => expect(locations).toHaveFocus())
    expect(screen.getByText("Step 3 of 4")).toBeInTheDocument()
  })

  it("queues a follow-up scan when a folder is added during discovery", async () => {
    let resolveScan!: (status: typeof SCAN_STATUS) => void
    const pendingScan = new Promise<typeof SCAN_STATUS>((resolve) => {
      resolveScan = resolve
    })
    mockCommands({ scan_now: pendingScan })
    openDialog.mockResolvedValue("/home/avery/work")
    render(<OnboardingView />)

    await advanceToSources()
    await waitFor(() =>
      expect(invoke.mock.calls.filter(([command]) => command === "scan_now")).toHaveLength(1),
    )
    fireEvent.click(await screen.findByRole("button", { name: /Add Locations/ }))
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("add_scan_root", expect.anything()))

    resolveScan(SCAN_STATUS)

    await waitFor(() =>
      expect(invoke.mock.calls.filter(([command]) => command === "scan_now")).toHaveLength(2),
    )
  })

  it("shows a failed scan instead of an empty result and allows a retry", async () => {
    let attempts = 0
    mockCommands({
      scan_now: () => {
        attempts += 1
        return attempts === 1 ? FAILED_SCAN_STATUS : SCAN_STATUS
      },
    })
    render(<OnboardingView />)

    await advanceToSources()

    expect(await screen.findByRole("alert")).toHaveTextContent("Scan did not finish")
    expect(screen.getByRole("alert")).toHaveTextContent("Could not read ~/.claude/projects.")
    expect(screen.queryByText("Nothing found yet")).not.toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Continue" })).toBeEnabled()

    fireEvent.click(screen.getByRole("button", { name: "Try again" }))

    await waitFor(() => expect(attempts).toBe(2))
    await waitFor(() => expect(screen.queryByRole("alert")).not.toBeInTheDocument())
  })

  it("keeps repository rows after failure and hides retry while scanning", async () => {
    let attempts = 0
    let resolveRetry!: (status: typeof SCAN_STATUS) => void
    const pendingRetry = new Promise<typeof SCAN_STATUS>((resolve) => {
      resolveRetry = resolve
    })
    mockCommands({
      list_repositories: [REPOSITORY],
      scan_now: () => {
        attempts += 1
        return attempts === 1 ? FAILED_SCAN_STATUS : pendingRetry
      },
    })
    render(<OnboardingView />)

    await advanceToSources()
    expect(await screen.findByRole("alert")).toHaveTextContent("Scan did not finish")
    expect(screen.getByText("avery/widgets")).toBeInTheDocument()

    fireEvent.click(screen.getByRole("button", { name: "Try again" }))
    await waitFor(() => expect(attempts).toBe(2))
    act(() => {
      for (const notify of listeners.get("scan:started") ?? []) {
        notify({ payload: { ...FAILED_SCAN_STATUS, running: true, error: null } })
      }
    })

    expect(screen.queryByRole("button", { name: "Try again" })).not.toBeInTheDocument()
    expect(screen.getByText("avery/widgets")).toBeInTheDocument()

    resolveRetry(SCAN_STATUS)
    await waitFor(() => expect(screen.queryByRole("alert")).not.toBeInTheDocument())
  })

  it("rescans after a folder is removed", async () => {
    mockCommands({ list_scan_roots: ["/home/avery/work"] })
    render(<OnboardingView />)

    await advanceToSources()
    await waitFor(() =>
      expect(invoke.mock.calls.filter(([command]) => command === "scan_now")).toHaveLength(1),
    )

    fireEvent.click(screen.getByRole("button", { name: "Stop scanning /home/avery/work" }))

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("remove_scan_root", expect.anything()),
    )
    await waitFor(() =>
      expect(invoke.mock.calls.filter(([command]) => command === "scan_now")).toHaveLength(2),
    )
  })

  it("opens the folder picker without holding the popover open", async () => {
    // The hold exists because the popover hides when it loses focus. This is a
    // decorated window, so asking for it would be asking the shell to guard a
    // window that needs no guarding.
    openDialog.mockResolvedValue("/home/avery/work")
    render(<OnboardingView />)

    await advanceToSources()
    fireEvent.click(await screen.findByRole("button", { name: /Add Locations/ }))

    await waitFor(() => expect(invoke).toHaveBeenCalledWith("add_scan_root", expect.anything()))
    expect(invoke).not.toHaveBeenCalledWith("begin_popover_hold")
    expect(invoke).not.toHaveBeenCalledWith("end_popover_hold")
  })

  it("does not close the window on Escape", async () => {
    // Escape dismissed the popover, which was right for a transient tray
    // surface and wrong for a decorated window in the middle of a task.
    render(<OnboardingView />)
    await screen.findByRole("heading", { name: "Stop hitting your token limits." })

    fireEvent.keyDown(document, { key: "Escape" })

    expect(invoke).not.toHaveBeenCalledWith("hide_popover")
    expect(
      screen.getByRole("heading", { name: "Stop hitting your token limits." }),
    ).toBeInTheDocument()
  })
})
