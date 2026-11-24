import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { useSyncExternalStore } from "react"
import { beforeEach, expect, it, vi } from "vitest"
import { CloudSession } from "../../onboarding/CloudSession"
import {
  cloudProjectFixture,
  ongoingStatus,
  projectGroup,
} from "../../onboarding/__tests__/cloudProjectFixtures"
import { CloudAccount } from "./CloudAccount"
import { SettingsTargetFocus } from "./SettingsTargetFocus"

beforeEach(() => {
  Object.defineProperty(HTMLDialogElement.prototype, "showModal", {
    configurable: true,
    value(this: HTMLDialogElement) {
      this.open = true
    },
  })
  Object.defineProperty(HTMLDialogElement.prototype, "close", {
    configurable: true,
    value(this: HTMLDialogElement) {
      this.open = false
    },
  })
})

function Account({ session }: { session: CloudSession }) {
  const value = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
    session.getSnapshot,
  )
  return <CloudAccount session={session} value={value} />
}

async function showOtherSavedFolders() {
  const summary = await screen.findByText("Manage other saved folders")
  summary.closest("details")!.open = true
}

it("shows saved controls before cold grouping and reuses grouped rows during refresh", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const group = projectGroup()
  const page = {
    groups: [group],
    sourceRoots: [{ sourceKind: "claude_jsonl" as const, path: group.projects[0]!.sourcePath }],
    nextCursor: null,
    partial: false,
  }
  ongoing.status.mockResolvedValue(
    ongoingStatus({
      status: "ready",
      consentVersion: "desktop-project-sync-v1",
      projects: group.projects,
      savedFolders: group.folderPaths.map((path) => ({ groupId: `saved-${path}`, path })),
    }),
  )
  let finish!: (value: typeof page) => void
  const deferPage = () => new Promise<typeof page>((resolve) => (finish = resolve))
  ongoing.groups.mockImplementation(deferPage)
  const cold = render(<Account session={new CloudSession(port, false, true)} />)
  await waitFor(() => expect(ongoing.groups).toHaveBeenCalledOnce())
  await showOtherSavedFolders()
  const exact = screen.getByRole("switch", { name: `Sync ${group.path} to Personal` })
  expect(exact).toBeChecked()
  expect(exact).toBeEnabled()
  await act(async () => finish(page))
  expect(await screen.findByRole("switch", { name: /Sync Studio/ })).toBeChecked()
  expect(screen.getAllByRole("switch")).toHaveLength(1)
  cold.unmount()

  render(<Account session={new CloudSession(port, false, true)} />)
  const control = await screen.findByRole("switch", { name: /Sync Studio/ })
  await waitFor(() => expect(ongoing.groups).toHaveBeenCalledTimes(2))
  expect(control).toBeEnabled()
  control.focus()
  await act(async () => finish(page))
  expect(screen.getByRole("switch", { name: /Sync Studio/ })).toBe(control)
  expect(control).toHaveFocus()
  expect(screen.getAllByRole("switch")).toHaveLength(1)
  expect(ongoing.setProject).not.toHaveBeenCalled()
})

it("keeps the exact saved path manageable when project discovery fails", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const path = "/Example/worktrees/studio-fix"
  ongoing.status.mockResolvedValue(
    ongoingStatus({
      status: "ready",
      consentVersion: "desktop-project-sync-v1",
      projects: projectGroup().projects.filter(({ projectPath }) => projectPath === path),
      savedFolders: [{ groupId: "saved-path", path }],
    }),
  )
  ongoing.groups.mockRejectedValue(new Error("Synthetic discovery unavailable"))
  render(<Account session={new CloudSession(port, false, true)} />)
  expect(await screen.findByText(/Still checking session folders/)).toBeVisible()
  expect(screen.queryByRole("alert")).not.toBeInTheDocument()
  expect(ongoing.groups).toHaveBeenCalledTimes(3)
  await showOtherSavedFolders()
  const control = screen.getByRole("switch", { name: `Sync ${path} to Personal` })
  expect(control).toBeChecked()
  fireEvent.click(control)
  await waitFor(() =>
    expect(ongoing.setProject).toHaveBeenCalledWith(
      expect.objectContaining({ groupId: "saved-path", selected: false, folderPaths: [path] }),
    ),
  )
})

it("uses the saved repository ID across checkouts and revokes that ID on request", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const anchor = projectGroup().projects[0]!
  const path = "/Example/new/Studio"
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
  ongoing.status.mockResolvedValue(
    ongoingStatus({
      status: "ready",
      consentVersion: "desktop-project-sync-v1",
      projects: [anchor],
      exactProjects: [],
      repositoryGroupIds: [group.id],
      repositoryFolders: [{ groupId: group.id, projects: [anchor] }],
      savedFolders: [{ groupId: group.id, path: anchor.projectPath }],
    }),
  )
  render(<Account session={new CloudSession(port, false, true)} />)

  const control = await screen.findByRole("switch", { name: `Sync Studio: ${path}` })
  expect(control).toBeChecked()
  expect(screen.queryByText(/Matching Git checkouts are included/)).not.toBeInTheDocument()
  expect(screen.queryByText("Manage other saved folders")).not.toBeInTheDocument()
  expect(ongoing.enable).not.toHaveBeenCalled()
  expect(ongoing.setProject).not.toHaveBeenCalled()
  fireEvent.click(control)
  await waitFor(() =>
    expect(ongoing.setProject).toHaveBeenCalledExactlyOnceWith(
      expect.objectContaining({ groupId: group.id, selected: false, projects: [] }),
    ),
  )
  await waitFor(() =>
    expect(screen.getByRole("switch", { name: `Sync Studio: ${path}` })).not.toBeChecked(),
  )
})

it.each([
  ["Could not verify Cloud workspaces.", "Still checking Cloud workspaces.", 3],
  ["Cloud workspace sign-in required.", "Sign in again to check Cloud workspaces.", 1],
  [
    "Cloud workspace access denied.",
    "You do not have access to the selected Cloud workspace.",
    1,
  ],
])(
  "reports workspace lookup failure without local file advice: %s",
  async (failure, message, reads) => {
    const { port, ongoing } = cloudProjectFixture()
    const workspaces = vi.fn().mockRejectedValue(failure)
    port.workspaces = workspaces
    render(<Account session={new CloudSession(port, false, true)} />)
    expect(await screen.findByText(message)).toBeVisible()
    expect(screen.queryByText(/can access your session files/)).not.toBeInTheDocument()
    expect(workspaces).toHaveBeenCalledTimes(reads)
    expect(ongoing.groups).not.toHaveBeenCalled()
  },
)

it("shows workspace receipt progress and actual queue counts separately", async () => {
  const { port, ongoing, emitSync } = cloudProjectFixture()
  const personal = ongoingStatus({
    status: "syncing",
    consentVersion: "desktop-project-sync-v1",
    projects: projectGroup().projects,
    uploadProgress: { uploaded: 6, total: 96, blocked: 4, unsupported: 1 },
    queuedCount: 3,
  })
  const team = {
    ...personal,
    destinationId: "synthetic-team",
    workspaceName: "Example Team",
    uploadProgress: { uploaded: 2, total: null, scanIncomplete: true },
    queuedCount: 1,
  }
  ongoing.status.mockResolvedValue({ ...personal, teamStatuses: [team] })
  render(<Account session={new CloudSession(port, false, true)} />)
  const statuses = await screen.findByRole("region", { name: "Cloud sync status" })
  const personalStatus = within(statuses).getByRole("group", { name: "Personal sync status" })
  const teamStatus = await within(statuses).findByRole("group", {
    name: "Example Team sync status",
  })
  expect(personalStatus).toHaveTextContent("3 queued · 4 blocked · 1 unsupported")
  expect(within(statuses).getByText("6 of 96 sessions accepted")).toBeVisible()
  expect(personalStatus).not.toHaveTextContent("90 queued")
  expect(teamStatus).toHaveTextContent("1 queued · Folder scan incomplete")
  expect(within(statuses).getByText(/2 sessions accepted/)).toBeVisible()

  act(() => emitSync({ ...team, storageKnown: false, status: "storage_unavailable" }))
  expect(teamStatus).toHaveTextContent("Checking saved uploads…")
  expect(within(statuses).queryByText(/2 sessions accepted/)).not.toBeInTheDocument()
  expect(within(statuses).getByText("6 of 96 sessions accepted")).toBeVisible()
})

it("offers manual upload retry only after Desktop stops automatic verification", async () => {
  const { port, ongoing, emitSync } = cloudProjectFixture()
  const policy = ongoingStatus({
    status: "fenced",
    consentVersion: "desktop-project-sync-v1",
    projects: projectGroup().projects,
    deviceUploadEnabled: true,
    queuedCount: 1,
    automaticRecoveryCount: 1,
  })
  ongoing.status.mockResolvedValue(policy)
  render(<Account session={new CloudSession(port, false, true)} />)
  expect(
    await screen.findByText("Checking saved uploads automatically. Saved files are kept."),
  ).toBeVisible()
  expect(screen.queryByRole("alert")).not.toBeInTheDocument()
  expect(screen.queryByRole("button", { name: "Retry saved uploads" })).not.toBeInTheDocument()
  expect(screen.queryByText(/accepted/)).not.toBeInTheDocument()

  act(() =>
    emitSync({
      ...policy,
      queuedCount: 0,
      automaticRecoveryCount: 0,
      fencedCount: 1,
      retryAvailable: true,
    }),
  )
  expect(screen.queryByText(/Checking saved uploads automatically/)).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Retry saved uploads" })).toBeEnabled()
  expect(ongoing.retry).not.toHaveBeenCalled()
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it("keeps offline uploads without promising to retry interrupted work", async () => {
  const { port, ongoing } = cloudProjectFixture()
  ongoing.status.mockResolvedValue(
    ongoingStatus({
      status: "offline",
      consentVersion: "desktop-project-sync-v1",
      projects: projectGroup().projects,
      interruptedCount: 1,
      retryAvailable: true,
      deviceUploadEnabled: true,
    }),
  )
  render(<Account session={new CloudSession(port, false, true)} />)
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Cloud is offline. Saved uploads are kept.",
  )
  expect(screen.getByRole("group", { name: "Folder sync status" })).toHaveTextContent(
    "1 need attention",
  )
  expect(ongoing.retry).not.toHaveBeenCalled()
})

it("shows the verified workspace count instead of the Personal grant name", async () => {
  const { port } = cloudProjectFixture()
  port.workspaces = vi.fn(async () => [
    { id: "synthetic-personal", name: "My workspace", kind: "personal" as const },
    { id: "synthetic-team", name: "Example Team", kind: "team" as const },
  ])
  render(<Account session={new CloudSession(port, false, true)} />)
  expect(await screen.findByText("2 workspaces")).toBeVisible()
  expect(screen.queryByText("My workspace", { selector: "p" })).not.toBeInTheDocument()
})

it("keeps the primary button and workspace picker mounted after saving to Team", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const group = projectGroup()
  const begin = vi.spyOn(port, "begin")
  const team = ongoingStatus({
    destinationId: "synthetic-team",
    workspaceName: "Example Team",
    consentVersion: "desktop-project-sync-v1",
    status: "ready",
    projects: group.projects,
  })
  port.workspaces = vi.fn(async () => [
    { id: "synthetic-personal", name: "My workspace", kind: "personal" as const },
    { id: "synthetic-team", name: "Example Team", kind: "team" as const },
  ])
  ongoing.enable.mockResolvedValueOnce(team)
  render(<Account session={new CloudSession(port, false, true)} />)
  const picker = await screen.findByRole("combobox", { name: /Sync Studio/ })
  fireEvent.change(picker, { target: { value: "synthetic-team" } })
  expect(ongoing.enable).not.toHaveBeenCalled()
  const start = screen.getByRole("button", { name: "Start syncing" })
  expect(start).toBeEnabled()
  start.focus()
  fireEvent.click(start)
  await waitFor(() => expect(picker).toHaveFocus())
  expect(screen.getByRole("combobox", { name: /Sync Studio/ })).toBe(picker)
  expect(picker).toHaveValue("synthetic-team")
  expect(screen.getByRole("button", { name: "Sync now" })).toBe(start)
  expect(start).toBeDisabled()
  expect(ongoing.enable).toHaveBeenCalledExactlyOnceWith(
    expect.objectContaining({ destinationId: "synthetic-team", projects: group.projects }),
  )
  expect(ongoing.setProject).not.toHaveBeenCalled()
  expect(begin).not.toHaveBeenCalled()
})

