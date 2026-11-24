import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { useSyncExternalStore } from "react"
import { expect, it, vi } from "vitest"
import type { CloudOngoingStatus, CloudSnapshot } from "../../lib/cloud"
import { CloudSession } from "../../onboarding/CloudSession"
import {
  cloudProjectFixture,
  ongoingStatus,
  projectGroup,
} from "../../onboarding/__tests__/cloudProjectFixtures"
import { CloudAccount } from "./CloudAccount"

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (error: Error) => void
  const promise = new Promise<T>((done, fail) => {
    resolve = done
    reject = fail
  })
  return { promise, resolve, reject }
}

function Account({ session }: { session: CloudSession }) {
  const value = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
    session.getSnapshot,
  )
  return (
    <CloudAccount session={session} value={value} folderRowId="cloud-loading-folders">
      <p role="status">Saved upload settings</p>
    </CloudAccount>
  )
}

function loadingStatus() {
  const status = screen.getByText("Loading Cloud settings…")
  expect(status).toHaveAttribute("role", "status")
  expect(screen.getAllByRole("status")).toEqual([status])
  expect(status.closest('[aria-busy="true"]')).toBeNull()
  const footer = status.closest("footer")!
  expect(within(footer).getByRole("button", { name: "Sync now" })).toBeDisabled()
  return status
}

it("reserves account, upload, and folder rows until the first account read resolves", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const healthy = await port.read()
  const account = deferred<CloudSnapshot>()
  const policy = deferred<CloudOngoingStatus | null>()
  port.read = vi.fn(() => account.promise)
  ongoing.status.mockReturnValue(policy.promise)
  const { container } = render(<Account session={new CloudSession(port, false, true)} />)

  loadingStatus()
  const body = container.querySelector<HTMLElement>('[aria-busy="true"]')!
  const folders = screen.getByRole("group", { name: "Cloud folders" })
  expect(body).toContainElement(folders)
  expect(folders).toHaveAttribute("id", "cloud-loading-folders")
  const accountRow = body.firstElementChild!
  expect(accountRow).not.toHaveAttribute("data-settings-control")
  expect(accountRow.querySelector("[data-placeholder]")).not.toBeNull()
  const uploadRow = body.querySelector('[data-settings-control="cloudUpload"]')!
  expect(uploadRow.querySelector("[data-placeholder]")).not.toBeNull()
  expect(folders.querySelector("[data-placeholder]")).not.toBeNull()
  const placeholders = body.querySelectorAll("[data-placeholder]")
  expect(placeholders.length).toBeGreaterThan(0)
  for (const placeholder of placeholders)
    expect(placeholder).toHaveAttribute("aria-hidden", "true")
  expect(within(body).queryByRole("button")).not.toBeInTheDocument()
  expect(screen.queryByRole("switch")).not.toBeInTheDocument()
  expect(screen.queryByText("Checking Cloud sign-in…")).not.toBeInTheDocument()
  expect(screen.queryByText("Antiburn Cloud is coming soon")).not.toBeInTheDocument()
  expect(screen.queryByText("Saved upload settings")).not.toBeInTheDocument()
  expect(screen.queryByRole("alert")).not.toBeInTheDocument()

  await act(async () => account.resolve(healthy))
  const identity = await screen.findByText("Signed in as Example")
  expect(identity.closest('[aria-busy="true"]')).toBeNull()
  expect(screen.getByRole("button", { name: "Sign out" })).toBeVisible()
  loadingStatus()
  expect(screen.queryByText("Saved upload settings")).not.toBeInTheDocument()

  await act(async () => policy.resolve(ongoingStatus()))
  expect(await screen.findByRole("switch", { name: /Sync Studio/ })).toBeVisible()
  expect(screen.getByText("Signed in as Example")).toBe(identity)
  expect(screen.getByText("Saved upload settings")).toBeVisible()
  expect(screen.getByRole("button", { name: "Start syncing" })).toBeEnabled()
  expect(screen.queryByText("Loading Cloud settings…")).not.toBeInTheDocument()
  expect(container.querySelector('[aria-busy="true"]')).toBeNull()
  expect(container.querySelector("[data-placeholder]")).toBeNull()
})

it("keeps provisional folders hidden while the first upload-policy read is pending", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const policy = deferred<CloudOngoingStatus | null>()
  ongoing.status.mockReturnValue(policy.promise)
  const session = new CloudSession(port, false, true)
  const { container } = render(<Account session={session} />)

  await waitFor(() => expect(session.getSnapshot().projects?.groups).toHaveLength(1))
  expect(session.getSnapshot().projects?.loaded).toBe(false)
  expect(session.getSnapshot().ongoing).toBeNull()
  expect(screen.getByText("Signed in as Example")).toBeVisible()
  loadingStatus()
  expect(screen.queryByText("Studio", { selector: "strong" })).not.toBeInTheDocument()
  expect(screen.queryByRole("switch")).not.toBeInTheDocument()
  expect(screen.queryByText("Saved upload settings")).not.toBeInTheDocument()
  expect(screen.queryByText(/No session folders found/)).not.toBeInTheDocument()
  expect(screen.queryByRole("alert")).not.toBeInTheDocument()

  await act(async () => policy.resolve(ongoingStatus()))
  expect(await screen.findByRole("switch", { name: /Sync Studio/ })).toBeVisible()
  expect(screen.getByText("Saved upload settings")).toBeVisible()
  expect(screen.queryByText("Loading Cloud settings…")).not.toBeInTheDocument()
  expect(container.querySelector("[data-placeholder]")).toBeNull()
  expect(ongoing.enable).not.toHaveBeenCalled()
  expect(ongoing.setProject).not.toHaveBeenCalled()
})

