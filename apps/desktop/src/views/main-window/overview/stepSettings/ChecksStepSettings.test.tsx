import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, expect, it, vi } from "vitest"

import { defaultConnection } from "../../../../lib/smartCheckProviders"
import { searchApp } from "../../../../lib/appSearch"
import { ChecksStepSettings } from "./ChecksStepSettings"
import type { CheckAvailabilityEvent } from "../../../../lib/checkAvailability"
import { deferred } from "../../burn-checks/tests/burnChecksTestSupport"

const getAvailability = vi.hoisted(() => vi.fn())
const providerInvoke = vi.hoisted(() => vi.fn())
const setChecksEnabled = vi.hoisted(() => vi.fn())
const setCheckEnabled = vi.hoisted(() => vi.fn())
const availabilityEvent = vi.hoisted(() => ({
  callback: null as ((event: CheckAvailabilityEvent) => void) | null,
}))
const emptyAvailability = vi.hoisted(() => ({
  revision: 1,
  yamlEnabled: false,
  checks: [
    { id: "sessionsOverDepth" as const, enabled: true },
    { id: "modelOverthinking" as const, enabled: true },
    { id: "overpoweredSubagents" as const, enabled: true },
    { id: "unusedMcpServers" as const, enabled: true },
    { id: "unusedBuiltInTools" as const, enabled: true },
    { id: "unusedSkills" as const, enabled: true },
    { id: "oldModelUsage" as const, enabled: true },
    { id: "overuseOfFastMode" as const, enabled: true },
    { id: "cacheChurn" as const, enabled: true },
    { id: "ignoredInstructions" as const, enabled: false },
    { id: "skillOpportunities" as const, enabled: false },
    { id: "overExploring" as const, enabled: false },
    { id: "scopeCreep" as const, enabled: false },
  ],
  configured: false,
  savedKey: false,
  error: null,
  historyDays: 0 as const,
  backfill: {
    total: 0,
    waitingForData: 0,
    waitingForIdle: 0,
    ready: 0,
    queued: 0,
    running: 0,
    completed: 0,
    skipped: 0,
    failed: 0,
    reviewed: 0,
    eligibleItems: 0,
  },
  usage: {
    inputTokens: 0,
    outputTokens: 0,
    confirmedCalls: 0,
    cacheHits: 0,
    unknownOutcomes: 0,
    estimatedUsd: "$0.00",
    lastUsedAtEpoch: null,
  },
}))

vi.mock("../../../../lib/checkAvailability", () => ({
  getCheckAvailability: getAvailability,
  removeTypeSafeApiKey: vi.fn(),
  setSmartBurnChecksEnabled: setChecksEnabled,
  setCheckEnabled,
  onCheckAvailabilityChanged: vi.fn(
    async (callback: (event: CheckAvailabilityEvent) => void) => {
      availabilityEvent.callback = callback
      return () => undefined
    },
  ),
  emptyCheckAvailability: emptyAvailability,
}))
vi.mock("@tauri-apps/api/core", () => ({ invoke: providerInvoke }))

beforeEach(() => {
  getAvailability.mockReset().mockResolvedValue(emptyAvailability)
  setChecksEnabled.mockReset().mockImplementation(async (enabled: boolean) => ({
    ...emptyAvailability,
    yamlEnabled: enabled,
  }))
  setCheckEnabled
    .mockReset()
    .mockImplementation(async (detector: string, enabled: boolean) => ({
      ...emptyAvailability,
      checks: emptyAvailability.checks.map((check) =>
        check.id === detector ? { ...check, enabled } : check,
      ),
    }))
  providerInvoke.mockReset().mockImplementation(async (command: string, args) => {
    if (command === "get_system_one_settings")
      return { activeId: "jev", profiles: { jev: defaultConnection("jev") } }
    if (command === "test_system_one_connection") return null
    if (command === "save_system_one_connection")
      return {
        activeId: "jev",
        profiles: {
          jev: { ...args.draft.connection, credential: { kind: "connection", id: "jev" } },
        },
      }
    return null
  })
})

it("shows local and model-based checks with a master discovery toggle", async () => {
  render(<ChecksStepSettings />)
  expect(screen.getByRole("heading", { name: "Local Checks" })).toBeInTheDocument()
  expect(screen.getByRole("heading", { name: "Model-based Checks" })).toBeInTheDocument()
  expect(screen.getAllByRole("switch")).toHaveLength(10)
  const master = screen.getByRole("switch", { name: "Model-based Checks" })
  expect(master).not.toBeChecked()
  fireEvent.click(master)
  await waitFor(() => expect(setChecksEnabled).toHaveBeenCalledWith(true))
  expect(master).toBeChecked()
  expect(screen.queryByRole("switch", { name: "Ignored instructions" })).not.toBeInTheDocument()
  expect(screen.queryByRole("heading", { name: "Past sessions" })).not.toBeInTheDocument()
})

