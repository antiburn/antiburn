import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, expect, it, vi } from "vitest"
import { emptyCloudSnapshot, type CloudSnapshot, type CloudTeamJoinResult } from "../lib/cloud"
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

it("offers Team joining after sign-in without requiring it to continue", async () => {
  const { port, ongoing } = fixture({
    dashboardAvailable: true,
    teamJoinAllowed: true,
    draft: { screen: "identity", folders: [] },
  })
  const join = vi.fn()
  port.teamJoin = join
  const done = vi.fn()
  render(<CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={done} />)
  fireEvent.click(await screen.findByRole("button", { name: "Join a Team" }))
  expect(screen.getByRole("textbox", { name: "Team invite link" })).toHaveFocus()
  expect(screen.getByRole("button", { name: "Continue Cloud setup" })).toBeEnabled()
  fireEvent.click(screen.getByRole("button", { name: "Continue locally" }))
  expect(done).toHaveBeenCalledOnce()
  expect(join).not.toHaveBeenCalled()
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it("keeps onboarding on the Team join until its pending claim is confirmed", async () => {
  const { port, ongoing } = fixture({
    dashboardAvailable: true,
    teamJoinAllowed: true,
    draft: { screen: "identity", folders: [] },
  })
  let resolveClaim!: (result: CloudTeamJoinResult) => void
  const claim = new Promise<CloudTeamJoinResult>((resolve) => {
    resolveClaim = resolve
  })
  const teamName = "Example Studio"
  port.teamJoin = vi
    .fn<NonNullable<typeof port.teamJoin>>()
    .mockResolvedValueOnce({ status: "started", attemptId: "example-attempt" })
    .mockResolvedValueOnce({
      status: "ready",
      attemptId: "example-attempt",
      teamName,
      email: "member@example.com",
      emailId: "example-email",
    })
    .mockReturnValueOnce(claim)
  const save = vi.spyOn(port, "save")
  const done = vi.fn()
  render(<CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={done} />)
  fireEvent.click(await screen.findByRole("button", { name: "Join a Team" }))
  fireEvent.change(screen.getByRole("textbox", { name: "Team invite link" }), {
    target: { value: `https://dashboard.example/join#token=${"a".repeat(43)}` },
  })
  fireEvent.click(screen.getByRole("button", { name: "Check invite" }))
  fireEvent.click(await screen.findByRole("button", { name: "Join Team" }))

  for (const name of ["Continue locally", "Continue Cloud setup"]) {
    const control = screen.getByRole("button", { name })
    expect(control).toBeDisabled()
    fireEvent.click(control)
  }
  expect(done).not.toHaveBeenCalled()
  expect(save).not.toHaveBeenCalled()

  await act(async () => resolveClaim({ status: "joined", teamName, inSetup: false }))
  expect(await screen.findByText("You joined Example Studio.")).toBeVisible()
  expect(screen.getByRole("button", { name: "Continue locally" })).toBeEnabled()
  expect(screen.getByRole("button", { name: "Continue Cloud setup" })).toBeEnabled()
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it("does not claim syncing when capture is unavailable on the platform", async () => {
  const { port, ongoing } = fixture({ captureEnabled: false })
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  expect(await screen.findByText("Uploads are unavailable on this device.")).toBeVisible()
  expect(screen.getByRole("button", { name: "Start syncing" })).toBeDisabled()
  expect(screen.queryByRole("button", { name: "Save capture request" })).not.toBeInTheDocument()
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