it("replaces account placeholders with an error and a working account retry", async () => {
  vi.useFakeTimers()
  try {
    const { port, ongoing } = cloudProjectFixture()
    const healthy = await port.read()
    const account = deferred<CloudSnapshot>()
    const read = vi.fn(() => account.promise)
    port.read = read
    const { container } = render(<Account session={new CloudSession(port, false, true)} />)
    await act(async () => {})
    expect(read).toHaveBeenCalledOnce()
    loadingStatus()

    await act(async () => {
      account.reject(new Error("Synthetic account read failure"))
      await vi.advanceTimersByTimeAsync(550)
    })
    expect(read).toHaveBeenCalledTimes(3)
    expect(screen.getByRole("alert")).toHaveTextContent("Could not check Cloud sign-in.")
    expect(screen.queryByText("Loading Cloud settings…")).not.toBeInTheDocument()
    expect(container.querySelector("[data-placeholder]")).toBeNull()
    expect(ongoing.status).not.toHaveBeenCalled()

    const retry = screen.getByRole("button", { name: "Check again" })
    expect(retry).toBeEnabled()
    read.mockResolvedValue(healthy)
    await act(async () => fireEvent.click(retry))
    expect(screen.getByText("Signed in as Example")).toBeVisible()
    expect(screen.queryByRole("alert")).not.toBeInTheDocument()
    expect(screen.queryByText("Loading Cloud settings…")).not.toBeInTheDocument()
    expect(screen.getByRole("switch", { name: /Sync Studio/ })).toBeVisible()
  } finally {
    vi.useRealTimers()
  }
})

it.each([
  {
    failure: "Synthetic upload-policy read failure",
    message: "Could not check Cloud setup or workspaces.",
    attempts: 5,
  },
  {
    failure: "Cloud workspace access denied.",
    message: "You do not have access to the selected Cloud workspace.",
    attempts: 1,
  },
])(
  "replaces pending upload settings with a retry after $failure",
  async ({ failure, message, attempts }) => {
    vi.useFakeTimers()
    try {
      const { port, ongoing } = cloudProjectFixture()
      const policy = deferred<CloudOngoingStatus | null>()
      ongoing.status.mockReturnValue(policy.promise)
      const { container } = render(<Account session={new CloudSession(port, false, true)} />)
      await act(async () => {})
      expect(ongoing.status).toHaveBeenCalledOnce()
      loadingStatus()

      await act(async () => {
        policy.reject(new Error(failure))
        await vi.advanceTimersByTimeAsync(3550)
      })
      expect(ongoing.status).toHaveBeenCalledTimes(attempts)
      expect(screen.getByText("Signed in as Example")).toBeVisible()
      expect(screen.getByRole("alert")).toHaveTextContent(message)
      expect(screen.queryByText("Loading Cloud settings…")).not.toBeInTheDocument()
      expect(container.querySelector('[aria-busy="true"]')).toBeNull()
      expect(container.querySelector("[data-placeholder]")).toBeNull()

      const retry = screen.getByRole("button", { name: "Check again" })
      expect(retry).toBeEnabled()
      ongoing.status.mockResolvedValue(ongoingStatus())
      await act(async () => fireEvent.click(retry))
      expect(screen.getByRole("switch", { name: /Sync Studio/ })).toBeVisible()
      expect(screen.queryByRole("alert")).not.toBeInTheDocument()
      expect(screen.queryByText("Loading Cloud settings…")).not.toBeInTheDocument()
    } finally {
      vi.useRealTimers()
    }
  },
)

it("keeps the account, upload settings, and saved folders visible during a refresh", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const saved = ongoingStatus({
    status: "ready",
    consentVersion: "desktop-project-sync-v1",
    deviceUploadEnabled: true,
    projects: projectGroup().projects,
  })
  ongoing.status.mockResolvedValue(saved)
  const session = new CloudSession(port, false, true)
  const { container } = render(<Account session={session} />)
  const folder = await screen.findByRole("switch", { name: /Sync Studio/ })
  await waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
  const identity = screen.getByText("Signed in as Example")
  const upload = screen.getByText("Saved upload settings")
  const sync = screen.getByRole("button", { name: "Sync now" })
  const policy = deferred<CloudOngoingStatus | null>()
  ongoing.status.mockReturnValueOnce(policy.promise)
  let refresh!: Promise<void>

  act(() => {
    refresh = session.load()
  })
  await waitFor(() => expect(ongoing.status).toHaveBeenCalledTimes(2))
  expect(session.getSnapshot().ongoing).toBe(saved)
  expect(screen.getByText("Signed in as Example")).toBe(identity)
  expect(screen.getByText("Saved upload settings")).toBe(upload)
  expect(screen.getByRole("switch", { name: /Sync Studio/ })).toBe(folder)
  expect(screen.getByRole("button", { name: "Sync now" })).toBe(sync)
  expect(screen.queryByText("Loading Cloud settings…")).not.toBeInTheDocument()
  expect(container.querySelector('[aria-busy="true"]')).toBeNull()
  expect(container.querySelector("[data-placeholder]")).toBeNull()

  await act(async () => {
    policy.resolve(saved)
    await refresh
  })
  expect(screen.getByRole("switch", { name: /Sync Studio/ })).toBe(folder)
  expect(container.querySelector("[data-placeholder]")).toBeNull()
})
