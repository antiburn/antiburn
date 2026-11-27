import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { StrictMode } from "react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import onboardingCapability from "../../src-tauri/capabilities/onboarding.json"
import { emptyCloudSnapshot } from "../lib/cloud"
import type { AppInfo, OnboardingState } from "../lib/ipc"
import { DesktopOnboardingEntry } from "./DesktopOnboardingEntry"

const invoke = vi.hoisted(() => vi.fn())
vi.mock("@tauri-apps/api/core", () => ({ invoke, isTauri: () => true }))
vi.mock("@tauri-apps/api/event", () => ({ listen: async () => () => {} }))

const info: AppInfo = {
  appVersion: "0.1.0",
  cloudEnabled: false,
  debugBuild: false,
  arch: "aarch64",
  pricingCatalogVersion: "synthetic",
  schemaVersion: 1,
  dataDir: "/Example/State",
  indexedSessions: 0,
  databaseBytes: 0,
  updatesSupported: false,
  analyticsSupported: false,
  analyticsEnvironmentDisabled: false,
  analyticsOperator: null,
}

function commands(
  cloudEnabled: boolean,
  initial: OnboardingState = "checks",
  authEnabled = true,
) {
  let state = initial
  invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
    switch (command) {
      case "app_info":
        return { ...info, cloudEnabled }
      case "get_onboarding_state":
        return state
      case "set_onboarding_state":
        state = args?.state as OnboardingState
        return state
      case "get_onboarding_window_visible":
        return true
      case "hud_island_available":
        return false
      case "cloud_status":
      case "cloud_begin":
        return { ...emptyCloudSnapshot, authEnabled, status: authEnabled ? "offline" : "idle" }
      case "plugin:opener|open_url":
        return null
      case "get_agent_integrations":
        return []
      case "note_interaction":
        return null
      default:
        throw new Error(`Unexpected synthetic command: ${command}`)
    }
  })
}

beforeEach(() => {
  vi.clearAllMocks()
  vi.stubGlobal("matchMedia", () => ({
    matches: true,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
  }))
})
afterEach(() => vi.unstubAllGlobals())

it.each(["cloud", "cloudUnavailable"] as const)(
  "resumes the public Cloud screen from %s and finishes without an account",
  async (step) => {
    commands(false, step)
    render(<DesktopOnboardingEntry />)
    expect(
      await screen.findByRole("heading", { name: "Antiburn Cloud is coming soon" }),
    ).toBeVisible()
    expect(
      screen.queryByRole("button", { name: "Sign in with GitHub" }),
    ).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Done" }))
    expect(await screen.findByRole("button", { name: "Open Sessions" })).toBeVisible()
    expect(invoke).toHaveBeenCalledWith("set_onboarding_state", { state: "completed" })
    expect(invoke).toHaveBeenCalledWith("note_interaction", {
      interaction: {
        kind: "onboardingActionTaken",
        screen: step,
        action: step === "cloudUnavailable" ? "done" : "use_locally",
      },
    })
    expect(invoke.mock.calls.some(([command]) => command.startsWith("cloud_"))).toBe(false)
  },
)

it("records Back when resuming the legacy waitlist step", async () => {
  commands(false, "cloudUnavailable")
  render(<DesktopOnboardingEntry />)
  fireEvent.click(await screen.findByRole("button", { name: "Back" }))
  expect(
    await screen.findByRole("heading", { name: "See what you could improve" }),
  ).toBeVisible()
  expect(invoke).toHaveBeenCalledWith("note_interaction", {
    interaction: { kind: "onboardingActionTaken", screen: "cloudUnavailable", action: "back" },
  })
})

