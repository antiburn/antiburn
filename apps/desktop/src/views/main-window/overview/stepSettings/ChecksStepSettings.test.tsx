import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, expect, it, vi } from "vitest"

import { ChecksStepSettings } from "./ChecksStepSettings"
import type {
  CheckAvailability,
  CheckAvailabilityEvent,
} from "../../../../lib/checkAvailability"

const getAvailability = vi.hoisted(() => vi.fn())
const save = vi.hoisted(() => vi.fn())
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

vi.mock("../../../../lib/checkAvailability", () => ({
  getCheckAvailability: getAvailability,
  setTypeSafeApiKey: save,
  removeTypeSafeApiKey: remove,
  setCheckHistoryDays: setHistory,
  setSmartBurnChecksEnabled: setChecksEnabled,
  runCheckBackfill: runBackfill,
  onCheckAvailabilityChanged: listen,
  emptyCheckAvailability: emptyAvailability,
}))
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => null) }))
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => undefined) }))

beforeEach(() => {
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

it("enables with a password input and clears the key after native storage succeeds", async () => {
  render(<ChecksStepSettings />)
  const input = screen.getByLabelText("TypeSafe API key") as HTMLInputElement
  expect(input).toHaveAttribute("type", "password")
  expect(screen.getByRole("button", { name: "Save key and enable" })).toBeDisabled()
  fireEvent.change(input, { target: { value: "synthetic-key" } })
  fireEvent.click(screen.getByRole("button", { name: "Save key and enable" }))
  await waitFor(() => expect(save).toHaveBeenCalledWith("synthetic-key"))
  await waitFor(() => expect(input.value).toBe(""))
  expect(screen.getByRole("button", { name: "Remove key" })).toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "Remove key" }))
  await waitFor(() => expect(remove).toHaveBeenCalledOnce())
  expect(screen.queryByText(/Smart Burn Checks off/)).not.toBeInTheDocument()
})

