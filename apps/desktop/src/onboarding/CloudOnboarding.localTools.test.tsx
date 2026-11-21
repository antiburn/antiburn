import { act, render, screen, waitFor } from "@testing-library/react"
import { expect, it, vi } from "vitest"
import type { LocalToolsStatus } from "../lib/ipcPayloads"
import { CloudOnboarding } from "./CloudOnboarding"
import { cloudProjectFixture, localToolsStatus } from "./__tests__/cloudProjectFixtures"

it.each([
  { phase: "pending", title: "Setting up local tools", detail: "CLI and skill" },
  { phase: "ready", title: "Local tools ready", detail: "CLI tested · Skill installed" },
  {
    phase: "unavailable",
    title: "Local tools unavailable",
    detail: "Unavailable for this setup.",
  },
  {
    phase: "failed",
    title: "Local tools setup failed",
    detail: "Try Repair in Settings → CLI.",
  },
] as const)(
  "shows the actual local-tools $phase state without claiming hooks are active",
  async ({ phase, title, detail }) => {
    const { port, emit } = cloudProjectFixture()
    emit({ syncEnabled: true })
    vi.mocked(port.localTools.read).mockResolvedValue(localToolsStatus(phase))
    render(
      <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
    )
    expect(await screen.findByText(title)).toBeVisible()
    expect(screen.getByText(detail)).toBeVisible()
    expect(screen.getByRole("button", { name: "Done" })).toBeEnabled()
    expect(
      screen.queryByText(/Local agent ready|Local integration is active|nativeValidated/),
    ).not.toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "Refresh folders" })).not.toBeInTheDocument()
  },
)

it("settles pending tool setup from events without account or folder refreshes", async () => {
  const { port, ongoing, emit, emitLocalTools } = cloudProjectFixture()
  emit({ syncEnabled: true })
  vi.mocked(port.localTools.read).mockResolvedValue(localToolsStatus("pending"))
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  await waitFor(() => expect(ongoing.groups).toHaveBeenCalledOnce())
  expect(await screen.findByText("Setting up local tools")).toBeVisible()
  act(() => emitLocalTools(localToolsStatus("ready", 1)))
  expect(screen.getByText("Local tools ready")).toBeVisible()
  act(() => emitLocalTools(localToolsStatus("pending", 0)))
  expect(screen.getByText("Local tools ready")).toBeVisible()
  act(() => emitLocalTools(localToolsStatus("pending", 2)))
  expect(screen.getByText("Setting up local tools")).toBeVisible()
  act(() => emitLocalTools(localToolsStatus("failed", 3)))
  act(() => emitLocalTools(localToolsStatus("ready", 1)))
  expect(screen.getByText("Local tools setup failed")).toBeVisible()
  expect(screen.getByText("Try Repair in Settings → CLI.")).toBeVisible()
  expect(port.read).toHaveBeenCalledOnce()
  expect(port.localTools.read).toHaveBeenCalledOnce()
  expect(ongoing.groups).toHaveBeenCalledOnce()
  expect(ongoing.enable).not.toHaveBeenCalled()
  expect(screen.getByRole("button", { name: "Done" })).toBeEnabled()
})

it("does not restore ready from a lower-revision query after a failed tool event", async () => {
  const { port, emit, emitLocalTools } = cloudProjectFixture()
  emit({ syncEnabled: true })
  let finish!: (status: LocalToolsStatus) => void
  vi.mocked(port.localTools.read).mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve
      }),
  )
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  await waitFor(() => expect(port.localTools.read).toHaveBeenCalledOnce())
  act(() => emitLocalTools(localToolsStatus("failed", 1)))
  await act(async () => finish(localToolsStatus("ready")))
  expect(screen.getByText("Local tools setup failed")).toBeVisible()
  expect(screen.queryByText("Local tools ready")).not.toBeInTheDocument()
})

it("shows a newer failed query after an intervening ready event", async () => {
  const { port, emit, emitLocalTools } = cloudProjectFixture()
  emit({ syncEnabled: true })
  let finish!: (status: LocalToolsStatus) => void
  vi.mocked(port.localTools.read).mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve
      }),
  )
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  await waitFor(() => expect(port.localTools.read).toHaveBeenCalledOnce())
  act(() => emitLocalTools(localToolsStatus("ready", 5)))
  expect(screen.getByText("Local tools ready")).toBeVisible()
  await act(async () => finish(localToolsStatus("failed", 6)))
  act(() => emitLocalTools(localToolsStatus("ready", 5)))
  expect(screen.getByText("Local tools setup failed")).toBeVisible()
  expect(screen.queryByText("Local tools ready")).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Done" })).toBeEnabled()
})

it("shows an unreadable tool status without exposing raw errors or blocking Done", async () => {
  const { port, emit } = cloudProjectFixture()
  emit({ syncEnabled: true })
  vi.mocked(port.localTools.read).mockRejectedValueOnce(
    new Error("Synthetic private diagnostic"),
  )
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  expect(await screen.findByText("Local tools status unavailable")).toBeVisible()
  expect(screen.getByText("Check Settings → CLI.")).toBeVisible()
  expect(screen.queryByText("Synthetic private diagnostic")).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Done" })).toBeEnabled()
})
