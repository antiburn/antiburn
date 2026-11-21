import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { appInfoStore } from "../lib/appInfoStore"
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
        return { ...emptyCloudSnapshot, authEnabled, status: authEnabled ? "offline" : "idle" }
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
  appInfoStore.set(null)
  vi.stubGlobal("matchMedia", () => ({
    matches: true,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
  }))
})
afterEach(() => vi.unstubAllGlobals())

it.each(["checks", "cloud", "cloudUnavailable"] as const)(
  "keeps ordinary builds local when resuming %s",
  async (step) => {
    commands(false, step)
    render(<DesktopOnboardingEntry />)
    const done = await screen.findByRole("button", { name: "Done" })
    await waitFor(() => expect(done).toBeEnabled())
    fireEvent.click(done)
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("set_onboarding_state", { state: "completed" }),
    )
    expect(screen.queryByRole("button", { name: "Set up Cloud" })).not.toBeInTheDocument()
    expect(invoke.mock.calls.some(([command]) => command.startsWith("cloud_"))).toBe(false)
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
      expect(screen.getByRole("button", { name: "Connect in browser" })).toBeEnabled()
    } else {
      expect(
        await screen.findByRole("heading", { name: "Antiburn Cloud is coming soon" }),
      ).toBeVisible()
      await waitFor(() =>
        expect(screen.getByRole("button", { name: "Learn more" })).toBeEnabled(),
      )
    }
    fireEvent.click(
      screen.getByRole("button", { name: configured ? "Continue locally" : "Done" }),
    )
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("set_onboarding_state", { state: "completed" }),
    )
  },
)

it("enables Cloud setup only after native build information confirms the feature", async () => {
  commands(true)
  let resolve!: (value: AppInfo) => void
  const native = invoke.getMockImplementation()!
  invoke.mockImplementation((command: string, args?: Record<string, unknown>) =>
    command === "app_info"
      ? new Promise<AppInfo>((done) => {
          resolve = done
        })
      : native(command, args),
  )
  render(<DesktopOnboardingEntry />)
  const done = await screen.findByRole("button", { name: "Done" })
  await waitFor(() => expect(done).toBeEnabled())
  expect(invoke.mock.calls.some(([command]) => command.startsWith("cloud_"))).toBe(false)
  await act(async () => resolve({ ...info, cloudEnabled: true }))
  expect(screen.getByRole("button", { name: "Continue" })).toBeEnabled()
  expect(invoke).not.toHaveBeenCalledWith("set_onboarding_state", { state: "completed" })
})