it.each([
  ["synthetic-personal", "synthetic-team"],
  ["synthetic-team", "synthetic-personal"],
])(
  "keeps the saved workspace picker for %s and does not move a folder until Sync now",
  async (from, to) => {
    const { port, ongoing } = cloudProjectFixture()
    const group = projectGroup()
    const saved = ongoingStatus({
      destinationId: from,
      workspaceName: from === "synthetic-team" ? "Example Team" : "Personal",
      consentVersion: "desktop-project-sync-v1",
      status: "ready",
      projects: group.projects,
      deviceUploadEnabled: true,
    })
    port.workspaces = vi.fn(async () => [
      { id: "synthetic-personal", name: "Personal", kind: "personal" as const },
      { id: "synthetic-team", name: "Example Team", kind: "team" as const },
    ])
    ongoing.status.mockResolvedValue(
      from === "synthetic-personal"
        ? saved
        : ongoingStatus({ deviceUploadEnabled: true, teamStatuses: [saved] }),
    )
    const session = new CloudSession(port, false, true)
    const moveSavedProject = vi
      .spyOn(session, "moveSavedProject")
      .mockResolvedValue("clear_draft")
    render(<Account session={session} />)
    const picker = await screen.findByRole("combobox", { name: /Sync Studio/ })
    const save = screen.getByRole("button", { name: "Sync now" })
    expect(picker).toHaveValue(from)
    expect(screen.queryByRole("switch", { name: /Sync Studio/ })).not.toBeInTheDocument()
    expect(save).toBeEnabled()

    fireEvent.change(picker, { target: { value: "" } })
    expect(picker).toHaveValue("")
    expect(save).toBeEnabled()
    fireEvent.change(picker, { target: { value: from } })
    expect(save).toBeEnabled()

    fireEvent.change(picker, { target: { value: to } })
    expect(picker).toHaveValue(to)
    expect(save).toBeEnabled()
    expect(ongoing.backfill).not.toHaveBeenCalled()
    expect(ongoing.setProject).not.toHaveBeenCalled()
    expect(ongoing.enable).not.toHaveBeenCalled()
    expect(moveSavedProject).not.toHaveBeenCalled()

    fireEvent.click(save)
    await waitFor(() =>
      expect(moveSavedProject).toHaveBeenCalledExactlyOnceWith(
        expect.anything(),
        group.id,
        from,
        to,
      ),
    )
    expect(ongoing.backfill).not.toHaveBeenCalled()
  },
)

it("keeps a staged workspace change editable when its target disappears", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const personal = { id: "synthetic-personal", name: "Personal", kind: "personal" } as const
  const team = { id: "synthetic-team", name: "Example Team", kind: "team" } as const
  const workspaces = vi.fn(async () => [personal, team])
  port.workspaces = workspaces
  ongoing.status.mockResolvedValue(
    ongoingStatus({
      status: "ready",
      consentVersion: "desktop-project-sync-v1",
      projects: projectGroup().projects,
    }),
  )
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  const picker = await screen.findByRole("combobox", { name: /Sync Studio/ })
  const save = screen.getByRole("button", { name: "Sync now" })
  fireEvent.change(picker, { target: { value: team.id } })
  expect(save).toBeEnabled()

  workspaces.mockResolvedValue([personal])
  await act(() => session.loadProjects())

  expect(picker).toHaveValue(team.id)
  expect(picker).toBeEnabled()
  expect(save).toBeDisabled()
  expect(screen.getByText("That workspace is unavailable. Choose another.")).toBeVisible()
  fireEvent.change(picker, { target: { value: personal.id } })
  expect(screen.getByRole("switch", { name: /Sync Studio/ })).toBeChecked()
  expect(ongoing.setProject).not.toHaveBeenCalled()
})

it("requires a new workspace choice when a staged move's saved policy changes", async () => {
  const { port, ongoing, emitSync } = cloudProjectFixture()
  const saved = ongoingStatus({
    status: "ready",
    consentVersion: "desktop-project-sync-v1",
    deviceUploadEnabled: true,
    projects: projectGroup().projects,
  })
  port.workspaces = vi.fn(async () => [
    { id: "synthetic-personal", name: "Personal", kind: "personal" as const },
    { id: "synthetic-team", name: "Example Team", kind: "team" as const },
  ])
  ongoing.status.mockResolvedValue(saved)
  const session = new CloudSession(port, false, true)
  const move = vi.spyOn(session, "moveSavedProject")
  render(<Account session={session} />)
  const picker = await screen.findByRole("combobox", { name: /Sync Studio/ })
  const sync = screen.getByRole("button", { name: "Sync now" })
  fireEvent.change(picker, { target: { value: "synthetic-team" } })
  expect(sync).toBeEnabled()

  act(() => emitSync({ ...saved, policyRevision: "changed-policy" }))
  expect(sync).toBeDisabled()
  expect(screen.getByText("The saved folder changed. Choose a workspace again.")).toBeVisible()
  fireEvent.click(sync)
  expect(move).not.toHaveBeenCalled()
  expect(ongoing.backfill).not.toHaveBeenCalled()
  fireEvent.change(picker, { target: { value: "synthetic-personal" } })
  fireEvent.change(picker, { target: { value: "synthetic-team" } })
  expect(sync).toBeEnabled()
})

it("keeps a staged move if Desktop could not verify the target before removal", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const personal = ongoingStatus({
    status: "ready",
    consentVersion: "desktop-project-sync-v1",
    deviceUploadEnabled: true,
    projects: projectGroup().projects,
  })
  const team = ongoingStatus({
    status: "ready",
    destinationId: "synthetic-team",
    consentVersion: "desktop-project-sync-v1",
    policyRevision: "team-policy",
    projects: [{ ...projectGroup().projects[0]!, projectPath: "/Example/Other" }],
  })
  const saved = { ...personal, teamStatuses: [team] }
  ongoing.status.mockResolvedValue(saved)
  port.workspaces = vi.fn(async () => [
    { id: "synthetic-personal", name: "Personal", kind: "personal" as const },
    { id: "synthetic-team", name: "Example Team", kind: "team" as const },
  ])
  render(<Account session={new CloudSession(port, false, true)} />)
  const picker = await screen.findByRole("combobox", { name: /Sync Studio/ })
  fireEvent.change(picker, { target: { value: "synthetic-team" } })
  ongoing.status.mockRejectedValueOnce(new Error("Synthetic Team verification failure"))
  fireEvent.click(screen.getByRole("button", { name: "Sync now" }))
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Could not verify the Team folder rules.",
  )
  expect(picker).toHaveValue("synthetic-team")
  fireEvent.click(screen.getByRole("button", { name: "Review folders" }))
  await waitFor(() => expect(screen.queryByRole("alert")).toBeNull())
  expect(screen.getByRole("button", { name: "Sync now" })).toBeEnabled()
  expect(picker).toHaveValue("synthetic-team")
  expect(ongoing.setProject).not.toHaveBeenCalled()
  expect(ongoing.backfill).not.toHaveBeenCalled()
})

it("does not restore folder focus when the user moved it during a save", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const team = ongoingStatus({
    destinationId: "synthetic-team",
    workspaceName: "Example Team",
    consentVersion: "desktop-project-sync-v1",
    status: "ready",
    projects: projectGroup().projects,
  })
  port.workspaces = vi.fn(async () => [
    { id: "synthetic-personal", kind: "personal" as const, name: "Personal" },
    { id: "synthetic-team", kind: "team" as const, name: "Example Team" },
  ])
  let finishSave!: (status: typeof team) => void
  ongoing.enable.mockImplementationOnce(() => new Promise((resolve) => (finishSave = resolve)))
  render(
    <>
      <Account session={new CloudSession(port, false, true)} />
      <button>Other setting</button>
    </>,
  )
  fireEvent.change(await screen.findByRole("combobox", { name: /Sync Studio/ }), {
    target: { value: "synthetic-team" },
  })
  const start = screen.getByRole("button", { name: "Start syncing" })
  start.focus()
  fireEvent.click(start)
  const other = screen.getByRole("button", { name: "Other setting" })
  other.focus()
  other.blur()
  await act(async () => finishSave(team))
  await waitFor(() => expect(start).toBeDisabled())
  expect(screen.getByRole("combobox", { name: /Sync Studio/ })).toHaveValue("synthetic-team")
  await act(() => new Promise<void>((resolve) => requestAnimationFrame(() => resolve())))
  expect(document.body).toHaveFocus()
})

it("revokes immediately and stages a new approval until the primary sync action", async () => {
  const { port, ongoing } = cloudProjectFixture()
  ongoing.status.mockResolvedValue(
    ongoingStatus({
      status: "ready",
      consentVersion: "desktop-project-sync-v1",
      projects: projectGroup().projects,
      projectChoices: { studio: true },
    }),
  )
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  const toggle = () => screen.getByRole("switch", { name: /Sync Studio/ })
  await screen.findByRole("switch", { name: /Sync Studio/ })
  const save = screen.getByRole("button", { name: "Sync now" })
  expect(save).toBeDisabled()
  expect(toggle()).toBeChecked()
  expect(
    screen.queryByRole("button", { name: "Update sync selection" }),
  ).not.toBeInTheDocument()
  fireEvent.click(toggle())
  await waitFor(() => expect(ongoing.setProject).toHaveBeenCalledOnce())
  expect(ongoing.setProject).toHaveBeenLastCalledWith(
    expect.objectContaining({
      selected: false,
      projects: [],
      expectedPolicyRevision: "synthetic-policy",
      folderPaths: projectGroup().folderPaths,
    }),
  )
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
  await waitFor(() => expect(toggle()).not.toBeChecked())
  fireEvent.click(toggle())
  expect(save).toBeEnabled()
  expect(ongoing.setProject).toHaveBeenCalledOnce()
  fireEvent.click(toggle())
  expect(save).toBeDisabled()
  fireEvent.click(toggle())
  fireEvent.click(save)
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
  await waitFor(() => expect(ongoing.setProject).toHaveBeenCalledTimes(2))
  expect(ongoing.setProject).toHaveBeenLastCalledWith(
    expect.objectContaining({
      selected: true,
      projects: projectGroup().projects,
    }),
  )
  expect(ongoing.enable).not.toHaveBeenCalled()
  await waitFor(() => expect(toggle()).toBeChecked())
  expect(screen.queryByText("Uploads on")).not.toBeInTheDocument()
})

