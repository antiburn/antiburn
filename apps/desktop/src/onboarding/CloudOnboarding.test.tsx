import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, expect, it, vi } from "vitest"
import { emptyCloudSnapshot, type CloudSnapshot, type CloudWorkspace } from "../lib/cloud"
import { CloudOnboarding } from "./CloudOnboarding"
import { cloudProjectFixture, ongoingStatus } from "./__tests__/cloudProjectFixtures"

const analytics = vi.hoisted(() => ({ action: vi.fn(), view: vi.fn() }))
vi.mock("../lib/onboardingAnalytics", () => ({
  noteCloudAction: analytics.action,
  noteCloudScreenViewed: analytics.view,
}))

beforeEach(() => vi.clearAllMocks())

function fixture(patch: Partial<CloudSnapshot> = {}) {
  const fixture = cloudProjectFixture()
  fixture.emit(patch)
  return fixture
}

it("offers browser setup without collecting invite data or requiring it for local completion", async () => {
  const { port, ongoing } = fixture({
    draft: { screen: "identity", folders: [] },
  })
  const openSetup = vi.fn().mockRejectedValueOnce(new Error("Browser unavailable."))
  openSetup.mockResolvedValue(undefined)
  port.openSetup = openSetup
  const done = vi.fn()
  render(<CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={done} />)
  const open = await screen.findByRole("button", { name: "Finish setup in browser" })
  expect(screen.queryByRole("textbox")).not.toBeInTheDocument()
  fireEvent.click(open)
  expect(await screen.findByRole("alert")).toHaveTextContent("Could not open Cloud setup")
  fireEvent.click(open)
  await waitFor(() => expect(screen.queryByRole("alert")).not.toBeInTheDocument())
  expect(openSetup).toHaveBeenCalledTimes(2)
  fireEvent.click(screen.getByRole("button", { name: "Continue locally" }))
  expect(done).toHaveBeenCalledOnce()
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it("disables device access updates while the parent is busy or browser setup is opening", async () => {
  const { port } = fixture({
    captureUpdateAvailable: true,
    draft: { screen: "identity", folders: [] },
  })
  let finishOpening!: () => void
  port.openSetup = vi.fn(
    () =>
      new Promise<void>((resolve) => {
        finishOpening = resolve
      }),
  )
  const props = { port, analytics: false, onBack: vi.fn(), onComplete: vi.fn() }
  const { rerender } = render(<CloudOnboarding {...props} busy />)
  const update = await screen.findByRole("button", {
    name: "Update device access to upload sessions",
  })
  expect(update).toBeDisabled()

  rerender(<CloudOnboarding {...props} />)
  expect(update).toBeEnabled()
  fireEvent.click(screen.getByRole("button", { name: "Finish setup in browser" }))
  expect(update).toBeDisabled()

  await act(async () => finishOpening())
  expect(update).toBeEnabled()
})

it("keeps an upload access update cancellable in onboarding after sign-in is lost", async () => {
  const { port, emit, ongoing } = fixture({
    captureUpdateAvailable: true,
    draft: { screen: "identity", folders: [] },
  })
  const disconnected: CloudSnapshot = {
    ...emptyCloudSnapshot,
    authEnabled: true,
    status: "offline",
  }
  const current = await port.read()
  const begin = vi
    .spyOn(port, "begin")
    .mockResolvedValueOnce({ ...current, accessUpdate: "waiting" })
    .mockResolvedValueOnce(current)
  const cancel = vi.spyOn(port, "cancel").mockResolvedValue(disconnected)
  const signOut = vi.spyOn(port, "signOut")
  render(
    <CloudOnboarding
      port={port}
      analytics={false}
      holdIdentity
      onBack={vi.fn()}
      onComplete={vi.fn()}
    />,
  )
  fireEvent.click(
    await screen.findByRole("button", { name: "Update device access to upload sessions" }),
  )
  expect(await screen.findByText(/your current connection stays active/i)).toBeVisible()
  expect(begin).toHaveBeenCalledExactlyOnceWith("capture")

  act(() => emit({ ...disconnected, destination: null, accessUpdate: "waiting" }))
  expect(screen.getByRole("heading", { name: "Sign in to Antiburn Cloud" })).toBeVisible()
  expect(screen.queryByText(/your current connection stays active/i)).toBeNull()
  expect(screen.queryByRole("button", { name: "Sign in with GitHub" })).toBeNull()
  fireEvent.click(screen.getByRole("button", { name: "Cancel access update" }))

  const signIn = await screen.findByRole("button", { name: "Sign in with GitHub" })
  expect(screen.queryByRole("button", { name: "Cancel access update" })).toBeNull()
  fireEvent.click(signIn)
  expect(await screen.findByRole("heading", { name: "You’re signed in" })).toBeVisible()
  expect(screen.queryByRole("button", { name: "Cancel sign-in" })).toBeNull()
  expect(begin).toHaveBeenLastCalledWith()
  expect(cancel).toHaveBeenCalledOnce()
  expect(signOut).not.toHaveBeenCalled()
  expect(port.save).not.toHaveBeenCalled()
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it("waits for a fresh workspace list before showing folder choices", async () => {
  const { port, ongoing } = fixture({
    draft: { screen: "identity", folders: [] },
  })
  const personal: CloudWorkspace = {
    id: "synthetic-personal",
    name: "Personal",
    kind: "personal",
  }
  const team: CloudWorkspace = { id: "synthetic-team", name: "Example Team", kind: "team" }
  let resolveWorkspaces!: (result: CloudWorkspace[]) => void
  const response = new Promise<CloudWorkspace[]>((resolve) => {
    resolveWorkspaces = resolve
  })
  port.openSetup = vi.fn().mockResolvedValue(undefined)
  const workspaces = vi.fn<NonNullable<typeof port.workspaces>>().mockResolvedValue([personal])
  port.workspaces = workspaces
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  const next = await screen.findByRole("button", { name: "Continue" })
  await waitFor(() => expect(workspaces).toHaveBeenCalledOnce())

  workspaces.mockResolvedValue([personal, team]).mockReturnValueOnce(response)
  vi.useFakeTimers()
  try {
    fireEvent.click(next)
    await act(() => vi.advanceTimersByTimeAsync(16_000))
    expect(next).toBeDisabled()
    expect(port.save).not.toHaveBeenCalled()
    expect(screen.queryByRole("button", { name: "Start syncing" })).not.toBeInTheDocument()
  } finally {
    vi.useRealTimers()
  }

  await act(async () => resolveWorkspaces([personal, team]))
  expect(await screen.findByRole("button", { name: "Start syncing" })).toBeVisible()
  expect(
    await screen.findByRole("option", { name: "Team workspace: Example Team" }),
  ).toBeInTheDocument()
  expect(workspaces).toHaveBeenLastCalledWith({
    accountId: "synthetic-account",
    destinationId: personal.id,
  })
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it("lets the user retry the workspace check or continue locally", async () => {
  const { port } = fixture({ draft: { screen: "identity", folders: [] } })
  port.openSetup = vi.fn()
  port.workspaces = vi.fn().mockRejectedValue(new Error("Could not verify Cloud workspaces."))
  const done = vi.fn()
  render(<CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={done} />)
  const next = await screen.findByRole("button", { name: "Continue" })
  fireEvent.click(next)
  expect(await screen.findByRole("alert")).toHaveTextContent("Try again")
  expect(next).toBeEnabled()
  expect(port.save).not.toHaveBeenCalled()
  fireEvent.click(screen.getByRole("button", { name: "Continue locally" }))
  expect(done).toHaveBeenCalledOnce()
})

it("offers sign-in instead of retrying an unauthorized workspace check", async () => {
  const { port, emit } = fixture({ draft: { screen: "identity", folders: [] } })
  port.openSetup = vi.fn()
  const workspaces = vi
    .fn<NonNullable<typeof port.workspaces>>()
    .mockResolvedValue([{ id: "synthetic-personal", name: "Personal", kind: "personal" }])
  port.workspaces = workspaces
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  const next = await screen.findByRole("button", { name: "Continue" })
  await waitFor(() => expect(workspaces).toHaveBeenCalledOnce())
  workspaces.mockImplementationOnce(async () => {
    emit({ status: "reauth_required", identity: null, destination: null })
    throw new Error("Cloud workspace sign-in required.")
  })
  fireEvent.click(next)
  expect(await screen.findByText("Sign in again to use Antiburn Cloud.")).toBeVisible()
  expect(screen.getByRole("button", { name: "Sign in with GitHub" })).toBeEnabled()
  expect(workspaces).toHaveBeenCalledTimes(2)
  expect(port.save).not.toHaveBeenCalled()
})

it("ignores a workspace list from a replaced grant", async () => {
  const { port, emit } = fixture({ draft: { screen: "identity", folders: [] }, grantId: "old" })
  const personal: CloudWorkspace = {
    id: "synthetic-personal",
    name: "Personal",
    kind: "personal",
  }
  port.openSetup = vi.fn()
  const workspaces = vi.fn<NonNullable<typeof port.workspaces>>().mockResolvedValue([personal])
  port.workspaces = workspaces
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  const next = await screen.findByRole("button", { name: "Continue" })
  await waitFor(() => expect(workspaces).toHaveBeenCalledOnce())

  let resolveWorkspaces!: (result: CloudWorkspace[]) => void
  workspaces.mockReturnValueOnce(new Promise((resolve) => (resolveWorkspaces = resolve)))
  fireEvent.click(next)
  await waitFor(() => expect(workspaces).toHaveBeenCalledTimes(2))
  act(() => emit({ grantId: "replacement" }))
  await act(async () => resolveWorkspaces([personal]))

  expect(port.save).not.toHaveBeenCalled()
  expect(next).toBeEnabled()
  expect(screen.queryByRole("button", { name: "Start syncing" })).not.toBeInTheDocument()
})

it("does not claim syncing when capture is unavailable on the platform", async () => {
  const { port, ongoing } = fixture({ captureEnabled: false })
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  expect(await screen.findByText("Uploads are unavailable on this device.")).toBeVisible()
  expect(screen.getByRole("button", { name: "Start syncing" })).toBeDisabled()
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it("names unsupported capture after a fresh connection without trying to save navigation", async () => {
  const { port, ongoing } = fixture({
    captureEnabled: false,
    setupError: "Desktop capture is not supported on this platform.",
    draft: { screen: "handoff", folders: [] },
  })
  const save = vi.spyOn(port, "save")
  const done = vi.fn()
  render(<CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={done} />)
  expect(
    await screen.findByText(/Desktop capture is not supported on this platform/),
  ).toBeVisible()
  fireEvent.click(screen.getByRole("button", { name: "Continue locally" }))
  expect(done).toHaveBeenCalledOnce()
  expect(save).not.toHaveBeenCalled()
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it("allows local completion while configured Cloud is offline", async () => {
  const { port, ongoing } = fixture({ status: "offline", identity: null, destination: null })
  const done = vi.fn()
  render(<CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={done} />)
  fireEvent.click(await screen.findByRole("button", { name: "Continue locally" }))
  expect(done).toHaveBeenCalledOnce()
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it("records a local choice without reporting upload consent", async () => {
  const { port, ongoing } = fixture()
  render(<CloudOnboarding port={port} onBack={vi.fn()} onComplete={vi.fn()} />)
  fireEvent.click(await screen.findByRole("button", { name: "Continue locally" }))
  expect(analytics.action).toHaveBeenCalledExactlyOnceWith("folders", "use_locally")
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it("cancels only the pending browser attempt before local completion", async () => {
  const { port, ongoing } = fixture({ status: "waiting", identity: null, destination: null })
  port.cancel = vi.fn(async () => ({
    ...emptyCloudSnapshot,
    authEnabled: true,
    status: "revocation_unconfirmed" as const,
    revocationPending: true,
  }))
  const done = vi.fn()
  render(<CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={done} />)
  expect(await screen.findByText(/Waiting for browser approval/)).toBeVisible()
  expect(screen.getByRole("button", { name: "Cancel sign-in" })).toBeEnabled()
  fireEvent.click(screen.getByRole("button", { name: "Continue locally" }))
  await waitFor(() => expect(done).toHaveBeenCalledOnce())
  expect(port.cancel).toHaveBeenCalledOnce()
  expect(ongoing.setEnabled).not.toHaveBeenCalled()
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it("waits for sign-in verification after the browser returns", async () => {
  const { port } = fixture({ status: "returned", identity: null, destination: null })
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  expect(await screen.findByText("Browser step complete. Checking your sign-in…")).toBeVisible()
  expect(screen.queryByRole("button", { name: "Sign in with GitHub" })).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Continue locally" })).toBeEnabled()
})

it("shows current Desktop sync status without submitting consent on reopen", async () => {
  const { port, ongoing, emitSync } = fixture({ syncEnabled: true })
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  await waitFor(() => expect(screen.getByRole("button", { name: "Done" })).toBeEnabled())
  act(() => emitSync(ongoingStatus({ status: "syncing", queuedCount: 2 })))
  expect(screen.getByRole("heading", { name: "Sessions sync in the background" })).toBeVisible()
  expect(screen.getByRole("button", { name: "Done" })).toBeEnabled()
  expect(ongoing.enable).not.toHaveBeenCalled()
  expect(ongoing.setProject).not.toHaveBeenCalled()
})
