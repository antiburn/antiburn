import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import { OnboardingView } from "./OnboardingView"

/**
 * The first-run window, driven entirely through the mocked command layer.
 *
 * These moved here from `PopoverView.test.tsx` with the standalone flow. They
 * assert the same things they always did — every step
 * appears, each is announced and takes focus, and the two settings writes go
 * out with the arguments the shell expects — because none of that changed;
 * only which window it happens in.
 */

const invoke = vi.hoisted(() => vi.fn())
const openDialog = vi.hoisted(() => vi.fn())
const clipboardWrite = vi.hoisted(() => vi.fn())
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
  onboardingCompleted: false,
  launchAtLogin: true,
  autoUpdate: true,
  discoveryPaused: false,
  analyticsEnabled: true,
}

/// An analytics-capable official build. Source builds use the unsupported case.
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

function mockCommands(overrides: Record<string, unknown> = {}) {
  invoke.mockImplementation((command: string, args?: unknown) => {
    if (command in overrides) return Promise.resolve(overrides[command])
    switch (command) {
      case "get_settings":
        return Promise.resolve(SETTINGS)
      case "app_info":
        return Promise.resolve(APP_INFO)
      case "scan_now":
      case "cancel_scan":
        return Promise.resolve(SCAN_STATUS)
      // A fresh install: nothing has been scanned, so the repository step has
      // to ask for a pass before it has anything to show.
      case "get_scan_status":
        return Promise.resolve(null)
      case "set_settings":
        return Promise.resolve((args as Record<string, unknown> | undefined)?.["settings"])
      case "finish_onboarding":
        return Promise.resolve({ ...SETTINGS, onboardingCompleted: true })
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
  for (let step = 0; step < 4; step += 1) {
    fireEvent.click(screen.getByRole("button", { name: "Continue" }))
  }
  await screen.findByRole("heading", { name: "Ready" })
}

describe("OnboardingView", () => {
  beforeEach(() => {
    invoke.mockReset()
    openDialog.mockReset()
    listeners.clear()
    clipboardWrite.mockReset()
    clipboardWrite.mockResolvedValue(undefined)
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText: clipboardWrite },
    })
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

  it("runs the five-step first-run flow and records that it finished", async () => {
    mockCommands({ default_scan_roots: ["/home/avery/code"] })
    render(<OnboardingView />)

    // 1 — Welcome. No account, and nothing of the reader's work leaving.
    expect(
      await screen.findByRole("heading", { name: "Stop hitting your token limits." }),
    ).toBeInTheDocument()
    expect(screen.getByText(/nothing from your sessions is ever uploaded/i)).toBeInTheDocument()
    // Welcome names the permanent opt-out before setup continues.
    expect(
      screen.getByText(/only goes online for.*You can opt out of the analytics/i),
    ).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Continue" }))

    // 2 — Sources. The engine's own default roots are listed, so the reader can
    // see the common cases are already covered.
    expect(await screen.findByRole("heading", { name: "Where to look" })).toBeInTheDocument()
    expect(screen.getByText("/home/avery/code")).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Continue" }))

    // 3 — Repositories, which needs a discovery pass to have something to show.
    expect(await screen.findByRole("heading", { name: "What to include" })).toBeInTheDocument()
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("scan_now", { activityWindowDays: 7 }),
    )
    fireEvent.click(screen.getByRole("button", { name: "Continue" }))

    // 4 — Historical scan: the window choice, and the pass with a way out.
    expect(await screen.findByRole("heading", { name: "Historical scan" })).toBeInTheDocument()
    fireEvent.click(screen.getByRole("radio", { name: "14 days" }))
    expect(invoke).not.toHaveBeenCalledWith("set_settings", expect.anything())
    fireEvent.click(screen.getByRole("button", { name: "Continue" }))

    // 5 — Ready.
    expect(await screen.findByRole("heading", { name: "Ready" })).toBeInTheDocument()
    expect(screen.getByText(/repositories are never modified/i)).toBeInTheDocument()
    expect(screen.getByRole("switch", { name: "Launch antiburn on startup" })).toBeChecked()
    expect(
      screen.getByText(/Never prompts, sessions, source code, filenames, or paths/i),
    ).toBeInTheDocument()
    expect(screen.getByText(/check what leaves your computer/i)).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Copy prompt" })).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Start using antiburn" }))

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("finish_onboarding", {
        activityWindowDays: 14,
        launchAtLogin: true,
      }),
    )
  })

  it("records each fixed onboarding step once", async () => {
    render(<OnboardingView />)
    await advanceToReady()

    for (const step of ["welcome", "sources", "repositories", "historical_scan", "ready"]) {
      expect(invoke).toHaveBeenCalledWith("note_interaction", {
        interaction: { kind: "onboardingStepViewed", step },
      })
    }
    expect(
      invoke.mock.calls.filter(([command]) => command === "note_interaction"),
    ).toHaveLength(5)
  })

  it("omits analytics disclosure from a build without analytics", async () => {
    mockCommands({ app_info: { ...APP_INFO, analyticsSupported: false } })
    render(<OnboardingView />)

    expect(
      await screen.findByRole("heading", { name: "Stop hitting your token limits." }),
    ).toBeInTheDocument()

    await advanceToReady()

    expect(screen.queryByText(/Never prompts, sessions/i)).not.toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "Copy prompt" })).not.toBeInTheDocument()
  })

  it("explains the process override without hiding analytics support", async () => {
    mockCommands({
      app_info: { ...APP_INFO, analyticsEnvironmentDisabled: true },
    })
    render(<OnboardingView />)

    expect(
      await screen.findByText(/Analytics is disabled for this launch by/i),
    ).toHaveTextContent("ANTIBURN_ANALYTICS_ENABLED=false")
    expect(screen.queryByText(/This build has no analytics endpoint/i)).not.toBeInTheDocument()

    await advanceToReady()

    expect(screen.getByText(/Off for this launch/i)).toHaveTextContent(
      "ANTIBURN_ANALYTICS_ENABLED=false. Remove it to use the setting in Settings → Privacy.",
    )
    expect(
      invoke.mock.calls.filter(([command]) => command === "note_interaction"),
    ).toHaveLength(0)
  })

  it("copies the optional privacy prompt for an AI agent", async () => {
    render(<OnboardingView />)
    await advanceToReady()

    fireEvent.click(screen.getByRole("button", { name: "Copy prompt" }))

    expect(await screen.findByRole("button", { name: "Copied" })).toBeInTheDocument()
    expect(clipboardWrite).toHaveBeenCalledWith(
      expect.stringContaining("Explain in plain language what stays on my computer"),
    )
    await waitFor(
      () => expect(screen.getByRole("button", { name: "Copy prompt" })).toBeInTheDocument(),
      { timeout: 3_000 },
    )
  })

  it("persists an opt-out before finishing onboarding", async () => {
    render(<OnboardingView />)
    await advanceToReady()

    const launchAtLogin = screen.getByRole("switch", { name: "Launch antiburn on startup" })
    expect(launchAtLogin).toBeChecked()
    fireEvent.click(launchAtLogin)
    // The choice is a draft until Finish, so there is no earlier settings
    // round-trip for this click to race.
    fireEvent.click(screen.getByRole("button", { name: "Start using antiburn" }))

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("finish_onboarding", {
        activityWindowDays: 7,
        launchAtLogin: false,
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
    expect(screen.getByText("Step 1 of 5")).toBeInTheDocument()

    fireEvent.click(screen.getByRole("button", { name: "Continue" }))

    const sources = await screen.findByRole("heading", { name: "Where to look" })
    await waitFor(() => expect(sources).toHaveFocus())
    expect(screen.getByText("Step 2 of 5")).toBeInTheDocument()
  })

  it("opens the folder picker without holding the popover open", async () => {
    // The hold exists because the popover hides when it loses focus. This is a
    // decorated window, so asking for it would be asking the shell to guard a
    // window that needs no guarding.
    openDialog.mockResolvedValue("/home/avery/work")
    render(<OnboardingView />)

    fireEvent.click(await screen.findByRole("button", { name: "Continue" }))
    fireEvent.click(await screen.findByRole("button", { name: /Add a folder/ }))

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
