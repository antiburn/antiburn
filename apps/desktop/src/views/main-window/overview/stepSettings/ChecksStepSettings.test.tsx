import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, expect, it, vi } from "vitest"

import { defaultConnection, type Connection } from "../../../../lib/smartCheckProviders"
import { searchApp, resolveStepSettingsSearchTarget } from "../../../../lib/appSearch"
import { SettingsTargetFocus } from "../../../settings/SettingsTargetFocus"
import { StepSettings } from "./StepSettings"
import { ChecksStepSettings } from "./ChecksStepSettings"
import type {
  CheckAvailability,
  CheckAvailabilityEvent,
} from "../../../../lib/checkAvailability"

const getAvailability = vi.hoisted(() => vi.fn())
const save = vi.hoisted(() => vi.fn())
const providerInvoke = vi.hoisted(() => vi.fn())
const remove = vi.hoisted(() => vi.fn())
const setHistory = vi.hoisted(() => vi.fn())
const runBackfill = vi.hoisted(() => vi.fn())
const setChecksEnabled = vi.hoisted(() => vi.fn())
const setCheckEnabled = vi.hoisted(() => vi.fn())
const availabilityEvent = vi.hoisted(() => ({
  callback: null as ((event: CheckAvailabilityEvent) => void) | null,
}))
const listen = vi.hoisted(() =>
  vi.fn(async (callback: (event: CheckAvailabilityEvent) => void) => {
    availabilityEvent.callback = callback
    return () => undefined
  }),
)
const emptyAvailability = vi.hoisted(() => ({
  revision: 1,
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
  setTypeSafeApiKey: save,
  removeTypeSafeApiKey: remove,
  setCheckHistoryDays: setHistory,
  setSmartBurnChecksEnabled: setChecksEnabled,
  setCheckEnabled,
  runCheckBackfill: runBackfill,
  onCheckAvailabilityChanged: listen,
  emptyCheckAvailability: emptyAvailability,
}))
vi.mock("@tauri-apps/api/core", () => ({ invoke: providerInvoke }))
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => undefined) }))

beforeEach(() => {
  providerInvoke.mockReset()
  remove.mockClear()
  providerInvoke.mockImplementation(async (command: string) =>
    command === "get_system_one_settings" || command === "save_system_one_connection"
      ? {
          activeId: "jev",
          profiles: {
            jev: {
              ...defaultConnection("jev"),
              credential:
                command === "save_system_one_connection"
                  ? { kind: "connection", id: "jev" }
                  : { kind: "legacy_type_safe" },
            },
          },
        }
      : null,
  )
  emptyAvailability.revision += 10
  availabilityEvent.callback = null
  setCheckEnabled.mockClear()
  listen.mockClear()
  getAvailability.mockResolvedValue(emptyAvailability)
  save.mockResolvedValue({ ...emptyAvailability, configured: true, savedKey: true })
  remove.mockResolvedValue(emptyAvailability)
  setHistory.mockImplementation(async (days: 0 | 7 | 30) => ({
    ...emptyAvailability,
    historyDays: days,
  }))
  runBackfill.mockResolvedValue({ queued: 4, availability: emptyAvailability })
  setCheckEnabled.mockImplementation(async (detector: string, enabled: boolean) => ({
    ...emptyAvailability,
    revision: emptyAvailability.revision + 1,
    checks: emptyAvailability.checks.map((check) =>
      check.id === detector ? { ...check, enabled } : check,
    ),
  }))
})

it("shows every local and Smart check as an independent preference", () => {
  render(<ChecksStepSettings />)
  expect(screen.getByRole("heading", { name: "Local Checks" })).toBeInTheDocument()
  expect(screen.getByRole("heading", { name: "Smart Burn Checks" })).toBeInTheDocument()
  expect(screen.getAllByRole("switch")).toHaveLength(14)
  expect(screen.getByRole("switch", { name: "Session overdepth" })).toBeChecked()
  expect(screen.getByRole("switch", { name: "Ignored instructions" })).not.toBeChecked()
  expect(
    screen.getAllByText(/Enable Smart Burn Checks with an active provider connection below/),
  ).toHaveLength(4)
})

