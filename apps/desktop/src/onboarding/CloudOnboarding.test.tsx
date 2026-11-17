import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { expect, it, vi } from "vitest"
import {
  emptyCloudSnapshot,
  nativeCloudPort,
  type CloudPort,
  type CloudSnapshot,
} from "../lib/cloud"
import { CloudOnboarding } from "./CloudOnboarding"

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
