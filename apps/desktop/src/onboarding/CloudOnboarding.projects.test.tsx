import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { expect, it, vi } from "vitest"
import type { CloudOngoingStatus } from "../lib/cloud"
import { CloudOnboarding } from "./CloudOnboarding"
import {
  cloudProjectFixture,
  ongoingStatus,
  projectGroup,
} from "./__tests__/cloudProjectFixtures"

it("uses grouped desktop projects and separate ongoing consent in production setup", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const done = vi.fn()
  render(<CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={done} />)
  expect(
    await screen.findByRole("switch", { name: "Sync Studio: /Example/Studio" }),
  ).toBeChecked()
  expect(screen.getByText("1 worktree")).toBeVisible()
  expect(screen.queryByText("/Example/claude/projects/studio")).not.toBeInTheDocument()
  const start = screen.getByRole("button", { name: "Start syncing" })
  expect(start).toBeDisabled()
  fireEvent.click(screen.getByRole("checkbox"))
  expect(start).toBeEnabled()
  fireEvent.click(start)
  expect(await screen.findByText("Background sync enabled")).toBeVisible()
  expect(ongoing.enable).toHaveBeenCalledOnce()
  expect(port.prepareSync).not.toHaveBeenCalled()
  expect(port.start).not.toHaveBeenCalled()
  expect(screen.getByText("Local agent setup unavailable")).toBeVisible()
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
  fireEvent.click(screen.getByRole("checkbox"))
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
  fireEvent.click(screen.getByRole("checkbox"))
  fireEvent.click(screen.getByRole("button", { name: "Start syncing" }))
  await waitFor(() => expect(ongoing.enable).toHaveBeenCalledOnce())
  act(() => emit({ syncEnabled: false }))
  expect(screen.getByRole("button", { name: "Done" })).toBeEnabled()
  expect(screen.queryByRole("button", { name: "Start syncing" })).not.toBeInTheDocument()
  expect(screen.queryByRole("alert")).not.toBeInTheDocument()
})

it("requests another bounded page only after Load more folders and clears old confirmation", async () => {
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
  fireEvent.click(screen.getByRole("checkbox"))
  expect(screen.getByRole("checkbox")).toBeChecked()
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
  expect(screen.getByRole("checkbox")).not.toBeChecked()
  expect(screen.getByText(/Some session folders could not be checked/)).toBeVisible()
  expect(ongoing.enable).not.toHaveBeenCalled()
})
