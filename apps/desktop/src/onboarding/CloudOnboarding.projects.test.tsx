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
  expect(screen.queryByText(/Matching Git checkouts are included/)).not.toBeInTheDocument()
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
  expect(screen.getByText("Local tools unavailable")).toBeVisible()
  expect(screen.queryByRole("progressbar")).not.toBeInTheDocument()
  expect(screen.queryByRole("button", { name: /Retry|Remove/ })).not.toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "Done" }))
  expect(done).toHaveBeenCalledOnce()
})

it("shows the repository scope and recognizes the saved group at a new checkout", async () => {
  const { port, ongoing, emitSync } = cloudProjectFixture()
  const path = "/Example/new/Studio"
  const anchor = projectGroup().projects[0]!
  const group = projectGroup({
    id: "repo-studio",
    path,
    folderPaths: [path],
    worktreeCount: 0,
    repositoryWide: true,
    projects: [{ ...anchor, projectPath: path }],
  })
  ongoing.groups.mockResolvedValue({
    groups: [group],
    sourceRoots: [{ sourceKind: anchor.sourceKind, path: anchor.sourcePath }],
    nextCursor: null,
    partial: false,
  })
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )

  expect(await screen.findByText(/Matching Git checkouts are included/)).toBeVisible()
  expect(screen.getByRole("switch", { name: `Sync Studio: ${path}` })).toBeChecked()
  act(() =>
    emitSync(
      ongoingStatus({
        status: "ready",
        consentVersion: "desktop-project-sync-v1",
        projects: [anchor],
        exactProjects: [],
        repositoryGroupIds: [group.id],
        repositoryFolders: [{ groupId: group.id, projects: [anchor] }],
        savedFolders: [{ groupId: group.id, path: anchor.projectPath }],
      }),
    ),
  )
  expect(screen.getByText(/Sync is already set up for Personal workspace/)).toBeVisible()
  expect(screen.queryByText(/Matching Git checkouts are included/)).not.toBeInTheDocument()
  expect(screen.queryByRole("switch", { name: `Sync Studio: ${path}` })).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Start syncing" })).toBeDisabled()
  expect(ongoing.enable).not.toHaveBeenCalled()
  expect(ongoing.setProject).not.toHaveBeenCalled()
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

it("does not show a worktree count when Desktop could not verify the group", async () => {
  const { port, ongoing } = cloudProjectFixture()
  ongoing.groups.mockResolvedValue({
    groups: [projectGroup({ groupingComplete: false, captureSupported: false })],
    sourceRoots: [],
    nextCursor: null,
    partial: false,
  })
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  expect(await screen.findByText("/Example/Studio")).toBeVisible()
  expect(screen.queryByText("Worktrees not verified")).not.toBeInTheDocument()
  expect(screen.queryByText("1 worktree")).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Start syncing" })).toBeDisabled()
  expect(screen.getByRole("button", { name: "Continue locally" })).toBeEnabled()
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
      projects: projectGroup().projects,
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

it("confirms a mixed draft and retries only the folder route that failed", async () => {
  const { port, ongoing, emit } = cloudProjectFixture()
  const docsProject = {
    sourceKind: "claude_jsonl" as const,
    sourcePath: "/Example/claude/projects/docs",
    projectPath: "/Example/Docs",
  }
  ongoing.groups.mockResolvedValue({
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
  let personalStatus = ongoingStatus()
  ongoing.status.mockImplementation(async () => personalStatus)
  ongoing.enable
    .mockImplementationOnce(async (consent) => {
      personalStatus = ongoingStatus({
        status: "ready",
        consentVersion: consent.version,
        projects: consent.projects,
        selectedSources: consent.selectedSources,
        projectChoices: consent.projectChoices,
      })
      emit({ syncEnabled: true })
      return personalStatus
    })
    .mockRejectedValueOnce(new Error("Team unavailable"))
    .mockImplementationOnce(async (consent) =>
      ongoingStatus({
        status: "ready",
        destinationId: team.id,
        consentVersion: consent.version,
        projects: consent.projects,
        selectedSources: consent.selectedSources,
        projectChoices: consent.projectChoices,
      }),
    )
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
  expect(screen.getByText("Each folder uploads only to its selected workspace.")).toBeVisible()
  expect(start).toBeEnabled()
  expect(ongoing.enable).not.toHaveBeenCalled()

  fireEvent.click(start)
  expect(
    await screen.findByText(/Could not confirm folder sync for Example Team/),
  ).toBeVisible()
  act(() => emit({ syncEnabled: true }))
  expect(screen.getByText(/Could not confirm folder sync for Example Team/)).toBeVisible()
  expect(screen.queryByRole("button", { name: "Done" })).not.toBeInTheDocument()
  expect(ongoing.enable).toHaveBeenNthCalledWith(
    1,
    expect.objectContaining({
      destinationId: personal.id,
      projects: projectGroup().projects,
    }),
  )
  expect(ongoing.enable).toHaveBeenNthCalledWith(
    2,
    expect.objectContaining({ destinationId: team.id, projects: [docsProject] }),
  )
  expect(
    screen.queryByRole("combobox", { name: /Sync destination for Studio/ }),
  ).not.toBeInTheDocument()
  expect(screen.getByRole("combobox", { name: /Sync destination for Docs/ })).toHaveValue(
    team.id,
  )

  fireEvent.click(screen.getByRole("button", { name: "Start syncing" }))
  expect(await screen.findByText("Background sync enabled")).toBeVisible()
  expect(ongoing.enable).toHaveBeenCalledTimes(3)
  expect(ongoing.enable).toHaveBeenNthCalledWith(
    3,
    expect.objectContaining({ destinationId: team.id, projects: [docsProject] }),
  )
})

it("keeps Start disabled while workspace availability is loading or missing", async () => {
  const { port, ongoing } = cloudProjectFixture()
  let finish!: (workspaces: CloudWorkspace[]) => void
  port.workspaces = vi
    .fn(async (): Promise<CloudWorkspace[]> => [team])
    .mockImplementationOnce(
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
  const provisional = await screen.findByRole("switch", { name: /Sync Studio/ })
  expect(provisional).toBeDisabled()
  expect(provisional).not.toBeChecked()
  expect(ongoing.groups).toHaveBeenCalledOnce()
  await waitFor(() => expect(port.workspaces).toHaveBeenCalledOnce())
  await act(async () => finish([team]))
  expect(await screen.findByText(/Workspace availability could not be confirmed/)).toBeVisible()
  expect(screen.queryByRole("button", { name: "Refresh folders" })).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Continue locally" })).toBeEnabled()
  expect(provisional).toBeDisabled()
  expect(start).toBeDisabled()
  expect(screen.queryByRole("combobox")).not.toBeInTheDocument()
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it("starts sync for verified folders while a later page is retrying", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const group = projectGroup()
  ongoing.groups.mockRejectedValue(new Error("Page unavailable")).mockResolvedValueOnce({
    groups: [group],
    sourceRoots: [{ sourceKind: "claude_jsonl", path: group.projects[0]!.sourcePath }],
    nextCursor: { environment: "native", cwd: "/Example/Next" },
    partial: true,
  })
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  fireEvent.click(await screen.findByRole("button", { name: "Load more folders" }))
  expect(await screen.findByText("Still checking for more folders.")).toBeVisible()
  const start = screen.getByRole("button", { name: "Start syncing" })
  expect(start).toBeEnabled()
  fireEvent.click(start)
  await waitFor(() => expect(ongoing.enable).toHaveBeenCalledOnce())
  expect(ongoing.enable).toHaveBeenCalledWith(
    expect.objectContaining({ projects: group.projects }),
  )
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

it("waits for exact folder confirmation before offering Done", async () => {
  const { port, ongoing } = cloudProjectFixture()
  let finish!: (value: CloudOngoingStatus) => void
  ongoing.enable.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve
      }),
  )
  const done = vi.fn()
  render(<CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={done} />)
  await screen.findByRole("switch")
  fireEvent.click(screen.getByRole("button", { name: "Start syncing" }))
  expect(screen.getByText("Confirming folder sync…")).toBeVisible()
  expect(screen.getByRole("button", { name: "Start syncing" })).toBeDisabled()
  expect(screen.queryByRole("button", { name: "Done" })).not.toBeInTheDocument()
  await act(async () =>
    finish(
      ongoingStatus({
        status: "ready",
        consentVersion: "desktop-project-sync-v1",
        projects: projectGroup().projects,
      }),
    ),
  )
  fireEvent.click(screen.getByRole("button", { name: "Done" }))
  expect(done).toHaveBeenCalledOnce()
  expect(ongoing.enable).toHaveBeenCalledOnce()
})

it("keeps a failed single-workspace choice selected and ready to retry", async () => {
  const { port, ongoing, emit } = cloudProjectFixture()
  ongoing.enable.mockRejectedValueOnce(new Error("Synthetic unknown result"))
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  const folder = await screen.findByRole("switch")
  const start = screen.getByRole("button", { name: "Start syncing" })
  fireEvent.click(start)
  expect(await screen.findByText(/Could not confirm folder sync for Personal/)).toBeVisible()
  act(() => emit({ syncEnabled: false }))
  expect(folder).toBeChecked()
  expect(start).toBeEnabled()
  expect(screen.queryByRole("button", { name: "Done" })).not.toBeInTheDocument()
  expect(ongoing.enable).toHaveBeenCalledOnce()
  fireEvent.click(start)
  expect(await screen.findByRole("button", { name: "Done" })).toBeEnabled()
  expect(ongoing.enable).toHaveBeenCalledTimes(2)
  expect(ongoing.enable.mock.calls[0]).toEqual(ongoing.enable.mock.calls[1])
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
  ongoing.groups.mockResolvedValue({
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
  const listing = { groups: [], sourceRoots: [], ...page }
  if (page.partial) ongoing.groups.mockResolvedValue(listing)
  else ongoing.groups.mockResolvedValueOnce(listing)
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
    expect(await screen.findByText("Loading more folders…")).toBeVisible()
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

it("retries a transient folder read without flashing an error or an empty list", async () => {
  const { port, ongoing } = cloudProjectFixture()
  ongoing.groups.mockRejectedValueOnce(new Error("Synthetic discovery failure"))
  render(
    <CloudOnboarding port={port} analytics={false} onBack={vi.fn()} onComplete={vi.fn()} />,
  )
  await screen.findByText("Loading project folders…")
  expect(screen.queryByText(/could not check your session folders/i)).not.toBeInTheDocument()
  expect(screen.queryByText(/No project folders found/)).not.toBeInTheDocument()
  expect(screen.queryByRole("button", { name: "Refresh folders" })).not.toBeInTheDocument()
  expect(await screen.findByRole("switch", { name: /Sync Studio/ })).toBeChecked()
  expect(ongoing.groups).toHaveBeenCalledTimes(2)
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
  vi.mocked(port.read).mockResolvedValue(healthy)
  act(() => emit({ setupError: "Synthetic setup failure" }))
  expect(screen.queryByText("Synthetic setup failure")).not.toBeInTheDocument()
  expect(screen.queryByRole("button", { name: "Refresh folders" })).not.toBeInTheDocument()
  expect(ongoing.groups).toHaveBeenCalledOnce()
  await act(async () =>
    finish({
      groups: [],
      sourceRoots: [],
      nextCursor: null,
      partial: false,
    }),
  )
  expect(await screen.findByRole("switch", { name: /Sync Studio/ })).toBeChecked()
  expect(screen.queryByText(/No project folders found/)).not.toBeInTheDocument()
  expect(ongoing.groups).toHaveBeenCalledTimes(2)
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it("explains unqualified folders after a complete check without offering another read", async () => {
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
  expect(screen.queryByRole("button", { name: "Refresh folders" })).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Start syncing" })).toBeDisabled()
  expect(document.querySelector("footer")?.querySelectorAll("p")).toHaveLength(0)
})