it("opens the public page from Checks and lets the user return or finish", async () => {
  commands(false)
  render(<DesktopOnboardingEntry />)
  fireEvent.click(await screen.findByRole("button", { name: "Continue" }))
  await screen.findByRole("heading", { name: "Antiburn Cloud is coming soon" })
  fireEvent.click(screen.getByRole("button", { name: "Back" }))
  fireEvent.click(await screen.findByRole("button", { name: "Continue" }))

  fireEvent.click(await screen.findByRole("button", { name: "Learn more" }))
  expect(invoke).toHaveBeenCalledWith("plugin:opener|open_url", {
    url: "https://antiburn.com/cloud",
  })
  const [, openerArgs] = invoke.mock.calls.find(
    ([command]) => command === "plugin:opener|open_url",
  )!
  expect(onboardingCapability.permissions).toContainEqual({
    identifier: "opener:allow-open-url",
    allow: [{ url: openerArgs.url }],
  })
  expect(
    invoke.mock.calls.filter(
      ([command, args]) =>
        command === "note_interaction" && args?.interaction?.action === "learn_more",
    ),
  ).toEqual([
    [
      "note_interaction",
      { interaction: { kind: "onboardingActionTaken", screen: "cloud", action: "learn_more" } },
    ],
  ])
  expect(invoke).not.toHaveBeenCalledWith("set_onboarding_state", { state: "completed" })
  expect(
    await screen.findByRole("heading", { name: "Antiburn Cloud is coming soon" }),
  ).toBeVisible()
  fireEvent.click(screen.getByRole("button", { name: "Done" }))
  expect(await screen.findByRole("button", { name: "Open Sessions" })).toBeVisible()
  expect(invoke.mock.calls.some(([command]) => command.startsWith("cloud_"))).toBe(false)
})

it.each([
  ["succeeds", "Could not finish the introduction"],
  ["fails", "Could not open the Cloud page"],
])(
  "keeps both actions available after a save failure when opening the page %s",
  async (outcome, message) => {
    commands(false, "cloud")
    const native = invoke.getMockImplementation()!
    let failSave = true
    invoke.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
      if (command === "plugin:opener|open_url" && outcome === "fails") {
        throw new Error("No system browser")
      }
      if (command === "set_onboarding_state" && failSave) {
        failSave = false
        throw new Error("Failed to save onboarding")
      }
      return native(command, args)
    })
    render(<DesktopOnboardingEntry />)
    fireEvent.click(await screen.findByRole("button", { name: "Done" }))
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Could not finish the introduction",
    )
    const learnMore = await screen.findByRole("button", { name: "Learn more" })
    fireEvent.click(learnMore)
    expect(invoke).toHaveBeenCalledWith("plugin:opener|open_url", {
      url: "https://antiburn.com/cloud",
    })
    await waitFor(() => expect(learnMore).toBeEnabled())
    expect(screen.getByRole("alert")).toHaveTextContent(message)
    fireEvent.click(screen.getByRole("button", { name: "Done" }))
    expect(await screen.findByRole("button", { name: "Open Sessions" })).toBeVisible()
  },
)

it.each([true, false])(
  "keeps the feature-enabled Cloud flow reachable with service configuration: %s",
  async (configured) => {
    commands(true, "checks", configured)
    render(<DesktopOnboardingEntry />)
    const next = await screen.findByRole("button", { name: "Continue" })
    await waitFor(() => expect(next).toBeEnabled())
    fireEvent.click(next)
    fireEvent.click(await screen.findByRole("button", { name: "Set up Cloud" }))
    expect(await screen.findByRole("main", { name: "Antiburn Cloud setup" })).toBeVisible()
    if (configured) {
      expect(
        await screen.findByText(
          "Cloud could not connect. Check your connection and try again.",
        ),
      ).toBeVisible()
      fireEvent.click(screen.getByRole("button", { name: "Sign in with GitHub" }))
    } else {
      expect(
        await screen.findByRole("heading", { name: "Antiburn Cloud is coming soon" }),
      ).toBeVisible()
      const learnMore = screen.getByRole("button", { name: "Learn more" })
      await waitFor(() => expect(learnMore).toBeEnabled())
      fireEvent.click(learnMore)
    }
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("cloud_begin"))
    const finish = screen.getByRole("button", {
      name: configured ? "Continue locally" : "Done",
    })
    await waitFor(() => expect(finish).toBeEnabled())
    fireEvent.click(finish)
    expect(await screen.findByRole("button", { name: "Open Sessions" })).toBeVisible()
    expect(invoke).toHaveBeenCalledWith("set_onboarding_state", { state: "completed" })
    expect(invoke).not.toHaveBeenCalledWith("cloud_enable_sync", expect.anything())
  },
)