it("does not redirect a draft when its Team disappears from the verified list", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const personal = { id: "synthetic-personal", kind: "personal", name: "Personal" } as const
  port.workspaces = vi
    .fn<NonNullable<typeof port.workspaces>>(async () => [personal])
    .mockResolvedValueOnce([
      personal,
      { id: "synthetic-team", kind: "team", name: "Example Team" },
    ])
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  const picker = await screen.findByRole("combobox", { name: /Sync Studio/ })
  const start = screen.getByRole("button", { name: "Start syncing" })
  fireEvent.change(picker, { target: { value: "synthetic-team" } })
  expect(start).toBeEnabled()

  await act(() => session.loadProjects())
  expect(start).toBeDisabled()
  expect(picker).toHaveValue("synthetic-team")
  expect(screen.getByRole("option", { name: "Selected workspace unavailable" })).toBeDisabled()
  fireEvent.click(start)
  expect(ongoing.enable).not.toHaveBeenCalled()

  fireEvent.change(picker, { target: { value: personal.id } })
  expect(start).toBeEnabled()
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it("does not apply a folder choice from an older saved policy", async () => {
  const { port, ongoing, emitSync } = cloudProjectFixture()
  const saved = ongoingStatus({
    status: "ready",
    consentVersion: "desktop-project-sync-v1",
    projects: [],
    projectChoices: { studio: false },
  })
  ongoing.status.mockResolvedValue(saved)
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  await screen.findByRole("switch", { name: /Sync Studio/ })
  const context = session.getSnapshot()
  act(() => emitSync({ ...saved, policyRevision: "new-policy" }))
  await session.setSavedProject(context, "studio", true)
  expect(ongoing.setProject).not.toHaveBeenCalled()
})

it.each([false, true])(
  "shows a failed folder save after a progress event: %s",
  async (progress) => {
    const { port, ongoing, emitSync } = cloudProjectFixture()
    const saved = ongoingStatus({
      status: "ready",
      consentVersion: "desktop-project-sync-v1",
      projects: projectGroup().projects,
      projectChoices: { studio: true },
    })
    ongoing.status.mockResolvedValue(saved)
    let reject!: (error: Error) => void
    ongoing.setProject.mockImplementationOnce(
      () =>
        new Promise((_, fail) => {
          reject = fail
        }),
    )
    const session = new CloudSession(port, false, true)
    render(<Account session={session} />)
    const toggle = await screen.findByRole("switch", { name: /Sync Studio/ })
    fireEvent.click(toggle)
    if (progress) act(() => emitSync(saved))
    await act(async () => reject(new Error("Synthetic failure")))
    expect(await screen.findByText("Could not update the folder. Try again.")).toBeVisible()
    expect(toggle).toBeChecked()
    expect(toggle).toBeEnabled()
  },
)

it("turns off a discovered but disconnected team folder by exact saved path", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const group = projectGroup()
  const path = group.path
  const otherPath = "/Example/Unavailable"
  const savedFolders = [
    { groupId: "saved-discovered", path },
    { groupId: "saved-unavailable", path: otherPath },
  ]
  const projects = [group.projects[0]!, { ...group.projects[0]!, projectPath: otherPath }]
  const team = ongoingStatus({
    status: "reauth_required",
    storageKnown: false,
    workspaceName: "Example Team",
    destinationId: "synthetic-team",
    consentVersion: "desktop-project-sync-v1",
    projects,
    savedFolders,
  })
  ongoing.status.mockResolvedValue(ongoingStatus({ teamStatuses: [team] }))
  ongoing.setProject.mockImplementation(async (change) => {
    if (change.groupId !== "saved-discovered") throw new Error("Unrelated path is unreadable")
    return { ...team, projects: projects.slice(1), savedFolders: savedFolders.slice(1) }
  })
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  await waitFor(() => expect(session.getSnapshot().projects?.groups[0]?.path).toBe(path))
  expect(screen.queryByRole("switch", { name: /Sync Studio:/ })).toBeNull()
  fireEvent.click(screen.getByRole("switch", { name: `Sync ${path} to Example Team` }))
  await waitFor(() =>
    expect(ongoing.setProject).toHaveBeenCalledWith({
      accountId: "synthetic-account",
      destinationId: "synthetic-team",
      groupId: "saved-discovered",
      folderPaths: [path],
      projects: [],
      selected: false,
      expectedPolicyRevision: "synthetic-policy",
    }),
  )
  await waitFor(() =>
    expect(screen.queryByRole("switch", { name: `Sync ${path} to Example Team` })).toBeNull(),
  )
  expect(
    screen.getByRole("switch", { name: `Sync ${otherPath} to Example Team` }),
  ).toBeChecked()
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it("keeps a missing saved rule removable without listing it as a folder choice", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const path = projectGroup().path
  ongoing.status.mockResolvedValue(
    ongoingStatus({
      status: "ready",
      consentVersion: "desktop-project-sync-v1",
      projects: projectGroup().projects,
      savedFolders: [{ groupId: "saved-missing", path }],
    }),
  )
  ongoing.groups.mockResolvedValue({
    groups: [
      projectGroup({
        folderPaths: [],
        projects: [],
        captureSupported: false,
        groupingComplete: false,
      }),
    ],
    sourceRoots: [],
    nextCursor: null,
    partial: false,
  })
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  const summary = await screen.findByText("Manage other saved folders")
  expect(summary.closest("details")).not.toHaveAttribute("open")
  expect(screen.queryByRole("switch", { name: /^Sync Studio:/ })).toBeNull()
  await showOtherSavedFolders()
  const toggle = screen.getByRole("switch", { name: `Sync ${path} to Personal` })
  expect(toggle).toBeChecked()
  expect(toggle).toBeEnabled()
  fireEvent.click(toggle)
  await waitFor(() =>
    expect(ongoing.setProject).toHaveBeenCalledWith(
      expect.objectContaining({
        selected: false,
        folderPaths: ["/Example/Studio"],
      }),
    ),
  )
})

it("keeps a saved team folder bound to that team after membership is lost", async () => {
  const { port, ongoing, emitSync } = cloudProjectFixture()
  const group = projectGroup()
  const team = ongoingStatus({
    status: "ready",
    workspaceName: "Example Team",
    destinationId: "synthetic-team",
    consentVersion: "desktop-project-sync-v1",
    policyRevision: "synthetic-team-policy",
    projects: group.projects,
  })
  ongoing.status.mockResolvedValue(ongoingStatus({ teamStatuses: [team] }))
  port.workspaces = vi.fn(async () => [
    { id: "synthetic-personal", kind: "personal" as const, name: "Personal" },
    { id: "synthetic-other-team", kind: "team" as const, name: "Other Team" },
  ])
  ongoing.setProject.mockResolvedValue({
    ...team,
    status: "reauth_required",
    storageKnown: false,
    projects: [],
    policyRevision: "synthetic-team-off",
  })
  render(<Account session={new CloudSession(port, false, true)} />)
  const toggle = await screen.findByRole("switch", { name: /Sync Studio: .* to Example Team/ })
  expect(screen.getByRole("group", { name: "Example Team sync status" })).toHaveTextContent(
    "Workspace unavailable",
  )
  act(() => emitSync({ ...team, status: "reauth_required", storageKnown: false }))
  expect(screen.getByText("Example Team: Uploads are unavailable.")).toBeVisible()
  expect(screen.queryByRole("button", { name: /Confirm folders/ })).not.toBeInTheDocument()
  expect(toggle).toBeChecked()
  expect(toggle).toBeEnabled()
  expect(screen.queryByRole("combobox", { name: /Sync Studio/ })).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Sync now" })).toBeDisabled()

  fireEvent.click(toggle)
  await waitFor(() =>
    expect(ongoing.setProject).toHaveBeenCalledWith({
      accountId: "synthetic-account",
      destinationId: "synthetic-team",
      groupId: group.id,
      selected: false,
      projects: [],
      folderPaths: group.folderPaths,
      expectedPolicyRevision: "synthetic-team-policy",
    }),
  )
  const choice = await screen.findByRole("combobox", { name: /Sync Studio/ })
  expect(choice).toHaveValue("")
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it("can confirm verified saved Team folders even if folder discovery fails", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const group = projectGroup()
  ongoing.status.mockResolvedValue(
    ongoingStatus({
      teamStatuses: [
        ongoingStatus({
          status: "reauth_required",
          storageKnown: false,
          destinationId: "synthetic-team",
          workspaceName: "Example Team",
          consentVersion: "desktop-project-sync-v1",
          folderConfirmationRequired: true,
          projects: group.projects,
          savedFolders: [{ groupId: "saved-synthetic", path: group.path }],
        }),
      ],
    }),
  )
  ongoing.groups.mockRejectedValue(new Error("Synthetic discovery failure"))
  render(<Account session={new CloudSession(port, false, true)} />)
  expect(await screen.findByText(/Still checking session folders/)).toBeVisible()
  expect(screen.queryByRole("group", { name: "Personal sync status" })).not.toBeInTheDocument()
  expect(screen.getByRole("group", { name: "Example Team sync status" })).toHaveTextContent(
    "Review saved folders",
  )
  const confirm = screen.getByRole("button", { name: "Sync now" })
  expect(confirm).toBeEnabled()
  expect(screen.getByText("Sync now approves the listed Team folders.")).toBeVisible()
  fireEvent.click(confirm)
  await waitFor(() =>
    expect(ongoing.confirmTeamFolders).toHaveBeenCalledWith({
      accountId: "synthetic-account",
      destinationId: "synthetic-team",
      expectedPolicyRevision: "synthetic-policy",
      folderPaths: [group.path],
    }),
  )
})

it("lets a separate nested folder use Team even when the Personal parent is approved", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const group = projectGroup()
  const personal = ongoingStatus({
    status: "ready",
    consentVersion: "desktop-project-sync-v1",
    deviceUploadEnabled: true,
    projects: [{ ...group.projects[0]!, projectPath: "/Example" }],
  })
  ongoing.status.mockResolvedValue(personal)
  port.workspaces = vi.fn(async () => [
    { id: "synthetic-personal", kind: "personal" as const, name: "Personal" },
    { id: "synthetic-team", kind: "team" as const, name: "Example Team" },
  ])
  ongoing.enable.mockResolvedValue(
    ongoingStatus({
      destinationId: "synthetic-team",
      status: "ready",
      consentVersion: "desktop-project-sync-v1",
      projects: group.projects,
    }),
  )
  render(<Account session={new CloudSession(port, false, true)} />)
  const picker = await screen.findByRole("combobox", { name: /Sync Studio/ })
  const sync = screen.getByRole("button", { name: "Sync now" })
  expect(sync).toBeEnabled()
  fireEvent.change(picker, { target: { value: "synthetic-team" } })
  expect(sync).toBeEnabled()
  expect(ongoing.backfill).not.toHaveBeenCalled()
  expect(ongoing.enable).not.toHaveBeenCalled()
  fireEvent.click(sync)
  await waitFor(() =>
    expect(ongoing.enable).toHaveBeenCalledExactlyOnceWith(
      expect.objectContaining({ destinationId: "synthetic-team", projects: group.projects }),
    ),
  )
  await waitFor(() => expect(ongoing.backfill).toHaveBeenCalledTimes(2))
  expect(ongoing.enable.mock.invocationCallOrder[0]).toBeLessThan(
    ongoing.backfill.mock.invocationCallOrder[0]!,
  )
  expect(ongoing.backfill).toHaveBeenNthCalledWith(
    2,
    { accountId: "synthetic-account", destinationId: "synthetic-team" },
    "synthetic-policy",
  )
  expect(ongoing.setProject).not.toHaveBeenCalled()
  expect(ongoing.retry).not.toHaveBeenCalled()
})

it("can turn off a saved-only Personal folder whose source no longer appears", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const path = "/Example/Missing"
  const saved = ongoingStatus({
    status: "paused",
    paused: true,
    sourceConfirmationRequired: true,
    consentVersion: "desktop-project-sync-v1",
    projects: [{ ...projectGroup().projects[0]!, projectPath: path }],
    savedFolders: [{ groupId: "saved-missing", path }],
  })
  ongoing.status.mockResolvedValue(saved)
  ongoing.setProject.mockResolvedValue({ ...saved, projects: [], savedFolders: [] })
  render(<Account session={new CloudSession(port, false, true)} />)
  const toggle = await screen.findByRole("switch", { name: `Sync ${path} to Personal` })
  expect(toggle).toBeChecked()
  fireEvent.click(toggle)
  await waitFor(() => expect(toggle).not.toBeInTheDocument())
  expect(ongoing.setProject).toHaveBeenCalledExactlyOnceWith(
    expect.objectContaining({
      destinationId: "synthetic-personal",
      groupId: "saved-missing",
      selected: false,
      folderPaths: [path],
    }),
  )
})