it("restores a saved key without a Settings action", async () => {
  getAvailability.mockResolvedValue({ ...emptyAvailability, savedKey: true, configured: true })
  render(<ChecksStepSettings />)
  const toggle = await screen.findByRole("switch", { name: "Smart Burn Checks" })
  expect(toggle).toHaveAttribute("aria-checked", "true")
  expect(screen.queryByText(/Smart Burn Checks on/)).not.toBeInTheDocument()
  expect(save).not.toHaveBeenCalled()
  expect(screen.getByRole("button", { name: "Replace key" })).toBeDisabled()
  expect(screen.getByLabelText("TypeSafe API key")).toHaveAttribute(
    "placeholder",
    "••••••••••••",
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
  const toggle = await screen.findByRole("switch", { name: "Smart Burn Checks" })
  fireEvent.click(toggle)
  await waitFor(() => expect(setChecksEnabled).toHaveBeenCalledWith(false))
  expect(toggle).toHaveAttribute("aria-checked", "false")
  expect(screen.queryByText("Paused. Your key is saved.")).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Remove key" })).toBeInTheDocument()
})

it("requires a replacement after TypeSafe rejects the saved key", async () => {
  getAvailability.mockResolvedValue({
    ...emptyAvailability,
    savedKey: true,
    error: "TypeSafe rejected this API key. Replace it in Settings → Checks.",
  })
  render(<ChecksStepSettings />)
  await screen.findByText("TypeSafe rejected this API key. Replace it in Settings → Checks.")
  expect(screen.getByRole("button", { name: "Replace key" })).toBeDisabled()
  fireEvent.change(screen.getByLabelText("TypeSafe API key"), {
    target: { value: "replacement-key" },
  })
  fireEvent.click(screen.getByRole("button", { name: "Replace key" }))
  await waitFor(() => expect(save).toHaveBeenCalledWith("replacement-key"))
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
  fireEvent.click(screen.getByRole("button", { name: "Privacy and usage" }))
  await screen.findByText(/1,200 input tokens · \$0\.0000504 estimated · 3 requests/)
  expect(screen.getByText(/1 request outcomes are unknown/)).toBeInTheDocument()
})

it("retains the previous status on refresh failure and clears the error after recovery", async () => {
  const previous = { ...emptyAvailability, savedKey: true, configured: true }
  getAvailability.mockResolvedValueOnce(previous)
  const first = render(<ChecksStepSettings />)
  await screen.findByRole("switch", { name: "Smart Burn Checks" })
  first.unmount()

  getAvailability.mockRejectedValueOnce(new Error("database"))
  const second = render(<ChecksStepSettings />)
  expect(await screen.findByRole("alert")).toHaveTextContent("Could not refresh check status")
  expect(screen.getByRole("switch", { name: "Smart Burn Checks" })).toHaveAttribute(
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
  const toggle = screen.getByRole("switch", { name: "Smart Burn Checks" })
  await waitFor(() => expect(toggle).toHaveAttribute("aria-checked", "true"))
  await waitFor(() => expect(availabilityEvent.callback).not.toBeNull())

  act(() => availabilityEvent.callback?.({ status: "failed" }))

  expect(screen.getByRole("alert")).toHaveTextContent("Could not refresh check status")
  expect(toggle).toHaveAttribute("aria-checked", "true")

  act(() => availabilityEvent.callback?.({ status: "updated", snapshot: emptyAvailability }))

  expect(screen.queryByRole("alert")).not.toBeInTheDocument()
  expect(screen.queryByRole("switch", { name: "Smart Burn Checks" })).not.toBeInTheDocument()
})

it("explains the selected and excluded fields and gives the API key a full-width field", () => {
  render(<ChecksStepSettings />)
  expect(screen.getByRole("heading", { name: "Smart Burn Checks" })).toBeInTheDocument()
  expect(
    screen.getByText(
      "Finds project instructions a session did not follow. Checks start after 3 minutes of inactivity.",
    ),
  ).toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Privacy and usage" })).toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "Privacy and usage" }))
  expect(
    screen.getByText(
      /assistant messages, Bash command input \(including inline scripts, heredocs, and patches\)/,
    ),
  ).toBeInTheDocument()
  expect(
    screen.getByText(
      /User messages, tool output, dedicated edit bodies, and private thinking are excluded/,
    ),
  ).toBeInTheDocument()
  expect(
    screen.getByText(
      /global and project instruction snapshots and selected paths also leave this device/,
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
  render(<ChecksStepSettings />)
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
  expect(screen.getByText("Checking sessions; 0/4 sessions checked so far")).toBeInTheDocument()
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
  render(<ChecksStepSettings />)
  await screen.findByText("Checking sessions; 0/14 sessions checked so far")
})

it("confirms when every session in the history run finished", async () => {
  getAvailability.mockResolvedValue({
    ...emptyAvailability,
    historyDays: 7,
    backfill: { ...emptyAvailability.backfill, total: 10, completed: 10 },
  })
  render(<ChecksStepSettings />)
  expect(await screen.findByText("Finished checking 10 sessions")).toBeVisible()

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
  expect(screen.queryByText("Finished checking 10 sessions")).not.toBeInTheDocument()
  expect(screen.getByText("1 waiting to be checked · 10 sessions checked")).toBeVisible()
})

it("keeps setup, history, and the API key in separate plain-language groups", () => {
  render(<ChecksStepSettings />)
  expect(screen.getByRole("heading", { name: "Smart Burn Checks" })).toBeInTheDocument()
  expect(screen.getByRole("heading", { name: "Past sessions" })).toBeInTheDocument()
  expect(screen.getByRole("heading", { name: "TypeSafe account" })).toBeInTheDocument()
  expect(screen.getByText("Ignored Instructions")).toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Privacy and usage" })).toHaveAttribute(
    "aria-expanded",
    "false",
  )
})