it("saves one check without changing its siblings", async () => {
  render(<ChecksStepSettings />)
  fireEvent.click(screen.getByRole("switch", { name: "Unused skills" }))
  await waitFor(() => expect(setCheckEnabled).toHaveBeenCalledWith("unusedSkills", false))
  expect(screen.getByRole("switch", { name: "Unused skills" })).not.toBeChecked()
  expect(screen.getByRole("switch", { name: "Unused MCP servers" })).toBeChecked()
})

it.each([
  ["ignoredInstructions", "Ignored instructions"],
  ["skillOpportunities", "Enable skill opportunities"],
  ["overExploring", "Enable over-exploring"],
  ["scopeCreep", "Enable scope creep"],
] as const)("saves %s independently of provider enablement", async (id, label) => {
  render(<ChecksStepSettings />)
  await waitFor(() => expect(screen.getByRole("switch", { name: label })).not.toBeChecked())
  fireEvent.click(screen.getByRole("switch", { name: label }))
  await waitFor(() => expect(setCheckEnabled).toHaveBeenCalledWith(id, true))
  expect(screen.getByRole("switch", { name: label })).toBeChecked()
  expect(screen.getByRole("switch", { name: "Enable Smart Burn Checks" })).not.toBeChecked()
  expect(screen.getByRole("switch", { name: "Unused skills" })).toBeChecked()
})

it.each(["macos", "windows", "linux"] as const)(
  "reveals provider limits from the Checks step search destination on %s",
  async (platform) => {
    const result = searchApp("Model limits", platform)[0]!
    if (result.target.kind !== "stepSetting") throw new Error("Missing step destination")
    const request = resolveStepSettingsSearchTarget(result.target)
    const { container } = render(
      <StepSettings step={request.step} control={request.control} targetRevision={1} />,
    )
    await waitFor(() => expect(screen.getByLabelText("TypeSafe API key")).toBeEnabled())
    if (!(container instanceof HTMLDivElement))
      throw new Error("Missing step settings container")
    HTMLElement.prototype.scrollIntoView = vi.fn()
    new SettingsTargetFocus().attach(container, request.step, request.control, 1)()
    expect(screen.getByRole("button", { name: "Model limits" })).toHaveAttribute(
      "aria-expanded",
      "true",
    )
    expect(
      container
        .querySelector('[data-settings-control="smartCheckLimits"]')
        ?.contains(document.activeElement),
    ).toBe(true)
    expect(
      providerInvoke.mock.calls.every(([command]) => command === "get_system_one_settings"),
    ).toBe(true)
    expect(setCheckEnabled).not.toHaveBeenCalled()
  },
)

it("keeps the saved value and reports a failed check preference change", async () => {
  setCheckEnabled.mockRejectedValueOnce(new Error("store"))
  render(<ChecksStepSettings />)
  const toggle = screen.getByRole("switch", { name: "Unused skills" })
  await waitFor(() => expect(toggle).toBeChecked())
  fireEvent.click(toggle)
  expect(await screen.findByRole("alert")).toHaveTextContent("Could not disable Unused skills.")
  expect(toggle).toBeChecked()
})

it("saves a password draft and clears the key after native storage succeeds", async () => {
  render(<ChecksStepSettings />)
  const input = screen.getByLabelText("TypeSafe API key") as HTMLInputElement
  expect(input).toHaveAttribute("type", "password")
  await waitFor(() => expect(input).toBeEnabled())
  fireEvent.change(input, { target: { value: "synthetic-key" } })
  fireEvent.click(screen.getByRole("button", { name: "Save connection" }))
  await waitFor(() =>
    expect(providerInvoke).toHaveBeenCalledWith(
      "save_system_one_connection",
      expect.objectContaining({
        draft: expect.objectContaining({ credential: "synthetic-key" }),
      }),
    ),
  )
  await waitFor(() => expect(input.value).toBe(""))
  expect(screen.getByRole("button", { name: "Remove credential" })).toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "Remove credential" }))
  await waitFor(() =>
    expect(providerInvoke).toHaveBeenCalledWith("remove_system_one_credential", {
      connectionId: "jev",
    }),
  )
  expect(screen.queryByText(/Smart Burn Checks off/)).not.toBeInTheDocument()
})

