import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { expect, it, vi } from "vitest"
import type { CloudOngoingStatus } from "../lib/cloud"
import { CloudOnboarding } from "./CloudOnboarding"
import {
  cloudProjectFixture,
  ongoingStatus,
  projectGroup,
} from "./__tests__/cloudProjectFixtures"

it("uses Start as explicit grouped consent without saving row defaults", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const done = vi.fn()
  render(<CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={done} />)
  expect(
    await screen.findByRole("switch", { name: "Sync Studio: /Example/Studio" }),
  ).toBeChecked()
  expect(screen.getByText("1 worktree")).toBeVisible()
  expect(screen.queryByText("/Example/claude/projects/studio")).not.toBeInTheDocument()
  const start = screen.getByRole("button", { name: "Start syncing" })
  expect(screen.queryByRole("checkbox")).not.toBeInTheDocument()
  expect(ongoing.enable).not.toHaveBeenCalled()
  expect(start).toHaveAccessibleDescription(expect.stringContaining("Sync full sessions"))
  expect(screen.getByText("1 folder selected", { exact: true })).toBeVisible()
  expect(document.querySelector("footer")).not.toContainElement(
    screen.getByText("1 folder selected"),
  )
  expect(document.querySelector("footer")?.querySelectorAll("p")).toHaveLength(0)
  expect(screen.getByRole("button", { name: "Continue locally" })).toHaveClass("underline")
  expect(start).toBeEnabled()
  fireEvent.click(start)
  expect(await screen.findByText("Background sync enabled")).toBeVisible()
  expect(ongoing.enable).toHaveBeenCalledOnce()
  expect(port.prepareSync).not.toHaveBeenCalled()
  expect(port.start).not.toHaveBeenCalled()
  expect(screen.getByText("Local tools unavailable")).toBeVisible()
  expect(screen.queryByRole("progressbar")).not.toBeInTheDocument()
  expect(screen.queryByRole("button", { name: /Retry|Remove/ })).not.toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "Done" }))
  expect(done).toHaveBeenCalledOnce()
})

it("keeps Done available while the desktop enable command is pending", async () => {
  const { port, ongoing } = cloudProjectFixture()
  let finish!: (value: CloudOngoingStatus) => void
  ongoing.enable.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve
      }),
  )
  const done = vi.fn()
  const { unmount } = render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={done} />,
  )
  await screen.findByRole("switch")
  fireEvent.click(screen.getByRole("button", { name: "Start syncing" }))
  expect(screen.getByText("Starting background sync")).toBeVisible()
  fireEvent.click(screen.getByRole("button", { name: "Done" }))
  expect(done).toHaveBeenCalledOnce()
  unmount()
  finish(ongoingStatus({ status: "ready", consentVersion: "desktop-project-sync-v1" }))
  await Promise.resolve()
  expect(ongoing.enable).toHaveBeenCalledOnce()
})

it("keeps an unconfirmed enable on completion when later desktop account snapshots arrive", async () => {
  const { port, ongoing, emit } = cloudProjectFixture()
  ongoing.enable.mockRejectedValueOnce(new Error("Synthetic unknown result"))
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  await screen.findByRole("switch")
  fireEvent.click(screen.getByRole("button", { name: "Start syncing" }))
  await waitFor(() => expect(ongoing.enable).toHaveBeenCalledOnce())
  act(() => emit({ syncEnabled: false }))
  expect(screen.getByRole("button", { name: "Done" })).toBeEnabled()
  expect(screen.queryByRole("button", { name: "Start syncing" })).not.toBeInTheDocument()
  expect(screen.queryByRole("alert")).not.toBeInTheDocument()
})

it("loads another page without approving new members until Start", async () => {
  const { port, ongoing } = cloudProjectFixture()
  ongoing.groups.mockResolvedValueOnce({
    groups: [projectGroup()],
    sourceRoots: [{ sourceKind: "claude_jsonl", path: "/Example/claude/projects/studio" }],
    nextCursor: { environment: "native", cwd: "/Example/next" },
    partial: false,
  })
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  await screen.findByRole("switch")
  expect(ongoing.groups).toHaveBeenCalledTimes(1)
  ongoing.groups.mockResolvedValueOnce({
    groups: [
      projectGroup({
        folderPaths: [...projectGroup().folderPaths, "/Example/external/studio-review"],
        worktreeCount: 2,
        projects: [
          ...projectGroup().projects,
          {
            sourceKind: "claude_jsonl",
            sourcePath: "/Example/claude/projects/studio",
            projectPath: "/Example/external/studio-review",
          },
        ],
      }),
    ],
    sourceRoots: [{ sourceKind: "claude_jsonl", path: "/Example/claude/projects/studio" }],
    nextCursor: null,
    partial: true,
  })
  fireEvent.click(screen.getByRole("button", { name: "Load more folders" }))
  expect(await screen.findByText("2 worktrees")).toBeVisible()
  expect(screen.getAllByRole("switch")).toHaveLength(1)
  expect(screen.queryByRole("checkbox")).not.toBeInTheDocument()
  expect(screen.queryByText(/Some folders could not be checked/)).not.toBeInTheDocument()
  expect(ongoing.enable).not.toHaveBeenCalled()
  fireEvent.click(screen.getByRole("button", { name: "Start syncing" }))
  await waitFor(() => expect(ongoing.enable).toHaveBeenCalledOnce())
  expect(ongoing.enable).toHaveBeenCalledWith(
    expect.objectContaining({
      projects: expect.arrayContaining([
        expect.objectContaining({ projectPath: "/Example/external/studio-review" }),
      ]),
    }),
  )
})

