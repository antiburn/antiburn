import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { expect, it, vi } from "vitest"
import {
  emptyCloudSnapshot,
  nativeCloudPort,
  type CloudPort,
  type CloudSnapshot,
} from "../lib/cloud"
import { CloudOnboarding } from "./CloudOnboarding"

const analytics = vi.hoisted(() => ({ action: vi.fn(), view: vi.fn() }))
vi.mock("../lib/onboardingAnalytics", () => ({
  noteCloudAction: analytics.action,
  noteCloudScreenViewed: analytics.view,
}))

function fixture(patch: Partial<CloudSnapshot> = {}) {
  let value: CloudSnapshot = {
    ...emptyCloudSnapshot,
    authEnabled: true,
    captureEnabled: true,
    status: "signed_in",
    identity: { id: "synthetic", login: "Example" },
    destination: { id: "personal", name: "Personal", kind: "personal" },
    draft: {
      screen: "folders",
      folders: [{ path: "/Example/claude/projects/project", selected: true }],
    },
    ...patch,
  }
  const start = vi.fn<CloudPort["start"]>(async () => ({ status: "upload_unavailable" }))
  const port: CloudPort = {
    ...nativeCloudPort,
    read: async () => value,
    watch: async () => () => {},
    agent: async () => "unavailable",
    prepareSync: vi.fn(async () => {}),
    start,
    reconcile: async () => value.pendingSync?.result ?? { status: "upload_unavailable" },
    save: async (draft) => {
      value = { ...value, draft }
      return value
    },
  }
  return { port, start }
}

it("saves native capture requests with truthful completion copy", async () => {
  const { port, start } = fixture()
  const done = vi.fn()
  render(<CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={done} />)
  const save = await screen.findByRole("button", { name: "Save capture request" })
  expect(save).toBeDisabled()
  expect(screen.getByText(/Choose native Claude projects\/<project>/)).toBeVisible()
  expect(screen.queryByText(/existing and future coding sessions/)).not.toBeInTheDocument()
  fireEvent.click(screen.getByRole("checkbox"))
  fireEvent.click(save)
  expect(await screen.findByRole("heading", { name: "Capture request saved" })).toBeVisible()
  expect(start).toHaveBeenCalledOnce()
  expect(screen.getByText("Upload unavailable")).toBeVisible()
  expect(screen.queryByText(/Sessions syncing/)).not.toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "Done" }))
  expect(done).toHaveBeenCalledOnce()
})

it("does not claim a native request was saved when its result is unknown", async () => {
  const { port } = fixture()
  vi.mocked(port.prepareSync).mockRejectedValue(new Error("save response lost"))
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  const save = await screen.findByRole("button", { name: "Save capture request" })
  fireEvent.click(screen.getByRole("checkbox"))
  fireEvent.click(save)
  await waitFor(() => expect(screen.getByText("Capture request not confirmed")).toBeVisible())
  expect(
    screen.queryByRole("heading", { name: "Capture request saved" }),
  ).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Done" })).toBeEnabled()
})

it("keeps the saved request visible and offers an explicit source-listing retry", async () => {
  const { port, start } = fixture()
  start.mockRejectedValueOnce(new Error("listing failed"))
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  const save = await screen.findByRole("button", { name: "Save capture request" })
  fireEvent.click(screen.getByRole("checkbox"))
  fireEvent.click(save)
  const retry = await screen.findByRole("button", { name: "Retry source listing" })
  expect(screen.getByRole("heading", { name: "Capture request saved" })).toBeVisible()
  expect(screen.getByText(/Could not finish listing sources/)).toBeVisible()
  fireEvent.click(retry)
  await waitFor(() => expect(start).toHaveBeenCalledTimes(2))
  expect(vi.mocked(port.prepareSync)).toHaveBeenCalledOnce()
})