it("restores a saved key without a Settings action", async () => {
  getAvailability.mockResolvedValue({ ...emptyAvailability, savedKey: true, configured: true })
  render(<ChecksStepSettings />)
  const toggle = await screen.findByRole("switch", { name: "Enable Smart Burn Checks" })
  expect(toggle).toHaveAttribute("aria-checked", "true")
  expect(screen.queryByText(/Smart Burn Checks on/)).not.toBeInTheDocument()
  expect(save).not.toHaveBeenCalled()
  await waitFor(() => expect(screen.getByLabelText("TypeSafe API key")).toBeEnabled())
  expect(screen.getByLabelText("TypeSafe API key")).toHaveAttribute(
    "placeholder",
    "Saved credential; enter a replacement",
  )
})

it("pauses checks without removing the saved key", async () => {
  getAvailability.mockResolvedValue({ ...emptyAvailability, savedKey: true, configured: true })
  setChecksEnabled.mockResolvedValue({
    ...emptyAvailability,
    savedKey: true,
    configured: false,
  })
  render(<ChecksStepSettings />)
  const toggle = await screen.findByRole("switch", { name: "Enable Smart Burn Checks" })
  fireEvent.click(toggle)
  await waitFor(() => expect(setChecksEnabled).toHaveBeenCalledWith(false))
  expect(toggle).toHaveAttribute("aria-checked", "false")
  expect(screen.queryByText("Paused. Your key is saved.")).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Remove credential" })).toBeInTheDocument()
})

it("renders keyless Ollama as paused after the backend enablement change", async () => {
  const connection = { ...defaultConnection("ollama"), model: "saved-model" }
  providerInvoke.mockResolvedValue({ activeId: "ollama", profiles: { ollama: connection } })
  getAvailability.mockResolvedValue({ ...emptyAvailability, configured: true })
  let finish: ((value: CheckAvailability) => void) | undefined
  setChecksEnabled.mockImplementationOnce(
    () =>
      new Promise<CheckAvailability>((resolve) => {
        finish = resolve
      }),
  )
  render(<ChecksStepSettings />)
  const toggle = screen.getByRole("switch", { name: "Enable Smart Burn Checks" })
  await waitFor(() => expect(toggle).toHaveAttribute("aria-checked", "true"))
  await waitFor(() =>
    expect(screen.getByLabelText("Smart check provider")).toHaveValue("ollama"),
  )
  expect(screen.getByLabelText("Provider model")).toHaveValue("saved-model")
  expect(screen.queryByRole("button", { name: "Remove credential" })).not.toBeInTheDocument()
  fireEvent.click(toggle)
  expect(toggle).toBeDisabled()
  await act(async () => finish?.(emptyAvailability))
  expect(toggle).toHaveAttribute("aria-checked", "false")
  expect(toggle).toBeEnabled()
  expect(setChecksEnabled).toHaveBeenCalledWith(false)
  expect(screen.getByLabelText("Provider credential (optional)")).toHaveValue("")
})

it("renders an active credential removal and the backend pause event together", async () => {
  let connection: Connection = {
    ...defaultConnection("jev"),
    credential: { kind: "connection", id: "jev" },
  }
  providerInvoke.mockImplementation(async (command: string) => {
    if (command === "remove_system_one_credential") {
      connection = { ...connection, credential: null, revision: connection.revision + 1 }
      availabilityEvent.callback?.({ status: "updated", snapshot: emptyAvailability })
      return null
    }
    return { activeId: "jev", profiles: { jev: connection } }
  })
  getAvailability.mockResolvedValue({ ...emptyAvailability, configured: true })
  render(<ChecksStepSettings />)
  const toggle = screen.getByRole("switch", { name: "Enable Smart Burn Checks" })
  await waitFor(() => expect(toggle).toHaveAttribute("aria-checked", "true"))
  fireEvent.click(await screen.findByRole("button", { name: "Remove credential" }))
  await screen.findByText("Credential removed.")
  expect(toggle).toHaveAttribute("aria-checked", "false")
  expect(screen.queryByRole("button", { name: "Remove credential" })).not.toBeInTheDocument()
  expect(screen.getByLabelText("Smart check provider")).toHaveValue("jev")
  expect(screen.getByLabelText("Provider model")).toHaveValue("jev-1.13.0")
})