it("does not report queued work as active without a syncing state", async () => {
  const { port, ongoing } = cloudProjectFixture()
  ongoing.status.mockResolvedValue(
    ongoingStatus({
      status: "ready",
      queuedCount: 3,
      consentVersion: "desktop-project-sync-v1",
      projects: projectGroup().projects,
    }),
  )
  render(<Account session={new CloudSession(port, false, true)} />)
  const folder = await screen.findByRole("switch", { name: /Sync Studio/ })
  expect(folder).toBeChecked()
  expect(screen.queryByRole("alert")).not.toBeInTheDocument()
  expect(screen.queryByRole("button", { name: /Retry/ })).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Sync now" })).toBeDisabled()
  expect(screen.queryByText("Uploading sessions")).not.toBeInTheDocument()
  expect(ongoing.retry).not.toHaveBeenCalled()
  expect(ongoing.setProject).not.toHaveBeenCalled()
})

it("rechecks only the exact saved source paths and keeps the device pause", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const saved = projectGroup().projects.slice(0, 1)
  ongoing.status.mockResolvedValue(
    ongoingStatus({
      status: "paused",
      paused: true,
      deviceUploadEnabled: false,
      sourceConfirmationRequired: true,
      consentVersion: "desktop-project-sync-v1",
      projects: saved,
    }),
  )
  ongoing.setProject.mockResolvedValue(
    ongoingStatus({
      status: "paused",
      paused: true,
      deviceUploadEnabled: false,
      sourceConfirmationRequired: false,
      consentVersion: "desktop-project-sync-v1",
      projects: saved,
    }),
  )
  render(<Account session={new CloudSession(port, false, true)} />)
  expect(await screen.findByText("Sync now checks changed sources.")).toBeVisible()
  const save = screen.getByRole("button", { name: "Sync now" })
  expect(save).toBeEnabled()
  fireEvent.click(save)
  await waitFor(() => expect(save).toBeDisabled())
  expect(ongoing.setProject).toHaveBeenCalledExactlyOnceWith(
    expect.objectContaining({ selected: true, projects: saved }),
  )
  expect(ongoing.setEnabled).not.toHaveBeenCalled()
  expect(ongoing.retry).not.toHaveBeenCalled()
})

it("does not pick a second stale Team after saving the chosen Team", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const teams = ["Design", "Engineering"].map((workspaceName) => {
    const path = `/Example/${workspaceName}`
    return ongoingStatus({
      destinationId: `team-${workspaceName}`,
      workspaceName,
      consentVersion: "desktop-project-sync-v1",
      folderConfirmationRequired: true,
      status: "reauth_required",
      projects: [{ ...projectGroup().projects[0]!, projectPath: path }],
      savedFolders: [{ groupId: `saved-${workspaceName}`, path }],
    })
  })
  port.workspaces = vi.fn(async () => [
    { id: "synthetic-personal", kind: "personal" as const, name: "Personal" },
    ...teams.map((team) => ({
      id: team.destinationId!,
      name: team.workspaceName!,
      kind: "team" as const,
    })),
  ])
  ongoing.status.mockResolvedValue(ongoingStatus({ teamStatuses: teams }))
  render(<Account session={new CloudSession(port, false, true)} />)
  const choice = await screen.findByRole("combobox", { name: "Save folders for" })
  const save = screen.getByRole("button", { name: "Sync now" })
  expect(save).toBeDisabled()
  fireEvent.change(choice, { target: { value: "team-Design" } })
  expect(save).toBeEnabled()
  fireEvent.click(save)
  await waitFor(() => expect(choice).toHaveValue(""))
  expect(save).toBeDisabled()
  expect(ongoing.confirmTeamFolders).toHaveBeenCalledExactlyOnceWith(
    expect.objectContaining({
      destinationId: "team-Design",
      folderPaths: ["/Example/Design"],
    }),
  )
  expect(ongoing.retry).not.toHaveBeenCalled()
})

it.each([false, true])(
  "restores the exact off control for a disconnected team folder without a discovered group: %s",
  async (overlappingGroup) => {
    const { port, ongoing } = cloudProjectFixture()
    const path = "/Example"
    const team = ongoingStatus({
      status: "reauth_required",
      storageKnown: false,
      workspaceName: "Example Team",
      destinationId: "synthetic-team",
      consentVersion: "desktop-project-sync-v1",
      projects: [{ ...projectGroup().projects[0]!, projectPath: path }],
      savedFolders: [{ groupId: "saved-synthetic-group", path }],
    })
    ongoing.status.mockResolvedValue(ongoingStatus({ teamStatuses: [team] }))
    if (!overlappingGroup)
      ongoing.groups.mockRejectedValue(new Error("Synthetic discovery failure"))
    ongoing.setProject.mockResolvedValue({ ...team, projects: [], savedFolders: [] })
    const session = new CloudSession(port, false, true)
    render(<Account session={session} />)
    if (overlappingGroup) {
      await waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
      expect(screen.getByRole("switch", { name: /Sync Studio/ })).not.toBeChecked()
    } else {
      expect(await screen.findByText(/Still checking session folders/)).toBeVisible()
    }
    const toggle = await screen.findByRole("switch", { name: `Sync ${path} to Example Team` })
    expect(toggle).toBeChecked()
    expect(toggle).toBeEnabled()
    toggle.focus()
    fireEvent.click(toggle)
    await waitFor(() =>
      expect(ongoing.setProject).toHaveBeenCalledWith(
        expect.objectContaining({
          destinationId: "synthetic-team",
          groupId: "saved-synthetic-group",
          selected: false,
          folderPaths: [path],
        }),
      ),
    )
    await waitFor(() => expect(toggle).not.toBeInTheDocument())
    await waitFor(() =>
      expect(screen.getByRole("group", { name: "Cloud folders" })).toHaveFocus(),
    )
    expect(ongoing.enable).not.toHaveBeenCalled()
  },
)

it.each([
  ["/Example", false],
  ["/Example/Studio", true],
  ["/Example/Studio/src", false],
  ["/Example/Stu", false],
  ["/Example/StudioSibling", false],
])(
  "shows a saved folder choice only for the exact displayed group: %s",
  async (projectPath, selected) => {
    const { port, ongoing } = cloudProjectFixture()
    ongoing.status.mockResolvedValue(
      ongoingStatus({
        status: "ready",
        consentVersion: "desktop-project-sync-v1",
        projects: [{ ...projectGroup().projects[0]!, projectPath: String(projectPath) }],
      }),
    )
    const session = new CloudSession(port, false, true)
    render(<Account session={session} />)
    const toggle = await screen.findByRole("switch", { name: /Sync Studio/ })
    expect(toggle).toHaveAttribute("aria-checked", String(selected))
  },
)

it("does not show a remote folder as approved by a matching local path", async () => {
  const { port, ongoing } = cloudProjectFixture()
  ongoing.status.mockResolvedValue(
    ongoingStatus({
      status: "ready",
      consentVersion: "desktop-project-sync-v1",
      projects: projectGroup().projects,
    }),
  )
  ongoing.groups.mockResolvedValue({
    groups: [
      projectGroup(),
      projectGroup({
        id: "remote-studio",
        environment: "ssh:synthetic",
        label: "Remote Studio",
        captureSupported: false,
        projects: [],
      }),
    ],
    sourceRoots: [],
    nextCursor: null,
    partial: false,
  })
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  expect(await screen.findByRole("switch", { name: /^Sync Studio:/ })).toBeChecked()
  expect(screen.queryByRole("switch", { name: /^Sync Remote Studio:/ })).toBeNull()
  expect(screen.getByText("No sessions ready to upload")).toBeVisible()
})