it("keeps sync wording for simulated accepted operations", async () => {
  const { port, start } = fixture({ captureEnabled: false, syncEnabled: true })
  start.mockResolvedValue({
    status: "accepted",
    operationId: "simulated",
    completed: 0,
    total: 1,
  })
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  const sync = await screen.findByRole("button", { name: "Start syncing" })
  fireEvent.click(screen.getByRole("checkbox", { name: /agree to upload/ }))
  fireEvent.click(sync)
  expect(
    await screen.findByRole("heading", { name: "Sessions syncing! Your agent is ready" }),
  ).toBeVisible()
  expect(screen.getByText("Syncing 1 folder")).toBeVisible()
  expect(screen.queryByText("Upload unavailable")).not.toBeInTheDocument()
  expect(screen.queryByText(/not repositories/)).not.toBeInTheDocument()
})

it("does not claim syncing when native capture is unavailable on the platform", async () => {
  const { port } = fixture({
    captureEnabled: false,
    syncEnabled: false,
    setupError: "Native capture is not supported on this platform.",
    pendingSync: { requestId: "saved", result: { status: "upload_unavailable" } },
  })
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  expect(
    await screen.findByRole("heading", { name: "Continue using Antiburn locally" }),
  ).toBeVisible()
  expect(screen.getByText("Native capture is not supported on this platform.")).toBeVisible()
  expect(screen.queryByText(/Sessions syncing/)).not.toBeInTheDocument()
})

it.each([{ draft: { screen: "folders" as const, folders: [] } }, {}])(
  "allows local completion without source or content consent: %j",
  async (patch) => {
    const { port, start } = fixture(patch)
    const done = vi.fn()
    render(<CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={done} />)
    const local = await screen.findByRole("button", { name: "Continue locally" })
    fireEvent.click(local)
    expect(done).toHaveBeenCalledOnce()
    expect(port.prepareSync).not.toHaveBeenCalled()
    expect(start).not.toHaveBeenCalled()
  },
)

it("names unsupported capture after a fresh connection without trying to save navigation", async () => {
  const { port, start } = fixture({
    captureEnabled: false,
    setupError: "Native capture is not supported on this platform.",
    draft: { screen: "handoff", folders: [] },
  })
  port.save = vi.fn()
  const done = vi.fn()
  render(<CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={done} />)
  expect(
    await screen.findByText(/Native capture is not supported on this platform/),
  ).toBeVisible()
  fireEvent.click(screen.getByRole("button", { name: "Continue locally" }))
  expect(done).toHaveBeenCalledOnce()
  expect(port.save).not.toHaveBeenCalled()
  expect(start).not.toHaveBeenCalled()
})

it("allows local completion while configured Cloud is offline", async () => {
  const { port } = fixture({ status: "offline", identity: null, destination: null })
  const done = vi.fn()
  render(<CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={done} />)
  fireEvent.click(await screen.findByRole("button", { name: "Continue locally" }))
  expect(done).toHaveBeenCalledOnce()
  expect(port.prepareSync).not.toHaveBeenCalled()
})

it("records a local choice without reporting request or upload success", async () => {
  const { port } = fixture()
  render(<CloudOnboarding port={port} onBack={vi.fn()} onComplete={vi.fn()} />)
  fireEvent.click(await screen.findByRole("button", { name: "Continue locally" }))
  expect(analytics.action).toHaveBeenCalledExactlyOnceWith("folders", "use_locally")
  expect(port.prepareSync).not.toHaveBeenCalled()
})

it("cancels only the pending browser attempt before local completion", async () => {
  const { port } = fixture({ status: "waiting", identity: null, destination: null })
  port.cancel = vi.fn(async () => ({
    ...emptyCloudSnapshot,
    authEnabled: true,
    status: "revocation_unconfirmed" as const,
    revocationPending: true,
  }))
  port.cancelSync = vi.fn()
  const done = vi.fn()
  render(<CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={done} />)
  fireEvent.click(await screen.findByRole("button", { name: "Continue locally" }))
  await waitFor(() => expect(done).toHaveBeenCalledOnce())
  expect(port.cancel).toHaveBeenCalledOnce()
  expect(port.cancelSync).not.toHaveBeenCalled()
  expect(port.prepareSync).not.toHaveBeenCalled()
})