it.each([
  "Smart Burn Checks connection update is incomplete. Retry it in Settings → Checks.",
  "Connection credential removal is incomplete. Retry it in Settings → Checks.",
])("shows a safe enablement retry error: %s", async (message) => {
  getAvailability.mockResolvedValue(emptyAvailability)
  setChecksEnabled.mockRejectedValueOnce(message)
  render(<ChecksStepSettings />)
  const toggle = screen.getByRole("switch", { name: "Enable Smart Burn Checks" })
  await waitFor(() => expect(toggle).toHaveAttribute("aria-checked", "false"))
  fireEvent.click(toggle)
  expect(await screen.findByRole("alert")).toHaveTextContent(message)
  expect(toggle).toHaveAttribute("aria-checked", "false")
})

it("removes a migrated Jev credential through the existing secure-storage command", async () => {
  getAvailability.mockResolvedValue({ ...emptyAvailability, savedKey: true, configured: true })
  render(<ChecksStepSettings />)
  fireEvent.click(await screen.findByRole("button", { name: "Remove credential" }))
  await waitFor(() => expect(remove).toHaveBeenCalledOnce())
})

it("requires a replacement after TypeSafe rejects the saved key", async () => {
  getAvailability.mockResolvedValue({
    ...emptyAvailability,
    savedKey: true,
    error: "TypeSafe rejected this API key. Replace it in Settings → Checks.",
  })
  render(<ChecksStepSettings />)
  await screen.findByText("TypeSafe rejected this API key. Replace it in Settings → Checks.")
  await waitFor(() => expect(screen.getByLabelText("TypeSafe API key")).toBeEnabled())
  fireEvent.change(screen.getByLabelText("TypeSafe API key"), {
    target: { value: "replacement-key" },
  })
  fireEvent.click(screen.getByRole("button", { name: "Save connection" }))
  await waitFor(() =>
    expect(providerInvoke).toHaveBeenCalledWith(
      "save_system_one_connection",
      expect.objectContaining({
        draft: expect.objectContaining({ credential: "replacement-key" }),
      }),
    ),
  )
})

it("shows bounded local usage and unknown outcomes separately", async () => {
  getAvailability.mockResolvedValue({
    ...emptyAvailability,
    usage: {
      inputTokens: 1200,
      outputTokens: 18,
      confirmedCalls: 3,
      cacheHits: 2,
      unknownOutcomes: 1,
      estimatedUsd: "$0.0000504",
      lastUsedAtEpoch: 1_000,
    },
  })
  render(<ChecksStepSettings />)
  expect(screen.getByRole("button", { name: "Model usage" })).toHaveAttribute(
    "aria-expanded",
    "false",
  )
  expect(screen.queryByText(/1,200 input tokens/)).not.toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "Model usage" }))
  await screen.findByText(/1,200 input tokens · \$0\.0000504 estimated · 3 requests/)
  expect(screen.getByText(/1 request outcomes are unknown/)).toBeInTheDocument()
})

it("retains the previous status on refresh failure and clears the error after recovery", async () => {
  const previous = { ...emptyAvailability, savedKey: true, configured: true }
  getAvailability.mockResolvedValueOnce(previous)
  const first = render(<ChecksStepSettings />)
  await waitFor(() =>
    expect(screen.getByRole("switch", { name: "Enable Smart Burn Checks" })).toBeChecked(),
  )
  first.unmount()

  getAvailability.mockRejectedValueOnce(new Error("database"))
  const second = render(<ChecksStepSettings />)
  expect(await screen.findByRole("alert")).toHaveTextContent("Could not refresh check status")
  expect(screen.getByRole("switch", { name: "Enable Smart Burn Checks" })).toHaveAttribute(
    "aria-checked",
    "true",
  )
  second.unmount()

  getAvailability.mockResolvedValueOnce(previous)
  render(<ChecksStepSettings />)
  await waitFor(() => expect(screen.queryByRole("alert")).not.toBeInTheDocument())
})