it.each(["enabled", "disabled", "failed"] as const)(
  "waits to show the Cloud screen when the delayed build lookup is %s",
  async (outcome) => {
    commands(false)
    let resolveInfo!: (value: AppInfo) => void
    let rejectInfo!: (reason: Error) => void
    const pendingInfo = new Promise<AppInfo>((resolve, reject) => {
      resolveInfo = resolve
      rejectInfo = reject
    })
    const native = invoke.getMockImplementation()!
    invoke.mockImplementation((command: string, args?: Record<string, unknown>) =>
      command === "app_info" ? pendingInfo : native(command, args),
    )
    render(
      <StrictMode>
        <DesktopOnboardingEntry />
      </StrictMode>,
    )

    const next = await screen.findByRole("button", { name: "Continue" })
    expect(next).toBeEnabled()
    fireEvent.click(next)
    expect(await screen.findByRole("status")).toHaveTextContent("Checking Cloud availability")
    expect(screen.queryByRole("button", { name: "Learn more" })).not.toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "Done" })).not.toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "Set up Cloud" })).not.toBeInTheDocument()
    expect(invoke.mock.calls.filter(([command]) => command === "set_onboarding_state")).toEqual(
      [["set_onboarding_state", { state: "cloud" }]],
    )

    await act(async () => {
      if (outcome === "failed") rejectInfo(new Error("App info unavailable"))
      else resolveInfo({ ...info, cloudEnabled: outcome === "enabled" })
    })
    expect(screen.queryByText("Checking Cloud availability…")).not.toBeInTheDocument()
    if (outcome === "enabled") {
      expect(screen.getByRole("button", { name: "Set up Cloud" })).toBeEnabled()
      expect(screen.queryByRole("button", { name: "Learn more" })).not.toBeInTheDocument()
    } else {
      expect(screen.queryByRole("button", { name: "Set up Cloud" })).not.toBeInTheDocument()
      fireEvent.click(screen.getByRole("button", { name: "Learn more" }))
      expect(invoke).toHaveBeenCalledWith("plugin:opener|open_url", {
        url: "https://antiburn.com/cloud",
      })
      fireEvent.click(screen.getByRole("button", { name: "Done" }))
      expect(await screen.findByRole("button", { name: "Open Sessions" })).toBeVisible()
    }
    expect(invoke.mock.calls.filter(([command]) => command === "app_info")).toHaveLength(1)
    expect(invoke.mock.calls.some(([command]) => command.startsWith("cloud_"))).toBe(false)
  },
)

it.each(["stalled", "late true", "enabled before Cloud"] as const)(
  "starts the lookup deadline at the Cloud step (%s)",
  async (outcome) => {
    vi.useFakeTimers()
    try {
      commands(true, outcome === "late true" ? "cloud" : "checks")
      let resolveInfo!: (value: AppInfo) => void
      const pendingInfo = new Promise<AppInfo>((resolve) => {
        resolveInfo = resolve
      })
      const native = invoke.getMockImplementation()!
      invoke.mockImplementation((command: string, args?: Record<string, unknown>) =>
        command === "app_info" ? pendingInfo : native(command, args),
      )
      render(<DesktopOnboardingEntry />)

      if (outcome !== "late true") {
        await act(() => vi.advanceTimersByTimeAsync(6_000))
        if (outcome === "enabled before Cloud") {
          await act(async () => resolveInfo({ ...info, cloudEnabled: true }))
        }
        await act(async () => {
          fireEvent.click(screen.getByRole("button", { name: "Continue" }))
        })
        if (outcome === "enabled before Cloud") {
          expect(screen.getByRole("button", { name: "Set up Cloud" })).toBeEnabled()
          expect(invoke.mock.calls.filter(([command]) => command === "app_info")).toHaveLength(
            1,
          )
          return
        }
      }

      await act(() => vi.advanceTimersByTimeAsync(4_999))
      expect(screen.getByRole("status")).toHaveTextContent("Checking Cloud availability")
      expect(screen.queryByRole("button", { name: "Done" })).not.toBeInTheDocument()

      await act(() => vi.advanceTimersByTimeAsync(1))
      expect(screen.getByRole("button", { name: "Learn more" })).toBeEnabled()
      const done = screen.getByRole("button", { name: "Done" })
      expect(done).toBeEnabled()
      done.focus()

      if (outcome === "late true") {
        await act(async () => resolveInfo({ ...info, cloudEnabled: true }))
        expect(screen.queryByRole("button", { name: "Set up Cloud" })).not.toBeInTheDocument()
        expect(done).toHaveFocus()
      }

      await act(async () => {
        fireEvent.click(done)
      })
      expect(screen.getByRole("button", { name: "Open Sessions" })).toBeVisible()
      expect(invoke.mock.calls.filter(([command]) => command === "app_info")).toHaveLength(1)
    } finally {
      vi.useRealTimers()
    }
  },
)
