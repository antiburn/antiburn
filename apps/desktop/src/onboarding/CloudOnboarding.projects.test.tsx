import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { expect, it, vi } from "vitest"
import type { CloudOngoingStatus, CloudWorkspace } from "../lib/cloud"
import { CloudOnboarding } from "./CloudOnboarding"
import {
  cloudProjectFixture,
  ongoingStatus,
  projectGroup,
} from "./__tests__/cloudProjectFixtures"

const personal = { id: "synthetic-personal", name: "Personal", kind: "personal" } as const
const team = { id: "synthetic-team", name: "Example Team", kind: "team" } as const

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

it("requires a folder destination and distinguishes Personal from a Team with the same name", async () => {
  const { port, ongoing } = cloudProjectFixture()
  port.workspaces = vi.fn(async () => [personal, { ...team, name: personal.name }])
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  const workspace = await screen.findByRole("combobox", {
    name: "Sync destination for Studio: /Example/Studio",
  })
  const start = screen.getByRole("button", { name: "Start syncing" })
  expect(workspace).toHaveValue("")
  expect(screen.getByText(/No sync destination selected/)).toBeVisible()
  expect(screen.getByRole("option", { name: "Personal workspace" })).toBeEnabled()
  expect(start).toBeDisabled()
  expect(screen.getByRole("option", { name: "Team workspace: Personal" })).toBeEnabled()
  fireEvent.change(workspace, { target: { value: "synthetic-team" } })
  expect(ongoing.enable).not.toHaveBeenCalled()
  expect(start).toHaveAccessibleDescription(
    expect.stringContaining("Sync destination: Team workspace: Personal"),
  )
  fireEvent.change(workspace, { target: { value: "synthetic-personal" } })
  expect(start).toHaveAccessibleDescription(
    expect.stringContaining("Sync destination: Personal workspace"),
  )
  expect(start).toBeEnabled()
  fireEvent.click(start)
  await waitFor(() =>
    expect(ongoing.enable).toHaveBeenCalledWith(
      expect.objectContaining({
        destinationId: "synthetic-personal",
        projectChoices: { studio: true },
      }),
    ),
  )
})