it("does not warn when bounded discovery still has pages to check", async () => {
  const { port, ongoing } = cloudProjectFixture()
  ongoing.groups.mockResolvedValue({
    groups: [projectGroup({ captureSupported: false, projects: [] })],
    sourceRoots: [],
    nextCursor: { environment: "native", cwd: "/Example/next" },
    partial: true,
  })
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  await screen.findByRole("switch")
  expect(screen.getByRole("button", { name: "Load more folders" })).toBeVisible()
  expect(screen.queryByText(/Could not check uploads/)).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Start syncing" })).toBeDisabled()
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it.each([
  { partial: true, nextCursor: null },
  { partial: false, nextCursor: { environment: "native", cwd: "/Example/Next" } },
])("keeps an incomplete empty folder page neutral: %j", async (page) => {
  const { port, ongoing } = cloudProjectFixture()
  ongoing.groups.mockResolvedValueOnce({ groups: [], sourceRoots: [], ...page })
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  expect(await screen.findByText("Folder discovery is incomplete.")).toBeVisible()
  expect(screen.queryByText(/No project folders found/)).not.toBeInTheDocument()
  expect(screen.queryByRole("button", { name: "Refresh folders" })).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Start syncing" })).toBeDisabled()
  expect(screen.getByRole("button", { name: "Continue locally" })).toBeEnabled()
  expect(document.querySelector("footer")?.querySelectorAll("p")).toHaveLength(0)
  if (page.nextCursor) {
    let finish!: (page: Awaited<ReturnType<typeof ongoing.groups>>) => void
    ongoing.groups.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    fireEvent.click(screen.getByRole("button", { name: "Load more folders" }))
    expect(await screen.findByText("Loading project folders…")).toBeVisible()
    expect(screen.queryByText(/No project folders found/)).not.toBeInTheDocument()
    await act(async () =>
      finish({
        groups: [projectGroup()],
        sourceRoots: [{ sourceKind: "claude_jsonl", path: "/Example/claude/projects/studio" }],
        nextCursor: null,
        partial: false,
      }),
    )
    expect(await screen.findByRole("switch", { name: /Sync Studio/ })).toBeChecked()
  }
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it("shows a complete empty result only after its pending discovery finishes", async () => {
  const { port, ongoing } = cloudProjectFixture()
  let finish!: (page: Awaited<ReturnType<typeof ongoing.groups>>) => void
  ongoing.groups.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve
      }),
  )
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  expect(await screen.findByText("Loading project folders…")).toBeVisible()
  expect(screen.queryByText(/No project folders found/)).not.toBeInTheDocument()
  await waitFor(() => expect(ongoing.groups).toHaveBeenCalledOnce())
  await act(async () =>
    finish({
      groups: [],
      sourceRoots: [],
      nextCursor: null,
      partial: false,
    }),
  )
  expect(
    await screen.findByText("No project folders found in your available sessions."),
  ).toBeVisible()
  expect(screen.getByRole("button", { name: "Start syncing" })).toBeDisabled()
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it("offers failure recovery instead of claiming an empty folder result", async () => {
  const { port, ongoing } = cloudProjectFixture()
  ongoing.groups.mockRejectedValueOnce(new Error("Synthetic discovery failure"))
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  expect(
    await screen.findByText("Could not load project folders. Try again in Cloud Settings."),
  ).toBeVisible()
  expect(screen.queryByText(/No project folders found/)).not.toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "Refresh folders" }))
  expect(await screen.findByRole("switch", { name: /Sync Studio/ })).toBeChecked()
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it("reloads account setup before retrying discovery after a setup failure", async () => {
  const { port, ongoing, emit } = cloudProjectFixture()
  const healthy = await port.read()
  let finish!: (page: Awaited<ReturnType<typeof ongoing.groups>>) => void
  ongoing.groups.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve
      }),
  )
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  await waitFor(() => expect(ongoing.groups).toHaveBeenCalledOnce())
  act(() => emit({ setupError: "Synthetic setup failure" }))
  expect(screen.getByText("Synthetic setup failure")).toBeVisible()
  const retry = screen.getByRole("button", { name: "Refresh folders" })
  expect(retry).toBeEnabled()
  vi.mocked(port.read).mockResolvedValue(healthy)
  fireEvent.click(retry)
  expect(await screen.findByRole("switch", { name: /Sync Studio/ })).toBeChecked()
  await act(async () =>
    finish({
      groups: [],
      sourceRoots: [],
      nextCursor: null,
      partial: false,
    }),
  )
  expect(screen.getByRole("switch", { name: /Sync Studio/ })).toBeChecked()
  expect(screen.queryByText(/No project folders found/)).not.toBeInTheDocument()
  expect(ongoing.groups).toHaveBeenCalledTimes(2)
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it("shows one refresh action for unqualified folders after a complete check", async () => {
  const { port, ongoing } = cloudProjectFixture()
  ongoing.groups.mockResolvedValue({
    groups: [projectGroup({ captureSupported: false, projects: [] })],
    sourceRoots: [],
    nextCursor: null,
    partial: false,
  })
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  expect(await screen.findByText("Could not check uploads for some folders.")).toBeVisible()
  expect(screen.getAllByText("Could not check uploads for some folders.")).toHaveLength(1)
  expect(screen.getByRole("button", { name: "Refresh folders" })).toBeEnabled()
  expect(screen.getByRole("button", { name: "Start syncing" })).toBeDisabled()
  expect(document.querySelector("footer")?.querySelectorAll("p")).toHaveLength(0)
})