it("updates a single check without changing its siblings", async () => {
  render(<ChecksStepSettings />)
  fireEvent.click(screen.getByRole("switch", { name: "Unused skills" }))
  await waitFor(() => expect(setCheckEnabled).toHaveBeenCalledWith("unusedSkills", false))
  expect(screen.getByRole("switch", { name: "Unused skills" })).not.toBeChecked()
  expect(screen.getByRole("switch", { name: "Unused MCP servers" })).toBeChecked()
})

it("keeps each concurrent toggle pending until its own request settles and retains its error", async () => {
  const skills = deferred<typeof emptyAvailability>()
  const servers = deferred<typeof emptyAvailability>()
  setCheckEnabled.mockImplementation((id: string) =>
    id === "unusedSkills" ? skills.promise : servers.promise,
  )
  render(<ChecksStepSettings />)
  act(() => availabilityEvent.callback?.({ status: "updated", snapshot: emptyAvailability }))
  const skillSwitch = screen.getByRole("switch", { name: "Unused skills" })
  const serverSwitch = screen.getByRole("switch", { name: "Unused MCP servers" })
  fireEvent.click(skillSwitch)
  fireEvent.click(serverSwitch)
  expect(skillSwitch).toBeDisabled()
  expect(serverSwitch).toBeDisabled()
  await act(async () => skills.reject(new Error("write failed")))
  expect(skillSwitch).toBeEnabled()
  expect(serverSwitch).toBeDisabled()
  expect(screen.getByRole("alert")).toHaveTextContent("Could not disable Unused skills")
  await act(async () => servers.resolve(emptyAvailability))
  expect(serverSwitch).toBeEnabled()
  expect(screen.getByRole("alert")).toHaveTextContent("Could not disable Unused skills")
})

it("saves a Jev credential through the connection session", async () => {
  render(<ChecksStepSettings />)
  const input = await screen.findByLabelText("TypeSafe API key")
  await waitFor(() => expect(input).toBeEnabled())
  fireEvent.change(input, { target: { value: "synthetic-key" } })
  fireEvent.click(screen.getByRole("button", { name: "Save Jev connection" }))
  await screen.findByText("Connection saved and selected for Model-based Checks.")
  expect(providerInvoke).toHaveBeenCalledWith(
    "save_system_one_connection",
    expect.objectContaining({
      draft: expect.objectContaining({ connection_id: "jev", credential: "synthetic-key" }),
    }),
  )
})

it("shows usage and removes the credential only when requested", async () => {
  providerInvoke.mockResolvedValueOnce({
    activeId: "jev",
    profiles: {
      jev: { ...defaultConnection("jev"), credential: { kind: "connection", id: "jev" } },
    },
  })
  getAvailability.mockResolvedValue({
    ...emptyAvailability,
    savedKey: true,
    configured: true,
    usage: { ...emptyAvailability.usage, inputTokens: 1200, confirmedCalls: 2 },
  })
  render(<ChecksStepSettings />)
  await screen.findByRole("button", { name: "Remove API key" })
  fireEvent.click(screen.getByRole("button", { name: "Model usage" }))
  expect(await screen.findByText(/1,200 input tokens/)).toBeVisible()
})

it("searches visible model-based controls and excludes removed controls", () => {
  expect(
    searchApp("Model-based Checks", "macos").some(
      (result) =>
        result.target.kind === "setting" && result.target.control === "smartChecksEnabled",
    ),
  ).toBe(true)
  expect(
    searchApp("TypeSafe API key", "macos").some(
      (result) =>
        result.target.kind === "setting" && result.target.control === "typeSafeApiKey",
    ),
  ).toBe(true)
  for (const query of ["Past sessions", "Model limits", "saved connections", "Ollama"])
    expect(searchApp(query, "macos").some((result) => result.target.kind === "setting")).toBe(
      false,
    )
})

it("keeps availability refresh errors visible", async () => {
  getAvailability.mockRejectedValueOnce(new Error("database"))
  render(<ChecksStepSettings />)
  expect(await screen.findByRole("alert")).toHaveTextContent("Could not refresh check status")
  act(() => availabilityEvent.callback?.({ status: "updated", snapshot: emptyAvailability }))
  await waitFor(() => expect(screen.queryByRole("alert")).not.toBeInTheDocument())
})