it("hides a missing folder even when sessions were found", async () => {
  const { port, ongoing } = cloudProjectFixture()
  ongoing.groups.mockResolvedValue({
    groups: [
      projectGroup({
        folderPaths: [],
        projects: [],
        sessionCount: 2,
        captureSupported: false,
        groupingComplete: false,
      }),
    ],
    sourceRoots: [],
    nextCursor: null,
    partial: false,
  })
  render(<Account session={new CloudSession(port, false, true)} />)
  expect(await screen.findByText(/No session folders found/)).toBeVisible()
  expect(screen.queryByText("2 sessions")).toBeNull()
  expect(screen.queryByText("/Example/Studio")).toBeNull()
  expect(screen.queryByRole("switch", { name: /^Sync Studio:/ })).toBeNull()
  expect(screen.queryByRole("combobox", { name: /^Sync Studio:/ })).toBeNull()
  expect(screen.getByRole("button", { name: "Start syncing" })).toBeDisabled()
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it.each([
  [
    "Project discovery is incomplete. Restore access or choose fewer sources and retry.",
    "Could not verify every selected folder.",
    "Review folders",
    2,
    1,
  ],
  [
    "The project folders changed. Reload projects and confirm again.",
    "The folders or session files changed. Review the folder choices.",
    "Review folders",
    2,
    1,
  ],
  [
    "Connect to Cloud before confirming sync.",
    "Could not reach Cloud to save your folders.",
    "Try again",
    1,
    2,
  ],
  ["Cloud is busy. Try again.", "Cloud is busy. Try again.", "Check Cloud", 2, 1],
  [
    "Synthetic provider error at /Example/private",
    "Could not save the folder choices.",
    "Try again",
    1,
    2,
  ],
])(
  "uses the primary button for recovery when the first save fails",
  async (failure, message, action, reads, saves) => {
    const { port, ongoing } = cloudProjectFixture()
    ongoing.enable.mockRejectedValueOnce(failure)
    render(<Account session={new CloudSession(port, false, true)} />)
    await screen.findByRole("switch", { name: /^Sync Studio:/ })
    const save = screen.getByRole("button", { name: "Start syncing" })
    const footer = within(save.closest("footer")!)
    fireEvent.click(save)
    expect(await screen.findByRole("alert")).toHaveTextContent(message)
    expect(footer.getByRole("button", { name: action })).toBe(save)
    expect(footer.getAllByRole("button")).toEqual([save])
    expect(save).toBeEnabled()
    expect(screen.queryByText(/Example\/private/)).toBeNull()
    expect(ongoing.enable).toHaveBeenCalledOnce()
    expect(ongoing.backfill).not.toHaveBeenCalled()
    fireEvent.click(save)
    await waitFor(() => {
      expect(ongoing.groups).toHaveBeenCalledTimes(reads)
      expect(ongoing.enable).toHaveBeenCalledTimes(saves)
    })
    if (action === "Review folders")
      expect(screen.getByRole("group", { name: "Cloud folders" })).toHaveFocus()
    if (action !== "Try again") {
      await waitFor(() => expect(footer.queryByRole("alert")).toBeNull())
      expect(footer.getByRole("button", { name: "Start syncing" })).toBe(save)
      expect(ongoing.backfill).not.toHaveBeenCalled()
    }
  },
)

it("saves the first folders before manually syncing their current policy", async () => {
  const { port, ongoing, emitSync } = cloudProjectFixture()
  const saved = ongoingStatus({
    status: "ready",
    consentVersion: "desktop-project-sync-v1",
    policyRevision: "newly-approved",
    deviceUploadEnabled: true,
    projects: projectGroup().projects,
  })
  let finishSave!: (status: typeof saved) => void
  ongoing.enable.mockImplementationOnce(() => new Promise((resolve) => (finishSave = resolve)))
  ongoing.backfill.mockResolvedValueOnce(saved)
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  await screen.findByRole("switch", { name: "Sync Studio: /Example/Studio" })
  const start = screen.getByRole("button", { name: "Start syncing" })
  const footer = within(start.closest("footer")!)
  const status = footer.getByRole("group", { name: "Folder sync status" })
  expect(start).toBeEnabled()
  expect(status).toHaveTextContent("Folder sync not started")
  expect(footer.queryByText("Personal", { selector: "p" })).not.toBeInTheDocument()
  expect(footer.getAllByRole("button")).toEqual([start])
  expect(ongoing.backfill).not.toHaveBeenCalled()
  expect(screen.queryByRole("checkbox")).not.toBeInTheDocument()
  expect(ongoing.enable).not.toHaveBeenCalled()
  fireEvent.click(start)
  await waitFor(() => expect(ongoing.enable).toHaveBeenCalledOnce())
  expect(ongoing.backfill).not.toHaveBeenCalled()
  await act(async () => finishSave(saved))
  await waitFor(() =>
    expect(ongoing.backfill).toHaveBeenCalledExactlyOnceWith(
      { accountId: saved.accountId, destinationId: saved.destinationId },
      saved.policyRevision,
    ),
  )
  await waitFor(() => expect(status).toHaveTextContent("Folder sync enabled"))
  expect(footer.getByRole("button", { name: "Sync now" })).toBe(start)
  expect(status).not.toHaveTextContent("0 accepted")
  expect(port.prepareSync).not.toHaveBeenCalled()
  expect(port.start).not.toHaveBeenCalled()
  act(() =>
    emitSync(
      ongoingStatus({
        status: "syncing",
        consentVersion: "desktop-project-sync-v1",
        acceptedCount: 1,
        queuedCount: 2,
        progress: {
          negotiated: true,
          totalCompressedBytes: 2000,
          requiredTransferBytes: 1000,
          reusedBytes: 1000,
          confirmedUploadBytes: 500,
        },
      }),
    ),
  )
  expect(session.getSnapshot().ongoing).toMatchObject({ status: "syncing", queuedCount: 2 })
  expect(screen.queryByText(/Sessions syncing|uploaded ·|Transferred/)).not.toBeInTheDocument()
  expect(
    screen.queryByRole("button", { name: /Refresh folders|Retry saved uploads/ }),
  ).not.toBeInTheDocument()
  expect(screen.queryByText("Your account is verified.")).not.toBeInTheDocument()
  expect(screen.queryByText(/Signing out disconnects/)).not.toBeInTheDocument()
  expect(
    screen.queryByRole("button", { name: "Choose Claude session folders…" }),
  ).not.toBeInTheDocument()
  expect(
    screen.queryByRole("button", { name: "Choose OpenCode databases…" }),
  ).not.toBeInTheDocument()
})

it("does not scan an unconfirmed folder save even when another saved workspace can sync", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const saved = ongoingStatus({
    status: "ready",
    consentVersion: "desktop-project-sync-v1",
    deviceUploadEnabled: true,
    projects: [{ ...projectGroup().projects[0]!, projectPath: "/Example/Other" }],
  })
  port.workspaces = vi.fn(async () => [
    { id: "synthetic-personal", name: "Personal", kind: "personal" as const },
    { id: "synthetic-team", name: "Example Team", kind: "team" as const },
  ])
  ongoing.status.mockResolvedValue(saved)
  ongoing.enable.mockResolvedValue(saved)
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  const picker = await screen.findByRole("combobox", { name: /Sync Studio/ })
  fireEvent.change(picker, { target: { value: "synthetic-team" } })
  const sync = screen.getByRole("button", { name: "Sync now" })
  fireEvent.click(sync)
  await waitFor(() => expect(ongoing.enable).toHaveBeenCalledOnce())
  await waitFor(() => expect(sync).not.toHaveAttribute("aria-busy"))
  expect(ongoing.backfill).not.toHaveBeenCalled()
})

it("keeps the cached-folder warning beside the unsaved folder sync status", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  await screen.findByRole("switch", { name: /Sync Studio/ })
  ongoing.groups.mockRejectedValue(new Error("Synthetic discovery failure"))
  await act(() => session.loadProjects(false, false))

  expect(screen.getByText("Studio", { selector: "strong" })).toBeVisible()
  expect(screen.queryByRole("switch", { name: /Sync Studio/ })).toBeNull()
  expect(screen.getByRole("alert")).toHaveTextContent(
    "Previously loaded folders are still shown.",
  )
  expect(screen.queryByRole("group", { name: "Personal sync status" })).not.toBeInTheDocument()
  expect(screen.getByRole("group", { name: "Folder sync status" })).toHaveTextContent(
    "Folder sync not started",
  )
  expect(screen.getByRole("button", { name: "Start syncing" })).toBeDisabled()
  expect(screen.queryByRole("button", { name: "Retry folders" })).not.toBeInTheDocument()
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it("keeps upload recovery available alongside a folder discovery warning", async () => {
  const { port, ongoing } = cloudProjectFixture()
  ongoing.status.mockResolvedValue(
    ongoingStatus({
      status: "fenced",
      consentVersion: "desktop-project-sync-v1",
      projects: projectGroup().projects,
      fencedCount: 1,
      retryAvailable: true,
      deviceUploadEnabled: true,
    }),
  )
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  await screen.findByRole("switch", { name: /Sync Studio/ })
  ongoing.groups.mockRejectedValue(new Error("Synthetic discovery failure"))
  await act(() => session.loadProjects(false, false))

  const footer = screen.getByRole("button", { name: "Sync now" }).closest("footer")!
  const alerts = within(footer).getAllByRole("alert")
  expect(alerts).toHaveLength(2)
  expect(alerts[0]).toHaveTextContent("Previously loaded folders are still shown.")
  expect(alerts[1]).toHaveTextContent("Some saved uploads are blocked.")
  expect(alerts[0]).toHaveClass("text-system-red-text")
  expect(alerts[1]).toHaveClass("text-system-red-text")
  fireEvent.click(within(footer).getByRole("button", { name: "Retry saved uploads" }))
  await waitFor(() => expect(ongoing.retry).toHaveBeenCalledOnce())
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it.each([
  {
    sourceKind: "claude_jsonl" as const,
    sourcePath: "/Example/claude/projects/studio",
    picker: "Choose Claude session folders…",
  },
  {
    sourceKind: "opencode_sqlite_v2" as const,
    sourcePath: "/Example/OpenCode/opencode.db",
    picker: "Choose OpenCode databases…",
  },
])(
  "uses automatic sources for a cached $sourceKind group without adding a picker",
  async ({ sourceKind, sourcePath }) => {
    const { port, ongoing } = cloudProjectFixture()
    const projects = projectGroup().folderPaths.map((projectPath) => ({
      sourceKind,
      sourcePath,
      projectPath,
    }))
    ongoing.groups.mockResolvedValue({
      groups: [projectGroup({ projects })],
      sourceRoots: [],
      nextCursor: null,
      partial: true,
    })
    port.pick = vi.fn(async () => [sourcePath])
    const session = new CloudSession(port, false, true)
    render(<Account session={session} />)
    expect(await screen.findByText("No sessions ready to upload")).toBeVisible()
    expect(screen.queryByRole("switch", { name: /Sync Studio/ })).toBeNull()
    const beforeAccess = session.getSnapshot()
    expect(screen.getByRole("button", { name: "Start syncing" })).toBeDisabled()
    expect(screen.queryByText("No session folders found.")).not.toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "Refresh folders" })).not.toBeInTheDocument()
    expect(
      screen.queryByRole("button", { name: /Choose Claude|Choose OpenCode/ }),
    ).not.toBeInTheDocument()
    expect(port.pick).not.toHaveBeenCalled()
    expect(ongoing.enable).not.toHaveBeenCalled()
    await act(() => session.startSync(beforeAccess))
    expect(ongoing.enable).not.toHaveBeenCalled()
  },
)

it("keeps source access reachable for an unqualified group beside a qualified group", async () => {
  const { port, ongoing } = cloudProjectFixture()
  ongoing.groups.mockResolvedValue({
    groups: [
      projectGroup(),
      projectGroup({
        id: "website",
        label: "Website",
        path: "/Example/Website",
        folderPaths: ["/Example/Website"],
        worktreeCount: 0,
        projects: [
          {
            sourceKind: "opencode_sqlite_v2",
            sourcePath: "/Example/OpenCode/opencode.db",
            projectPath: "/Example/Website",
          },
        ],
      }),
    ],
    sourceRoots: [{ sourceKind: "claude_jsonl", path: "/Example/claude/projects/studio" }],
    nextCursor: { environment: "native", cwd: "/Example/Next" },
    partial: true,
  })
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  expect(await screen.findByText("No sessions ready to upload")).toBeVisible()
  expect(screen.queryByRole("switch", { name: /Sync Website/ })).toBeNull()
  expect(screen.getByRole("button", { name: "Start syncing" })).toBeEnabled()
  expect(
    screen.queryByRole("button", { name: /Choose Claude|Choose OpenCode/ }),
  ).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Load more folders" })).toBeEnabled()
  expect(screen.queryByRole("button", { name: "Refresh folders" })).not.toBeInTheDocument()
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it("saves the verified folders while a later page is retrying", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const group = projectGroup()
  ongoing.groups.mockRejectedValue(new Error("Page unavailable")).mockResolvedValueOnce({
    groups: [group],
    sourceRoots: [{ sourceKind: "claude_jsonl", path: group.projects[0]!.sourcePath }],
    nextCursor: { environment: "native", cwd: "/Example/Next" },
    partial: true,
  })
  render(<Account session={new CloudSession(port, false, true)} />)
  fireEvent.click(await screen.findByRole("button", { name: "Load more folders" }))
  expect(await screen.findByText("Still checking for more folders.")).toBeVisible()
  const save = screen.getByRole("button", { name: "Start syncing" })
  expect(save).toBeEnabled()
  fireEvent.click(save)
  await waitFor(() => expect(ongoing.enable).toHaveBeenCalledOnce())
  expect(ongoing.enable).toHaveBeenCalledWith(
    expect.objectContaining({ projects: group.projects }),
  )
  expect(screen.queryByText("Still checking for more folders.")).not.toBeInTheDocument()
})

it("accepts Start syncing while a later page is in flight without racing the native read", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const group = projectGroup()
  let finish!: (error: Error) => void
  ongoing.groups
    .mockResolvedValueOnce({
      groups: [group],
      sourceRoots: [{ sourceKind: "claude_jsonl", path: group.projects[0]!.sourcePath }],
      nextCursor: { environment: "native", cwd: "/Example/Next" },
      partial: true,
    })
    .mockImplementationOnce(() => new Promise((_, reject) => (finish = reject)))
  render(<Account session={new CloudSession(port, false, true)} />)
  fireEvent.click(await screen.findByRole("button", { name: "Load more folders" }))
  expect(await screen.findByText("Loading more folders…")).toBeVisible()
  const save = screen.getByRole("button", { name: "Start syncing" })
  expect(save).toBeEnabled()
  fireEvent.click(save)
  expect(ongoing.enable).not.toHaveBeenCalled()
  await act(async () => finish(new Error("Page unavailable")))
  await waitFor(() => expect(ongoing.enable).toHaveBeenCalledOnce())
  expect(ongoing.groups).toHaveBeenCalledTimes(2)
})

it("retries only the saved desktop policy when a new external worktree awaits confirmation", async () => {
  const { port, ongoing, emitSync } = cloudProjectFixture()
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  await screen.findByRole("switch")
  fireEvent.click(screen.getByRole("button", { name: "Start syncing" }))
  await waitFor(() => expect(ongoing.enable).toHaveBeenCalledOnce())
  act(() =>
    emitSync(
      ongoingStatus({
        status: "fenced",
        retryAvailable: true,
        deviceUploadEnabled: true,
        consentVersion: "desktop-project-sync-v1",
        projectChoices: { studio: true },
        projects: projectGroup().projects,
      }),
    ),
  )
  expect(screen.getByRole("alert")).toHaveTextContent("Some saved uploads are blocked.")
  ongoing.status.mockResolvedValue(
    ongoingStatus({
      status: "fenced",
      retryAvailable: true,
      deviceUploadEnabled: true,
      consentVersion: "desktop-project-sync-v1",
      projectChoices: { studio: true },
    }),
  )
  ongoing.groups.mockResolvedValue({
    groups: [
      projectGroup({
        folderPaths: [...projectGroup().folderPaths, "/Example/external/new-worktree"],
        projects: [
          ...projectGroup().projects,
          {
            sourceKind: "claude_jsonl",
            sourcePath: "/Example/claude/projects/studio",
            projectPath: "/Example/external/new-worktree",
          },
        ],
        worktreeCount: 2,
      }),
    ],
    sourceRoots: [{ sourceKind: "claude_jsonl", path: "/Example/claude/projects/studio" }],
    nextCursor: null,
    partial: false,
  })
  await act(() => session.loadProjects())
  await screen.findByText("2 worktrees")
  expect(screen.queryByRole("checkbox")).not.toBeInTheDocument()
  expect(
    screen.queryByRole("button", { name: "Update sync selection" }),
  ).not.toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "Retry saved uploads" }))
  await waitFor(() => expect(ongoing.retry).toHaveBeenCalledOnce())
  expect(ongoing.enable).toHaveBeenCalledOnce()
})

