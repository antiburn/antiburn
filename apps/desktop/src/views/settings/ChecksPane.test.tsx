import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, expect, it, vi } from "vitest"

import { ChecksPane } from "./ChecksPane"
import type { CheckAvailability, CheckAvailabilityEvent } from "../../lib/checkAvailability"
import { defaultConnection, type Connection } from "../../lib/smartCheckProviders"

const getAvailability = vi.hoisted(() => vi.fn())
const save = vi.hoisted(() => vi.fn())
const providerInvoke = vi.hoisted(() => vi.fn())
const remove = vi.hoisted(() => vi.fn())
const setHistory = vi.hoisted(() => vi.fn())
const runBackfill = vi.hoisted(() => vi.fn())
const setChecksEnabled = vi.hoisted(() => vi.fn())
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

vi.mock("../../lib/checkAvailability", () => ({
  getCheckAvailability: getAvailability,
  setTypeSafeApiKey: save,
  removeTypeSafeApiKey: remove,
  setCheckHistoryDays: setHistory,
  setSmartBurnChecksEnabled: setChecksEnabled,
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
  availabilityEvent.callback = null
  listen.mockClear()
  getAvailability.mockResolvedValue(emptyAvailability)
  save.mockResolvedValue({ ...emptyAvailability, configured: true, savedKey: true })
  remove.mockResolvedValue(emptyAvailability)
  setHistory.mockImplementation(async (days: 0 | 7 | 30) => ({
    ...emptyAvailability,
    historyDays: days,
  }))
  runBackfill.mockResolvedValue({ queued: 4, availability: emptyAvailability })
})

it("saves a password draft and clears the key after native storage succeeds", async () => {
  render(<ChecksPane />)
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
  render(<ChecksPane />)
  const toggle = await screen.findByRole("switch", { name: "Smart Burn Checks" })
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
  render(<ChecksPane />)
  const toggle = await screen.findByRole("switch", { name: "Smart Burn Checks" })
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
  render(<ChecksPane />)
  const toggle = screen.getByRole("switch", { name: "Smart Burn Checks" })
  await waitFor(() => expect(toggle).toHaveAttribute("aria-checked", "true"))
  await screen.findByText("Active connection: Ollama · saved-model")
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
  render(<ChecksPane />)
  const toggle = screen.getByRole("switch", { name: "Smart Burn Checks" })
  await waitFor(() => expect(toggle).toHaveAttribute("aria-checked", "true"))
  fireEvent.click(await screen.findByRole("button", { name: "Remove credential" }))
  await screen.findByText("Credential removed.")
  expect(toggle).toHaveAttribute("aria-checked", "false")
  expect(screen.queryByRole("button", { name: "Remove credential" })).not.toBeInTheDocument()
  expect(screen.getByText("Active connection: Jev (recommended) · jev-1.13.0")).toBeVisible()
})

it.each([
  "Smart Burn Checks connection update is incomplete. Retry it in Settings → Checks.",
  "Connection credential removal is incomplete. Retry it in Settings → Checks.",
])("shows a safe enablement retry error: %s", async (message) => {
  getAvailability.mockResolvedValue(emptyAvailability)
  setChecksEnabled.mockRejectedValueOnce(message)
  render(<ChecksPane />)
  const toggle = screen.getByRole("switch", { name: "Smart Burn Checks" })
  await waitFor(() => expect(toggle).toHaveAttribute("aria-checked", "false"))
  fireEvent.click(toggle)
  expect(await screen.findByRole("alert")).toHaveTextContent(message)
  expect(toggle).toHaveAttribute("aria-checked", "false")
})

it("removes a migrated Jev credential through the existing secure-storage command", async () => {
  getAvailability.mockResolvedValue({ ...emptyAvailability, savedKey: true, configured: true })
  render(<ChecksPane />)
  fireEvent.click(await screen.findByRole("button", { name: "Remove credential" }))
  await waitFor(() => expect(remove).toHaveBeenCalledOnce())
})

it("requires a replacement after TypeSafe rejects the saved key", async () => {
  getAvailability.mockResolvedValue({
    ...emptyAvailability,
    savedKey: true,
    error: "TypeSafe rejected this API key. Replace it in Settings → Checks.",
  })
  render(<ChecksPane />)
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
  render(<ChecksPane />)
  await screen.findByText(/1,200 input tokens · \$0\.0000504 estimated · 3 requests/)
  expect(screen.getByText(/1 request outcomes are unknown/)).toBeInTheDocument()
})

it("retains the previous status on refresh failure and clears the error after recovery", async () => {
  const previous = { ...emptyAvailability, savedKey: true, configured: true }
  getAvailability.mockResolvedValueOnce(previous)
  const first = render(<ChecksPane />)
  await screen.findByRole("switch", { name: "Smart Burn Checks" })
  first.unmount()

  getAvailability.mockRejectedValueOnce(new Error("database"))
  const second = render(<ChecksPane />)
  expect(await screen.findByRole("alert")).toHaveTextContent("Could not refresh check status")
  expect(screen.getByRole("switch", { name: "Smart Burn Checks" })).toHaveAttribute(
    "aria-checked",
    "true",
  )
  second.unmount()

  getAvailability.mockResolvedValueOnce(previous)
  render(<ChecksPane />)
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
  render(<ChecksPane />)
  const toggle = screen.getByRole("switch", { name: "Smart Burn Checks" })
  await waitFor(() => expect(toggle).toHaveAttribute("aria-checked", "true"))
  await waitFor(() => expect(availabilityEvent.callback).not.toBeNull())

  act(() => availabilityEvent.callback?.({ status: "failed" }))

  expect(screen.getByRole("alert")).toHaveTextContent("Could not refresh check status")
  expect(toggle).toHaveAttribute("aria-checked", "true")

  act(() => availabilityEvent.callback?.({ status: "updated", snapshot: emptyAvailability }))

  expect(screen.queryByRole("alert")).not.toBeInTheDocument()
  expect(screen.getByRole("switch", { name: "Smart Burn Checks" })).toHaveAttribute(
    "aria-checked",
    "false",
  )
})

it("explains the selected and excluded fields and gives the API key a full-width field", () => {
  render(<ChecksPane />)
  expect(screen.getByRole("heading", { name: "Smart Burn Checks" })).toBeInTheDocument()
  expect(
    screen.getByText(
      "Find avoidable work after a session is idle. Checks start after 3 minutes of inactivity.",
    ),
  ).toBeInTheDocument()
  expect(
    screen.getByText(
      /user and assistant messages, tool inputs and outputs, and current skill descriptions/,
    ),
  ).toBeInTheDocument()
  expect(screen.getByText(/Private thinking is excluded/)).toBeInTheDocument()
  expect(
    screen.getByText(
      /Data stays on this device only when the inference endpoint runs on this device/,
    ),
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
  render(<ChecksPane />)
  await screen.findByText("13 waiting to be checked · 5 waiting for session analysis")
})

it("saves a history window and runs checks only after the user asks", async () => {
  setHistory.mockResolvedValue({
    ...emptyAvailability,
    configured: true,
    historyDays: 30,
  })
  runBackfill.mockResolvedValue({
    queued: 4,
    availability: {
      ...emptyAvailability,
      configured: true,
      historyDays: 30,
      backfill: { ...emptyAvailability.backfill, total: 4, queued: 4 },
    },
  })
  render(<ChecksPane />)

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
})

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
  render(<ChecksPane />)
  await screen.findByText("Running checks; 0/14 check jobs complete")
})

it("confirms when every session in the history run finished", async () => {
  getAvailability.mockResolvedValue({
    ...emptyAvailability,
    historyDays: 7,
    backfill: { ...emptyAvailability.backfill, total: 10, completed: 10 },
  })
  render(<ChecksPane />)
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

it("keeps setup, history, and the API key in separate plain-language groups", () => {
  render(<ChecksPane />)
  expect(screen.getByRole("heading", { name: "Smart Burn Checks" })).toBeInTheDocument()
  expect(screen.getByRole("heading", { name: "Past sessions" })).toBeInTheDocument()
  expect(screen.getByRole("heading", { name: "Model connection" })).toBeInTheDocument()
  expect(screen.getByText("Ignored Instructions")).toBeInTheDocument()
  expect(
    screen.getByRole("button", { name: "Use Ollama or another provider" }),
  ).toHaveAttribute("aria-expanded", "false")
})