it("shows an availability-event failure without clearing the last known usage", async () => {
  const availability: CheckAvailability = {
    ...emptyAvailability,
    configured: true,
    savedKey: true,
    usage: { ...emptyAvailability.usage, inputTokens: 123, confirmedCalls: 1 },
  }
  getAvailability.mockResolvedValue(availability)
  render(<ChecksStepSettings />)
  const toggle = screen.getByRole("switch", { name: "Enable Smart Burn Checks" })
  await waitFor(() => expect(toggle).toHaveAttribute("aria-checked", "true"))
  await waitFor(() => expect(availabilityEvent.callback).not.toBeNull())

  act(() => availabilityEvent.callback?.({ status: "failed" }))

  expect(screen.getByRole("alert")).toHaveTextContent("Could not refresh check status")
  expect(toggle).toHaveAttribute("aria-checked", "true")

  fireEvent.click(screen.getByRole("button", { name: "Model usage" }))
  expect(screen.getByText(/123 input tokens/)).toBeVisible()

  act(() => availabilityEvent.callback?.({ status: "updated", snapshot: emptyAvailability }))

  expect(screen.queryByRole("alert")).not.toBeInTheDocument()
  expect(screen.getByRole("switch", { name: "Enable Smart Burn Checks" })).toHaveAttribute(
    "aria-checked",
    "false",
  )
})

it("does not let an older availability event undo a saved preference", async () => {
  const latest = {
    ...emptyAvailability,
    revision: emptyAvailability.revision + 2,
    checks: emptyAvailability.checks.map((check) =>
      check.id === "unusedSkills" ? { ...check, enabled: false } : check,
    ),
  }
  getAvailability.mockResolvedValue(latest)
  render(<ChecksStepSettings />)
  const toggle = screen.getByRole("switch", { name: "Unused skills" })
  await waitFor(() => expect(toggle).not.toBeChecked())

  act(() =>
    availabilityEvent.callback?.({
      status: "updated",
      snapshot: { ...emptyAvailability, revision: latest.revision - 1 },
    }),
  )

  expect(toggle).not.toBeChecked()
})

it("explains the selected and excluded fields and gives the API key a full-width field", () => {
  render(<ChecksStepSettings />)
  expect(screen.getByRole("heading", { name: "Smart Burn Checks" })).toBeInTheDocument()
  expect(
    screen.getByText(/Finds project instructions a session did not follow/),
  ).toBeInTheDocument()
  expect(
    screen.getByText(
      /Checks send selected session content and skill descriptions to the provider in use/,
    ),
  ).toBeInTheDocument()
  expect(screen.getByText(/excluding private thinking/)).toBeInTheDocument()
  expect(
    screen.getByText(/Data stays local only when inference runs on this device/),
  ).toBeInTheDocument()
  expect(screen.getByLabelText("TypeSafe API key")).toHaveClass("w-full")
})

it("shows when selected sessions are waiting for current evidence", async () => {
  getAvailability.mockResolvedValue({
    ...emptyAvailability,
    historyDays: 30,
    backfill: {
      total: 18,
      waitingForData: 5,
      ready: 13,
      waitingForIdle: 0,
      queued: 0,
      running: 0,
      completed: 0,
      skipped: 0,
      failed: 0,
    },
  })
  render(<ChecksStepSettings />)
  await screen.findByText("13 waiting to be checked · 5 waiting for session analysis")
})