it("syncs the saved revision without retrying or changing upload settings", async () => {
  const { port, ongoing, emitSync } = cloudProjectFixture()
  const saved = ongoingStatus({
    status: "ready",
    consentVersion: "desktop-project-sync-v1",
    deviceUploadEnabled: true,
    projects: projectGroup().projects,
    projectChoices: { studio: true },
  })
  ongoing.status.mockResolvedValue(saved)
  let finishBackfill!: (status: typeof saved) => void
  ongoing.backfill.mockImplementationOnce(
    () => new Promise((resolve) => (finishBackfill = resolve)),
  )
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  const button = await screen.findByRole("button", { name: "Sync now" })
  expect(button).not.toHaveAttribute("aria-disabled")
  fireEvent.click(button)

  expect(button).toHaveAttribute("aria-busy", "true")
  expect(screen.getByText("Starting sync…")).toHaveAttribute("role", "status")
  await act(async () => finishBackfill({ ...saved, backfillInProgress: true }))
  await waitFor(() => expect(session.getSnapshot().busy).toBe(false))
  expect(button).toHaveAttribute("aria-disabled", "true")
  expect(screen.getByText("Scanning folders…")).toHaveAttribute("role", "status")
  act(() => emitSync({ ...saved, backfillInProgress: false }))
  expect(button).not.toHaveAttribute("aria-disabled")
  expect(ongoing.backfill).toHaveBeenCalledExactlyOnceWith(
    { accountId: saved.accountId, destinationId: saved.destinationId },
    saved.policyRevision,
  )
  expect(ongoing.retry).not.toHaveBeenCalled()
  expect(ongoing.setEnabled).not.toHaveBeenCalled()
  expect(ongoing.enable).not.toHaveBeenCalled()
  const oldView = session.getSnapshot()
  act(() => emitSync({ ...saved, policyRevision: "new-revision" }))
  await act(() => session.syncNow(oldView))
  expect(ongoing.backfill).toHaveBeenCalledOnce()
  act(() => emitSync({ ...saved, status: "paused", paused: true, deviceUploadEnabled: false }))
  expect(button).toBeVisible()
  expect(button).toBeDisabled()
  expect(screen.getByRole("group", { name: "Folder sync status" })).toHaveTextContent(
    "Folder sync paused",
  )
  fireEvent.click(button)
  expect(ongoing.backfill).toHaveBeenCalledOnce()
})

it("retries a failed scan through the same button without approving a new draft", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const studio = projectGroup()
  const websitePath = "/Example/Website"
  ongoing.groups.mockResolvedValue({
    groups: [
      studio,
      projectGroup({
        id: "website",
        label: "Website",
        path: websitePath,
        folderPaths: [websitePath],
        projects: [{ ...studio.projects[0]!, projectPath: websitePath }],
      }),
    ],
    sourceRoots: [{ sourceKind: "claude_jsonl", path: studio.projects[0]!.sourcePath }],
    nextCursor: null,
    partial: false,
  })
  const saved = ongoingStatus({
    status: "ready",
    consentVersion: "desktop-project-sync-v1",
    deviceUploadEnabled: true,
    projects: studio.projects,
    projectChoices: { studio: true },
  })
  ongoing.status.mockResolvedValue(saved)
  ongoing.backfill
    .mockRejectedValueOnce(new Error("Synthetic failure"))
    .mockResolvedValue(saved)
  render(<Account session={new CloudSession(port, false, true)} />)
  const sync = await screen.findByRole("button", { name: "Sync now" })
  const footer = within(sync.closest("footer")!)
  fireEvent.click(sync)
  expect(await footer.findByRole("alert")).toHaveTextContent("Could not start sync.")
  expect(footer.getByRole("button", { name: "Try again" })).toBe(sync)
  expect(footer.getAllByRole("button")).toEqual([sync])
  expect(sync).toBeEnabled()
  const website = screen.getByRole("switch", { name: /Sync Website/ })
  expect(website).not.toBeChecked()
  fireEvent.click(website)
  expect(website).toBeChecked()
  fireEvent.click(sync)
  await waitFor(() => expect(footer.queryByRole("alert")).toBeNull())
  expect(ongoing.backfill).toHaveBeenCalledTimes(2)
  expect(ongoing.enable).not.toHaveBeenCalled()
  expect(ongoing.setProject).not.toHaveBeenCalled()
  expect(website).toBeChecked()
})

it("reviews changed saved folders without writing or scanning again", async () => {
  const { port, ongoing } = cloudProjectFixture()
  ongoing.status.mockResolvedValue(
    ongoingStatus({
      status: "ready",
      consentVersion: "desktop-project-sync-v1",
      deviceUploadEnabled: true,
      projects: projectGroup().projects,
    }),
  )
  ongoing.backfill.mockRejectedValueOnce(
    new Error("Cloud folder choices changed. Reload before retrying saved work."),
  )
  render(<Account session={new CloudSession(port, false, true)} />)
  const sync = await screen.findByRole("button", { name: "Sync now" })
  fireEvent.click(sync)
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "The saved folder choices changed.",
  )
  expect(screen.getByRole("button", { name: "Review folders" })).toBe(sync)
  fireEvent.click(sync)
  await waitFor(() => expect(screen.queryByRole("alert")).toBeNull())
  expect(screen.getByRole("button", { name: "Sync now" })).toBe(sync)
  expect(screen.getByRole("group", { name: "Cloud folders" })).toHaveFocus()
  expect(ongoing.groups).toHaveBeenCalledTimes(2)
  expect(ongoing.backfill).toHaveBeenCalledOnce()
  expect(ongoing.enable).not.toHaveBeenCalled()
  expect(ongoing.setProject).not.toHaveBeenCalled()
})