it("keeps the folder switch for one verified workspace", async () => {
  const { port, ongoing } = cloudProjectFixture()
  port.workspaces = vi.fn(async () => [personal])
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  expect(
    await screen.findByRole("switch", { name: "Sync Studio: /Example/Studio" }),
  ).toBeChecked()
  expect(screen.getByText(/Sync destination: Personal workspace/)).toBeVisible()
  expect(screen.queryByRole("combobox")).not.toBeInTheDocument()
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it("uses the existing account to save folder consent only for the chosen Team", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const begin = vi.spyOn(port, "begin")
  port.workspaces = vi.fn(async () => [personal, team])
  ongoing.enable.mockResolvedValueOnce(
    ongoingStatus({
      status: "ready",
      destinationId: "synthetic-team",
      consentVersion: "desktop-project-sync-v1",
    }),
  )
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  fireEvent.change(
    await screen.findByRole("combobox", { name: /Sync destination for Studio/ }),
    {
      target: { value: "synthetic-team" },
    },
  )
  expect(ongoing.enable).not.toHaveBeenCalled()
  expect(screen.getByText(/Sync destination: Team workspace: Example Team/)).toBeVisible()
  const start = screen.getByRole("button", { name: "Start syncing" })
  await waitFor(() => expect(start).toBeEnabled())
  fireEvent.click(start)
  expect(await screen.findByText("Background sync enabled")).toBeVisible()
  expect(begin).not.toHaveBeenCalled()
  expect(screen.queryByRole("button", { name: "Start syncing" })).not.toBeInTheDocument()
  expect(ongoing.enable).toHaveBeenCalledExactlyOnceWith(
    expect.objectContaining({
      destinationId: "synthetic-team",
      projects: projectGroup().projects,
    }),
  )
})

it("explains a mixed draft and starts only after the user narrows it to one workspace", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const docsProject = {
    sourceKind: "claude_jsonl" as const,
    sourcePath: "/Example/claude/projects/docs",
    projectPath: "/Example/Docs",
  }
  ongoing.groups.mockResolvedValueOnce({
    groups: [
      projectGroup(),
      projectGroup({
        id: "docs",
        label: "Docs",
        path: docsProject.projectPath,
        folderPaths: [docsProject.projectPath],
        worktreeCount: 0,
        projects: [docsProject],
      }),
    ],
    sourceRoots: [
      { sourceKind: "claude_jsonl", path: "/Example/claude/projects/studio" },
      { sourceKind: docsProject.sourceKind, path: docsProject.sourcePath },
    ],
    nextCursor: null,
    partial: false,
  })
  port.workspaces = vi.fn(async () => [personal, team])
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )

  const studio = await screen.findByRole("combobox", { name: /Sync destination for Studio/ })
  const docs = screen.getByRole("combobox", { name: /Sync destination for Docs/ })
  const start = screen.getByRole("button", { name: "Start syncing" })
  expect(studio).toHaveValue("")
  expect(docs).toHaveValue("")
  fireEvent.change(studio, { target: { value: personal.id } })
  fireEvent.change(docs, { target: { value: team.id } })
  await waitFor(() => expect(docs).toBeEnabled())
  expect(
    screen.getByText(/Sync destinations: Personal workspace · Team workspace: Example Team/),
  ).toBeVisible()
  expect(screen.getByText(/Set folders for any other workspace to Don't sync/)).toBeVisible()
  expect(start).toBeDisabled()
  expect(ongoing.enable).not.toHaveBeenCalled()

  fireEvent.change(docs, { target: { value: "" } })
  expect(start).toBeEnabled()
  fireEvent.click(start)
  await waitFor(() =>
    expect(ongoing.enable).toHaveBeenCalledExactlyOnceWith(
      expect.objectContaining({
        destinationId: personal.id,
        projects: projectGroup().projects,
      }),
    ),
  )
})

it("keeps Start disabled while workspace availability is loading or missing", async () => {
  const { port, ongoing } = cloudProjectFixture()
  let finish!: (workspaces: CloudWorkspace[]) => void
  port.workspaces = vi.fn(
    () =>
      new Promise<CloudWorkspace[]>((resolve) => {
        finish = resolve
      }),
  )
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  expect(await screen.findByText(/Checking workspace destinations/)).toBeVisible()
  const start = screen.getByRole("button", { name: "Start syncing" })
  expect(start).toBeDisabled()
  expect(screen.queryByRole("switch")).not.toBeInTheDocument()
  await waitFor(() => expect(port.workspaces).toHaveBeenCalledOnce())
  await act(async () => finish([team]))
  expect(screen.getByText(/Workspace availability could not be confirmed/)).toBeVisible()
  expect(screen.getByRole("button", { name: "Refresh folders" })).toBeEnabled()
  expect(screen.getByRole("button", { name: "Continue locally" })).toBeEnabled()
  expect(start).toBeDisabled()
  expect(screen.queryByRole("combobox")).not.toBeInTheDocument()
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it("does not treat a previously checked folder as a new Personal choice", async () => {
  const { port, ongoing } = cloudProjectFixture()
  port.workspaces = vi.fn(async () => [personal, team])
  ongoing.status.mockResolvedValueOnce(
    ongoingStatus({
      status: "ready",
      consentVersion: "desktop-project-sync-v1",
      projectChoices: { studio: true },
    }),
  )
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  const picker = await screen.findByRole("combobox", { name: /Sync destination for Studio/ })
  const start = screen.getByRole("button", { name: "Start syncing" })
  expect(
    screen.getByRole("option", { name: "Choose a workspace", selected: true }),
  ).toBeVisible()
  expect(start).toBeDisabled()
  fireEvent.change(picker, { target: { value: personal.id } })
  expect(start).toBeEnabled()
  expect(picker).toHaveValue(personal.id)
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it("shows the saved Personal destination without offering to move the folder", async () => {
  const { port, ongoing } = cloudProjectFixture()
  port.workspaces = vi.fn(async () => [personal, team])
  ongoing.status.mockResolvedValueOnce(
    ongoingStatus({
      status: "ready",
      projects: projectGroup().projects,
      consentVersion: "desktop-project-sync-v1",
      projectChoices: { studio: true },
    }),
  )
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  expect(await screen.findByText(/Sync is already set up for Personal workspace/)).toBeVisible()
  expect(screen.queryByRole("combobox")).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Start syncing" })).toBeDisabled()
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it("lets older servers add an explicit Personal choice with the one-workspace switch", async () => {
  const { port, ongoing } = cloudProjectFixture()
  port.workspaces = vi.fn(async () => null)
  ongoing.status.mockResolvedValueOnce(
    ongoingStatus({
      status: "ready",
      consentVersion: "desktop-project-sync-v1",
      projectChoices: { anotherFolder: false },
    }),
  )
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  const toggle = await screen.findByRole("switch", { name: /Sync Studio/ })
  const start = screen.getByRole("button", { name: "Start syncing" })
  expect(toggle).not.toBeChecked()
  expect(start).toBeDisabled()
  fireEvent.click(toggle)
  expect(toggle).toBeChecked()
  expect(start).toBeEnabled()
  expect(screen.getByText(/Sync destination: Personal workspace/)).toBeVisible()
  expect(ongoing.enable).not.toHaveBeenCalled()
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