it.each(["ignoredInstructions", "skillOpportunities", "overExploring", "scopeCreep"] as const)(
  "runs history with only %s selected after the user asks",
  async (detector) => {
    setHistory.mockResolvedValue({
      ...emptyAvailability,
      configured: true,
      historyDays: 30,
      checks: emptyAvailability.checks.map((check) =>
        check.id === detector ? { ...check, enabled: true } : check,
      ),
    })
    runBackfill.mockResolvedValue({
      queued: 4,
      availability: {
        ...emptyAvailability,
        configured: true,
        historyDays: 30,
        checks: emptyAvailability.checks.map((check) =>
          check.id === detector ? { ...check, enabled: true } : check,
        ),
        backfill: { ...emptyAvailability.backfill, total: 4, queued: 4 },
      },
    })
    render(<ChecksStepSettings />)

    await waitFor(() =>
      expect(screen.getByRole("radio", { name: "Future only" })).toHaveAttribute(
        "aria-checked",
        "true",
      ),
    )
    expect(screen.getByRole("button", { name: "Check past sessions" })).toBeDisabled()
    fireEvent.click(screen.getByRole("radio", { name: "30 days" }))
    await waitFor(() => expect(setHistory).toHaveBeenCalledWith(30))
    fireEvent.click(screen.getByRole("button", { name: "Check past sessions" }))
    await waitFor(() => expect(runBackfill).toHaveBeenCalledOnce())
    expect(screen.getByText("Running checks; 0/4 check jobs complete")).toBeInTheDocument()
    expect(screen.queryByText(/added .* to the queue/i)).not.toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Check past sessions" })).toBeDisabled()
    expect(screen.queryByText(/running,.*waiting/)).not.toBeInTheDocument()
  },
)

it("keeps the checking denominator stable while sessions change status", async () => {
  getAvailability.mockResolvedValue({
    ...emptyAvailability,
    configured: true,
    historyDays: 30,
    backfill: {
      ...emptyAvailability.backfill,
      total: 14,
      waitingForData: 4,
      running: 1,
      failed: 2,
    },
  })
  render(<ChecksStepSettings />)
  await screen.findByText("Running checks; 0/14 check jobs complete")
})

it("confirms when every session in the history run finished", async () => {
  getAvailability.mockResolvedValue({
    ...emptyAvailability,
    historyDays: 7,
    backfill: { ...emptyAvailability.backfill, total: 10, completed: 10 },
  })
  render(<ChecksStepSettings />)
  expect(await screen.findByText("Finished 10 check jobs")).toBeVisible()

  act(() =>
    availabilityEvent.callback?.({
      status: "updated",
      snapshot: {
        ...emptyAvailability,
        historyDays: 7,
        backfill: { ...emptyAvailability.backfill, total: 11, completed: 10, ready: 1 },
      },
    }),
  )
  expect(screen.queryByText("Finished 10 check jobs")).not.toBeInTheDocument()
  expect(screen.getByText("1 waiting to be checked · 10 check jobs complete")).toBeVisible()
})

it("keeps setup, history, and the API key in separate plain-language groups", async () => {
  render(<ChecksStepSettings />)
  expect(screen.getByRole("heading", { name: "Smart Burn Checks" })).toBeInTheDocument()
  expect(screen.getByRole("heading", { name: "Past sessions" })).toBeInTheDocument()
  expect(screen.getByRole("heading", { name: "Decision model" })).toBeInTheDocument()
  expect(screen.getByText("Ignored instructions")).toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Model limits" })).toHaveAttribute(
    "aria-expanded",
    "false",
  )
  expect(screen.getByLabelText("Smart check provider")).toBeVisible()
  expect(await screen.findByLabelText("Provider model")).toBeVisible()
  expect(screen.getByLabelText("TypeSafe API key")).toBeVisible()
  expect(screen.getByText(/A decision model reviews selected session content/)).toBeVisible()
})

it("requires a nonblank credential before saving an unsaved Jev connection", async () => {
  providerInvoke.mockResolvedValue({
    activeId: "jev",
    profiles: { jev: defaultConnection("jev") },
  })
  render(<ChecksStepSettings />)
  const input = screen.getByLabelText("TypeSafe API key")
  await waitFor(() => expect(input).toBeEnabled())
  const saveConnection = screen.getByRole("button", { name: "Save connection" })
  expect(saveConnection).toBeDisabled()
  fireEvent.change(input, { target: { value: "   " } })
  expect(saveConnection).toBeDisabled()
  fireEvent.change(input, { target: { value: "synthetic-key" } })
  expect(saveConnection).toBeEnabled()
  expect(
    providerInvoke.mock.calls.every(([command]) => command === "get_system_one_settings"),
  ).toBe(true)
})