it("shows each workspace's own session upload percentage beside one sync button", async () => {
  const { port, ongoing } = cloudProjectFixture()
  port.workspaces = vi.fn(async () => [
    { id: "synthetic-personal", name: "Personal", kind: "personal" as const },
    { id: "synthetic-team", name: "Example Team", kind: "team" as const },
  ])
  const saved = ongoingStatus({
    status: "ready",
    consentVersion: "desktop-project-sync-v1",
    deviceUploadEnabled: true,
    projects: projectGroup().projects,
    projectChoices: { studio: true },
    acceptedCount: 100,
    uploadProgress: { uploaded: 3, total: 4 },
  })
  ongoing.status.mockResolvedValue({
    ...saved,
    teamStatuses: [
      {
        ...saved,
        destinationId: "synthetic-team",
        workspaceName: "Example Team",
        uploadProgress: { uploaded: 2, total: 8 },
      },
    ],
  })
  render(<Account session={new CloudSession(port, false, true)} />)
  const button = await screen.findByRole("button", { name: "Sync now" })
  expect(within(button.closest("footer")!).getAllByRole("button")).toEqual([button])
  expect(screen.getByRole("group", { name: "Personal sync status" })).toHaveTextContent(
    /^Personal · /,
  )
  expect(screen.getByRole("group", { name: "Example Team sync status" })).toHaveTextContent(
    /^Example Team · /,
  )
  const personal = screen.getByRole("progressbar", {
    name: "Session upload progress for Personal",
  })
  expect(personal).toHaveAttribute("aria-valuenow", "3")
  expect(personal).toHaveAttribute("aria-valuemax", "4")
  expect(personal).toHaveAttribute("aria-valuetext", "3 of 4 sessions accepted (75%)")
  expect(screen.getByText("3 of 4 sessions accepted")).toBeVisible()
  expect(screen.getByText("75%")).toBeVisible()
  const team = screen.getByRole("progressbar", {
    name: "Session upload progress for Example Team",
  })
  expect(team).toHaveAttribute("aria-valuenow", "2")
  expect(team).toHaveAttribute("aria-valuemax", "8")
  expect(team).toHaveAttribute("aria-valuetext", "2 of 8 sessions accepted (25%)")
  expect(screen.getByText("25%")).toBeVisible()
})

it("shows accepted sessions without a percentage until the scan has a nonzero total", async () => {
  const { port, ongoing, emitSync } = cloudProjectFixture()
  const saved = ongoingStatus({
    status: "ready",
    consentVersion: "desktop-project-sync-v1",
    deviceUploadEnabled: true,
    projects: projectGroup().projects,
    projectChoices: { studio: true },
  })
  ongoing.status.mockResolvedValue(saved)
  render(<Account session={new CloudSession(port, false, true)} />)
  await screen.findByRole("button", { name: "Sync now" })

  act(() =>
    emitSync({
      ...saved,
      backfillInProgress: true,
      uploadProgress: { uploaded: 1, total: null },
    }),
  )
  const status = screen.getByText("Scanning folders…")
  expect(status).toBeVisible()
  expect(status).toHaveAttribute("role", "status")
  expect(screen.getByText("1 session accepted")).toBeVisible()
  expect(screen.queryByRole("progressbar")).not.toBeInTheDocument()

  act(() => emitSync({ ...saved, uploadProgress: { uploaded: 2, total: null } }))
  expect(status).toHaveTextContent("Personal: Session total unavailable.")
  expect(screen.getByText("2 sessions accepted. Session total unavailable.")).toBeVisible()
  expect(screen.queryByRole("progressbar")).not.toBeInTheDocument()

  act(() => emitSync({ ...saved, uploadProgress: { uploaded: 0, total: 0 } }))
  expect(screen.getByText("No sessions found to sync.", { selector: "p" })).toBeVisible()
  expect(status).toHaveTextContent("Personal: No sessions found to sync.")
  expect(screen.queryByRole("progressbar")).not.toBeInTheDocument()
  expect(screen.queryByText(/%$/)).not.toBeInTheDocument()

  const count = new Intl.NumberFormat().format(1000)
  act(() => emitSync({ ...saved, uploadProgress: { uploaded: 998, total: 1000 } }))
  expect(status).toHaveTextContent(`Personal: Scan complete. ${count} sessions found.`)
  const statusText = status.firstChild
  act(() => emitSync({ ...saved, uploadProgress: { uploaded: 999, total: 1000 } }))
  expect(status.firstChild).toBe(statusText)
  expect(status).toHaveTextContent(`Personal: Scan complete. ${count} sessions found.`)
  expect(screen.getByText(">99%")).toBeVisible()
  expect(screen.queryByText("100%")).not.toBeInTheDocument()

  act(() => emitSync({ ...saved, uploadProgress: { uploaded: 1000, total: 1000 } }))
  expect(status).toHaveTextContent(`Personal: ${count} sessions accepted.`)
  expect(screen.getByText("100%")).toBeVisible()

  act(() =>
    emitSync({
      ...saved,
      status: "storage_unavailable",
      storageKnown: false,
      uploadProgress: { uploaded: 1000, total: 1000 },
    }),
  )
  expect(screen.queryByRole("progressbar")).not.toBeInTheDocument()
  expect(screen.queryByText("100%")).not.toBeInTheDocument()
})

it("hides cached totals and progress after saved upload storage becomes unknown", async () => {
  const { port, ongoing, emitSync } = cloudProjectFixture()
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  await screen.findByRole("switch")
  const healthy = ongoingStatus({
    status: "ready",
    storageKnown: true,
    consentVersion: "desktop-project-sync-v1",
    deviceUploadEnabled: true,
    projects: projectGroup().projects,
    projectChoices: { studio: true },
    acceptedCount: 7,
    queuedCount: 2,
    progress: {
      negotiated: true,
      totalCompressedBytes: 2000,
      requiredTransferBytes: 1000,
      reusedBytes: 1000,
      confirmedUploadBytes: 500,
    },
  })
  act(() => emitSync(healthy))
  expect(session.getSnapshot().ongoing?.acceptedCount).toBe(7)
  const status = screen.getByRole("group", { name: "Folder sync status" })
  expect(status).toHaveTextContent("Folder sync enabled · 7 accepted")
  expect(status).toHaveTextContent("2 queued")
  expect(screen.queryByText(/7 uploaded|Transferred/)).not.toBeInTheDocument()
  const unavailable = {
    ...healthy,
    status: "storage_unavailable" as const,
    storageKnown: false,
  }
  ongoing.status.mockResolvedValueOnce(unavailable).mockResolvedValue(healthy)
  act(() => emitSync(unavailable))
  expect(screen.getByText("Checking saved uploads…")).toBeVisible()
  expect(status).not.toHaveTextContent("7 accepted")
  expect(status).not.toHaveTextContent("2 queued")
  expect(screen.queryByText(/7 uploaded|0 uploaded|Transferred/)).not.toBeInTheDocument()
  expect(screen.queryByText("Background sync enabled")).not.toBeInTheDocument()
  expect(screen.queryByRole("button", { name: "Check saved uploads" })).not.toBeInTheDocument()
  expect(
    screen.queryByRole("button", { name: "Update sync selection" }),
  ).not.toBeInTheDocument()
  expect(screen.queryByRole("button", { name: "Refresh folders" })).not.toBeInTheDocument()
  expect(screen.queryByRole("switch", { name: /Sync Studio/ })).toBeNull()
  await waitFor(() => expect(session.getSnapshot().ongoing?.storageKnown).toBe(true))
  expect(screen.getByRole("switch", { name: /Sync Studio/ })).toBeEnabled()
  expect(session.getSnapshot().ongoing?.acceptedCount).toBe(7)
  expect(ongoing.retry).not.toHaveBeenCalled()
})

it("reads folders on verified-account mount without a manual refresh action", async () => {
  const { port, ongoing, emit } = cloudProjectFixture()
  emit({ identity: null, status: "signed_out" })
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  await screen.findByRole("button", { name: "Sign in with GitHub" })
  expect(ongoing.groups).not.toHaveBeenCalled()
  act(() =>
    emit({
      identity: { id: "synthetic-account", login: "Example" },
      status: "signed_in",
    }),
  )
  expect(await screen.findByRole("switch", { name: /Sync Studio/ })).toBeVisible()
  expect(ongoing.groups).toHaveBeenCalledOnce()
  expect(screen.queryByRole("button", { name: "Refresh folders" })).not.toBeInTheDocument()
  expect(ongoing.enable).not.toHaveBeenCalled()
  expect(port.save).not.toHaveBeenCalled()
})

it("keeps cached exclusions during a failed read and leaves new groups unapproved", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const website = projectGroup({
    id: "website",
    label: "Website",
    path: "/Example/Website",
    folderPaths: ["/Example/Website"],
    worktreeCount: 0,
    projects: [
      {
        sourceKind: "claude_jsonl",
        sourcePath: "/Example/claude/projects/studio",
        projectPath: "/Example/Website",
      },
    ],
  })
  const tools = projectGroup({
    ...website,
    id: "tools",
    label: "Tools",
    path: "/Example/Tools",
    folderPaths: ["/Example/Tools"],
    projects: [{ ...website.projects[0]!, projectPath: "/Example/Tools" }],
  })
  const page = {
    groups: [projectGroup(), website],
    sourceRoots: [
      { sourceKind: "claude_jsonl" as const, path: "/Example/claude/projects/studio" },
    ],
    nextCursor: null,
    partial: false,
  }
  const saved = ongoingStatus({
    status: "ready",
    consentVersion: "desktop-project-sync-v1",
    projectChoices: { studio: false, website: true },
    projects: website.projects,
    selectedSources: page.sourceRoots,
  })
  ongoing.status.mockResolvedValue(saved)
  ongoing.groups.mockResolvedValue(page)
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  const studio = await screen.findByRole("switch", { name: /Sync Studio/ })
  expect(studio).not.toBeChecked()
  let fail!: (error: Error) => void
  ongoing.groups.mockImplementationOnce(
    () =>
      new Promise<Awaited<ReturnType<typeof ongoing.groups>>>((_, reject) => {
        fail = reject
      }),
  )
  let pending!: Promise<void>
  act(() => {
    pending = session.loadProjects()
  })
  await waitFor(() => expect(ongoing.groups).toHaveBeenCalledTimes(2))
  expect(session.getSnapshot().projects?.loading).toBe(true)
  expect(studio).toBeVisible()
  expect(studio).not.toBeChecked()
  expect(screen.queryByText("No session folders found.")).not.toBeInTheDocument()
  await act(async () => {
    ongoing.groups.mockResolvedValueOnce({ ...page, groups: [...page.groups, tools] })
    fail(new Error("Synthetic read failure"))
    await pending
  })
  expect(screen.queryByRole("alert")).not.toBeInTheDocument()
  expect(studio).not.toBeChecked()
  expect(
    screen.queryByRole("button", { name: "Choose Claude session folders…" }),
  ).not.toBeInTheDocument()
  expect(screen.queryByRole("button", { name: "Retry folder list" })).not.toBeInTheDocument()
  expect(await screen.findByRole("switch", { name: /Sync Tools/ })).not.toBeChecked()
  expect(studio).not.toBeChecked()
  expect(screen.getByRole("switch", { name: /Sync Website/ })).toBeChecked()
  expect(ongoing.enable).not.toHaveBeenCalled()
  expect(port.save).not.toHaveBeenCalled()
  expect(session.getSnapshot().ongoing?.projects).toEqual(website.projects)
  expect(session.getSnapshot().ongoing?.projectChoices).toEqual({
    studio: false,
    website: true,
  })
})

it("does not turn an initial folder read failure into an empty list", async () => {
  const { port, ongoing } = cloudProjectFixture()
  ongoing.status.mockResolvedValue(
    ongoingStatus({
      status: "interrupted",
      consentVersion: "desktop-project-sync-v1",
      interruptedCount: 1,
      recoveryRequiredCount: 1,
    }),
  )
  ongoing.groups.mockRejectedValue(new Error("Synthetic read failure"))
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  expect(await screen.findByText(/Still checking session folders/)).toBeVisible()
  expect(screen.queryByText("No session folders found.")).not.toBeInTheDocument()
  expect(
    screen.queryByRole("button", { name: "Choose Claude session folders…" }),
  ).not.toBeInTheDocument()
  expect(screen.queryByRole("button", { name: "Retry folder list" })).not.toBeInTheDocument()
  expect(screen.getByText(/Some sessions need local recovery/)).toBeVisible()
  expect(screen.queryByRole("button", { name: "Retry saved uploads" })).not.toBeInTheDocument()
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it("retries an incomplete empty folder listing without saving upload permission", async () => {
  const { port, ongoing } = cloudProjectFixture()
  ongoing.groups.mockResolvedValueOnce({
    groups: [],
    sourceRoots: [],
    nextCursor: null,
    partial: true,
  })
  let finish!: (page: Awaited<ReturnType<typeof ongoing.groups>>) => void
  ongoing.groups.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve
      }),
  )
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  await waitFor(() => expect(ongoing.groups).toHaveBeenCalledTimes(2))
  expect(ongoing.groups).toHaveBeenLastCalledWith(expect.anything(), null)
  expect(screen.getByText("Loading folders…")).toBeVisible()
  expect(screen.queryByRole("alert")).not.toBeInTheDocument()
  expect(screen.queryByText(/No session folders found/)).not.toBeInTheDocument()
  await act(async () =>
    finish({
      groups: [projectGroup()],
      sourceRoots: [{ sourceKind: "claude_jsonl", path: "/Example/claude/projects/studio" }],
      nextCursor: null,
      partial: false,
    }),
  )
  expect(await screen.findByRole("switch", { name: /Sync Studio/ })).toBeChecked()
  expect(screen.queryByRole("alert")).not.toBeInTheDocument()
  expect(screen.queryByRole("button", { name: "Retry folder list" })).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Start syncing" })).toBeEnabled()
  expect(ongoing.retry).not.toHaveBeenCalled()
  expect(ongoing.setProject).not.toHaveBeenCalled()
  expect(ongoing.enable).not.toHaveBeenCalled()
  expect(port.save).not.toHaveBeenCalled()
})

it("does not claim an empty discovery with another page is complete", async () => {
  const { port, ongoing } = cloudProjectFixture()
  ongoing.groups.mockResolvedValue({
    groups: [],
    sourceRoots: [],
    partial: false,
    nextCursor: { environment: "native", cwd: "/Example/Next" },
  })
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  await waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
  expect(screen.queryByText("No session folders found.")).not.toBeInTheDocument()
  expect(
    screen.queryByRole("button", { name: "Choose Claude session folders…" }),
  ).not.toBeInTheDocument()
  expect(screen.queryByText(/Could not|unavailable|blocked/)).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Load more folders" })).toBeEnabled()
  expect(screen.queryByRole("button", { name: "Retry folder list" })).not.toBeInTheDocument()
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it.each([
  { status: "syncing" as const, fencedCount: 1, message: "Some saved uploads are blocked." },
  {
    status: "paused" as const,
    interruptedCount: 1,
    message: "Some sessions could not be prepared.",
  },
])(
  "keeps an actual saved-work failure visible while $status",
  async ({ message, ...patch }) => {
    const { port, ongoing } = cloudProjectFixture()
    ongoing.status.mockResolvedValue(
      ongoingStatus({
        consentVersion: "desktop-project-sync-v1",
        projectChoices: { studio: true },
        projects: projectGroup().projects,
        retryAvailable: patch.status !== "paused",
        deviceUploadEnabled: patch.status !== "paused",
        ...patch,
      }),
    )
    const session = new CloudSession(port, false, true)
    render(<Account session={session} />)
    expect(await screen.findByText(message)).toBeVisible()
    expect(screen.getAllByText(message)).toHaveLength(1)
    if (patch.status === "paused")
      expect(screen.queryByRole("button", { name: "Retry saved uploads" })).toBeNull()
    else expect(screen.getByRole("button", { name: "Retry saved uploads" })).toBeEnabled()
    expect(screen.queryByText(/uploaded ·|Transferred/)).not.toBeInTheDocument()
  },
)

it.each([
  { status: "interrupted" as const, paused: false },
  { status: "paused" as const, paused: true },
  { status: "syncing" as const, paused: false },
])("does not offer upload retry for recovery-only jobs while $status", async (patch) => {
  const { port, ongoing } = cloudProjectFixture()
  ongoing.status.mockResolvedValue(
    ongoingStatus({
      consentVersion: "desktop-project-sync-v1",
      projectChoices: { studio: true },
      projects: projectGroup().projects,
      interruptedCount: 2,
      recoveryRequiredCount: 2,
      ...patch,
    }),
  )
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  const message = await screen.findByText(
    "Some sessions need local recovery. Saved data is kept.",
  )
  expect(message.closest('[role="alert"]')).toHaveClass("text-system-red-text")
  expect(screen.queryByText("Some sessions could not be prepared.")).not.toBeInTheDocument()
  expect(screen.queryByRole("button", { name: "Retry saved uploads" })).not.toBeInTheDocument()
  expect(screen.getByRole("switch", { name: /Sync Studio/ })).toBeEnabled()
  expect(ongoing.retry).not.toHaveBeenCalled()
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it.each([
  { status: "interrupted" as const, interruptedCount: 2 },
  { status: "interrupted" as const, queuedCount: 1 },
  { status: "syncing" as const, fencedCount: 1 },
  { status: "fenced" as const },
  { status: "upload_unavailable" as const },
  { status: "route_denied" as const },
])("keeps upload retry for other saved-work problems: %j", async (patch) => {
  const { port, ongoing } = cloudProjectFixture()
  ongoing.status.mockResolvedValue(
    ongoingStatus({
      consentVersion: "desktop-project-sync-v1",
      projectChoices: { studio: true },
      projects: projectGroup().projects,
      interruptedCount: 1,
      recoveryRequiredCount: 1,
      retryAvailable: true,
      deviceUploadEnabled: true,
      ...patch,
    }),
  )
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  const retry = await screen.findByRole("button", { name: "Retry saved uploads" })
  expect(retry).toBeEnabled()
  if (patch.status === "route_denied") {
    expect(screen.getByRole("alert")).toHaveTextContent(
      "Cloud denied uploads to this workspace. Check your workspace access.",
    )
  } else if (patch.status === "interrupted") {
    expect(screen.getByRole("alert")).toHaveTextContent(
      "Some sessions need local recovery. Saved data is kept.",
    )
  }
  fireEvent.click(retry)
  await waitFor(() => expect(ongoing.retry).toHaveBeenCalledOnce())
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it("keeps interrupted upload retry compatible with an older status without a recovery count", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const saved = ongoingStatus({
    status: "interrupted",
    consentVersion: "desktop-project-sync-v1",
    projectChoices: { studio: true },
    projects: projectGroup().projects,
    interruptedCount: 1,
    retryAvailable: true,
    deviceUploadEnabled: true,
  })
  delete saved.recoveryRequiredCount
  ongoing.status.mockResolvedValue(saved)
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  expect(await screen.findByText("Some sessions could not be prepared.")).toBeVisible()
  expect(screen.queryByText(/Some sessions need local recovery/)).not.toBeInTheDocument()
  const retry = screen.getByRole("button", { name: "Retry saved uploads" })
  expect(retry).toBeEnabled()
  fireEvent.click(retry)
  await waitFor(() => expect(ongoing.retry).toHaveBeenCalledOnce())
})

it("does not expose an unsupported team destination or route it to personal", async () => {
  const { port, ongoing, emit } = cloudProjectFixture()
  emit({ destination: { id: "requested-team", name: "Example team", kind: "team" } })
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  expect(
    await screen.findByText("Personal uploads are unavailable for this connection."),
  ).toBeVisible()
  expect(screen.queryByText("Example team")).not.toBeInTheDocument()
  expect(screen.queryByText("Personal", { exact: true })).not.toBeInTheDocument()
  expect(screen.queryByRole("switch")).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Sign out" })).toBeEnabled()
  expect(ongoing.groups).not.toHaveBeenCalled()
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it("focuses a folder from Settings search without changing its choice", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const session = new CloudSession(port, false, true)
  const container = document.createElement("div")
  document.body.append(container)
  render(<Account session={session} />, { container })
  const folder = await screen.findByRole("switch", { name: /Sync Studio/ })
  const row = screen.getByText("Cloud folders").closest<HTMLElement>("[data-settings-control]")!
  row.scrollIntoView = vi.fn()
  new SettingsTargetFocus().attach(container, "sessions", "cloudFolders", 1)()
  expect(folder).toHaveFocus()
  expect(folder).toBeChecked()
  expect(ongoing.enable).not.toHaveBeenCalled()
  expect(port.save).not.toHaveBeenCalled()
})

it("retries the first account read without showing a coming-soon state", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const read = port.read
  port.read = vi
    .fn<typeof port.read>()
    .mockRejectedValueOnce(new Error("Synthetic account read failure"))
    .mockImplementation(read)
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  expect(await screen.findByText("Checking Cloud sign-in…")).toBeVisible()
  expect(screen.queryByText("Antiburn Cloud is coming soon")).not.toBeInTheDocument()
  expect(screen.queryByRole("button", { name: "Learn more" })).not.toBeInTheDocument()
  expect(await screen.findByRole("switch", { name: /Sync Studio/ })).toBeVisible()
  expect(port.read).toHaveBeenCalledTimes(2)
  expect(ongoing.groups).toHaveBeenCalledOnce()
  expect(ongoing.enable).not.toHaveBeenCalled()
})
