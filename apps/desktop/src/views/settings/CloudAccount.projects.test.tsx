import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
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

it("keeps Save mounted after saving to Team and lets the user choose Personal after turning it off", async () => {
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
  ongoing.setProject.mockResolvedValueOnce({ ...team, projects: [] })
  render(<Account session={new CloudSession(port, false, true)} />)
  const folderPicker = () => screen.findByRole("combobox", { name: /Sync Studio/ })
  fireEvent.change(await folderPicker(), { target: { value: "synthetic-team" } })
  expect(ongoing.enable).not.toHaveBeenCalled()
  await waitFor(() => expect(screen.getByRole("button", { name: "Save" })).toBeEnabled())
  const start = screen.getByRole("button", { name: "Save" })
  start.focus()
  fireEvent.click(start)
  const teamSwitch = await screen.findByRole("switch", {
    name: /Sync Studio: .* to Example Team/,
  })
  expect(teamSwitch).toBeChecked()
  await waitFor(() => expect(teamSwitch).toHaveFocus())
  expect(screen.getByRole("button", { name: "Save" })).toBe(start)
  expect(start).toBeDisabled()

  fireEvent.click(teamSwitch)
  expect(ongoing.setProject).toHaveBeenCalledExactlyOnceWith({
    accountId: "synthetic-account",
    destinationId: "synthetic-team",
    groupId: group.id,
    selected: false,
    projects: [],
    folderPaths: group.folderPaths,
    expectedPolicyRevision: "synthetic-policy",
  })
  const picker = await folderPicker()
  expect(picker).toHaveValue("")
  await waitFor(() => expect(picker).toHaveFocus())
  fireEvent.change(picker, { target: { value: "synthetic-personal" } })
  expect(
    screen.getByText("Personal workspace: My workspace", { selector: "span" }),
  ).toBeVisible()
  expect(ongoing.enable).toHaveBeenCalledTimes(1)
  const save = screen.getByRole("button", { name: "Save" })
  save.focus()
  fireEvent.click(save)
  await waitFor(() =>
    expect(ongoing.enable).toHaveBeenLastCalledWith(
      expect.objectContaining({
        destinationId: "synthetic-personal",
        projects: group.projects,
      }),
    ),
  )
  expect(begin).not.toHaveBeenCalled()
  const personalSwitch = await screen.findByRole("switch", {
    name: /Sync Studio:.* to Personal/,
  })
  await waitFor(() => expect(personalSwitch).toHaveFocus())
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
  const start = screen.getByRole("button", { name: "Save" })
  start.focus()
  fireEvent.click(start)
  const other = screen.getByRole("button", { name: "Other setting" })
  other.focus()
  other.blur()
  await act(async () => finishSave(team))
  await screen.findByRole("switch", { name: /Sync Studio:.* to Example Team/ })
  await act(() => new Promise<void>((resolve) => requestAnimationFrame(() => resolve())))
  expect(document.body).toHaveFocus()
})

it("revokes immediately and stages a new approval until Save", async () => {
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
  const toggle = await screen.findByRole("switch", { name: /Sync Studio/ })
  const save = screen.getByRole("button", { name: "Save" })
  expect(save).toBeDisabled()
  expect(toggle).toBeChecked()
  expect(
    screen.queryByRole("button", { name: "Update sync selection" }),
  ).not.toBeInTheDocument()
  fireEvent.click(toggle)
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
  await waitFor(() => expect(toggle).not.toBeChecked())
  fireEvent.click(toggle)
  expect(save).toBeEnabled()
  expect(ongoing.setProject).toHaveBeenCalledOnce()
  fireEvent.click(toggle)
  expect(save).toBeDisabled()
  fireEvent.click(toggle)
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
  await waitFor(() => expect(toggle).toBeChecked())
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
  const start = screen.getByRole("button", { name: "Save" })
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

it("keeps missing saved folders checked and available for revocation", async () => {
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
  const toggle = await screen.findByRole("switch", { name: /Sync Studio/ })
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
  act(() => emitSync({ ...team, status: "reauth_required", storageKnown: false }))
  expect(screen.getByText("Example Team: Uploads are unavailable.")).toBeVisible()
  expect(screen.queryByRole("button", { name: /Confirm folders/ })).not.toBeInTheDocument()
  expect(toggle).toBeChecked()
  expect(toggle).toBeEnabled()
  expect(screen.queryByRole("combobox", { name: /Sync Studio/ })).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Save" })).toBeDisabled()

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
  expect(await screen.findByText("Could not load folders.")).toBeVisible()
  const confirm = screen.getByRole("button", { name: "Save" })
  expect(confirm).toBeEnabled()
  expect(screen.getByText("Save the listed Team folders.")).toBeVisible()
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
  fireEvent.change(picker, { target: { value: "synthetic-team" } })
  fireEvent.click(screen.getByRole("button", { name: "Save" }))
  await waitFor(() =>
    expect(ongoing.enable).toHaveBeenCalledExactlyOnceWith(
      expect.objectContaining({ destinationId: "synthetic-team", projects: group.projects }),
    ),
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
  expect(screen.getByRole("button", { name: "Save" })).toBeDisabled()
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
  expect(await screen.findByText("Save to check changed sources.")).toBeVisible()
  const save = screen.getByRole("button", { name: "Save" })
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
  const save = screen.getByRole("button", { name: "Save" })
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
      expect(await screen.findByText("Could not load folders.")).toBeVisible()
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
  const remote = screen.getByRole("switch", { name: /^Sync Remote Studio:/ })
  expect(remote).not.toBeChecked()
  expect(remote).toBeDisabled()
})

it("starts desktop ongoing uploads from Settings without using legacy request discovery", async () => {
  const { port, ongoing, emitSync } = cloudProjectFixture()
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  await screen.findByRole("switch", { name: "Sync Studio: /Example/Studio" })
  const start = screen.getByRole("button", { name: "Save" })
  expect(start).toBeEnabled()
  expect(screen.queryByRole("checkbox")).not.toBeInTheDocument()
  expect(ongoing.enable).not.toHaveBeenCalled()
  fireEvent.click(start)
  await waitFor(() => expect(ongoing.enable).toHaveBeenCalledOnce())
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
    const page = {
      groups: [projectGroup({ projects })],
      sourceRoots: [{ sourceKind, path: sourcePath }],
      nextCursor: null,
      partial: false,
    }
    ongoing.groups.mockResolvedValueOnce({ ...page, sourceRoots: [], partial: true })
    ongoing.groups.mockResolvedValue(page)
    port.pick = vi.fn(async () => [sourcePath])
    const session = new CloudSession(port, false, true)
    render(<Account session={session} />)
    expect(await screen.findByRole("switch", { name: /Sync Studio/ })).toBeChecked()
    const beforeAccess = session.getSnapshot()
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled()
    expect(screen.queryByText("No session folders found.")).not.toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "Refresh folders" })).not.toBeInTheDocument()
    expect(
      screen.queryByRole("button", { name: /Choose Claude|Choose OpenCode/ }),
    ).not.toBeInTheDocument()
    expect(port.pick).not.toHaveBeenCalled()
    expect(ongoing.groups).toHaveBeenCalledOnce()
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
  await screen.findByRole("switch", { name: /Sync Website/ })
  expect(screen.getByRole("button", { name: "Save" })).toBeEnabled()
  expect(
    screen.queryByRole("button", { name: /Choose Claude|Choose OpenCode/ }),
  ).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Load more folders" })).toBeEnabled()
  expect(screen.queryByRole("button", { name: "Refresh folders" })).not.toBeInTheDocument()
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it("retries only the saved desktop policy when a new external worktree awaits confirmation", async () => {
  const { port, ongoing, emitSync } = cloudProjectFixture()
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  await screen.findByRole("switch")
  fireEvent.click(screen.getByRole("button", { name: "Save" }))
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

it("requests a backfill for the saved revision without retrying or changing upload settings", async () => {
  const { port, ongoing, emitSync } = cloudProjectFixture()
  const saved = ongoingStatus({
    status: "ready",
    consentVersion: "desktop-project-sync-v1",
    deviceUploadEnabled: true,
    projects: projectGroup().projects,
    projectChoices: { studio: true },
  })
  ongoing.status.mockResolvedValue(saved)
  vi.mocked(ongoing.backfill!).mockResolvedValue({ ...saved, backfillInProgress: true })
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  const button = await screen.findByRole("button", { name: "Backfill sessions" })
  fireEvent.click(button)

  await waitFor(() => expect(button).toHaveAttribute("aria-busy", "true"))
  await waitFor(() => expect(session.getSnapshot().busy).toBe(false))
  expect(button).toHaveAttribute("aria-disabled", "true")
  expect(screen.getByText("Backfill in progress.")).toHaveAttribute("role", "status")
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
  await act(() => session.backfillSync(oldView))
  expect(ongoing.backfill).toHaveBeenCalledOnce()
  act(() => emitSync({ ...saved, status: "paused", deviceUploadEnabled: false }))
  expect(screen.queryByRole("button", { name: "Backfill sessions" })).toBeNull()
})

it("shows visible workspace names next to identical Backfill sessions labels", async () => {
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
  })
  ongoing.status.mockResolvedValue({
    ...saved,
    teamStatuses: [
      { ...saved, destinationId: "synthetic-team", workspaceName: "Example Team" },
    ],
  })
  render(<Account session={new CloudSession(port, false, true)} />)
  expect(
    await screen.findByRole("button", { name: "Backfill sessions to Personal" }),
  ).toHaveTextContent(/^Backfill sessions$/)
  expect(
    await screen.findByRole("button", { name: "Backfill sessions to Example Team" }),
  ).toHaveTextContent(/^Backfill sessions$/)
  expect(screen.getByText("Personal", { selector: "p" })).toBeVisible()
  expect(screen.getByText("Example Team", { selector: "p" })).toBeVisible()
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
  expect(screen.queryByText(/7 uploaded|Transferred/)).not.toBeInTheDocument()
  act(() => emitSync({ ...healthy, status: "storage_unavailable", storageKnown: false }))
  expect(screen.getByText("Could not read saved uploads.")).toBeVisible()
  expect(screen.queryByText(/7 uploaded|0 uploaded|Transferred/)).not.toBeInTheDocument()
  expect(screen.queryByText("Background sync enabled")).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Check saved uploads" })).toBeEnabled()
  expect(
    screen.queryByRole("button", { name: "Update sync selection" }),
  ).not.toBeInTheDocument()
  expect(screen.queryByRole("button", { name: "Refresh folders" })).not.toBeInTheDocument()
  expect(screen.getByRole("switch", { name: /Sync Studio/ })).toBeVisible()
  ongoing.status.mockResolvedValue(healthy)
  fireEvent.click(screen.getByRole("button", { name: "Check saved uploads" }))
  await waitFor(() =>
    expect(screen.queryByText("Could not read saved uploads.")).not.toBeInTheDocument(),
  )
  expect(session.getSnapshot().ongoing?.acceptedCount).toBe(7)
  expect(ongoing.retry).not.toHaveBeenCalled()
})

it("reads folders on verified-account mount without a manual refresh action", async () => {
  const { port, ongoing, emit } = cloudProjectFixture()
  emit({ identity: null, status: "signed_out" })
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  await screen.findByRole("button", { name: "Sign in" })
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
    fail(new Error("Synthetic read failure"))
    await pending
  })
  expect(screen.getByText("Could not update folders.")).toBeVisible()
  expect(studio).not.toBeChecked()
  expect(
    screen.queryByRole("button", { name: "Choose Claude session folders…" }),
  ).not.toBeInTheDocument()
  ongoing.groups.mockResolvedValueOnce({ ...page, groups: [...page.groups, tools] })
  fireEvent.click(screen.getByRole("button", { name: "Retry folder list" }))
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
  ongoing.groups.mockRejectedValueOnce(new Error("Synthetic read failure"))
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  expect(await screen.findByText("Could not load folders.")).toBeVisible()
  expect(screen.queryByText("No session folders found.")).not.toBeInTheDocument()
  expect(
    screen.queryByRole("button", { name: "Choose Claude session folders…" }),
  ).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Retry folder list" })).toBeEnabled()
  expect(screen.queryByText(/Some sessions need local recovery/)).not.toBeInTheDocument()
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
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  const retry = await screen.findByRole("button", { name: "Retry folder list" })
  expect(screen.getByRole("alert")).toHaveTextContent("Could not finish loading folders.")
  expect(screen.queryByText("No session folders found.")).not.toBeInTheDocument()
  expect(screen.queryByRole("button", { name: "Load more folders" })).not.toBeInTheDocument()
  expect(
    screen.queryByRole("button", { name: /Choose Claude|Choose OpenCode/ }),
  ).not.toBeInTheDocument()
  expect(ongoing.groups).toHaveBeenCalledOnce()
  let finish!: (page: Awaited<ReturnType<typeof ongoing.groups>>) => void
  ongoing.groups.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve
      }),
  )
  fireEvent.click(retry)
  await waitFor(() => expect(ongoing.groups).toHaveBeenCalledTimes(2))
  expect(ongoing.groups).toHaveBeenLastCalledWith(expect.anything(), null)
  expect(retry).toBeDisabled()
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
  expect(screen.getByRole("button", { name: "Save" })).toBeEnabled()
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
  { status: "offline" as const },
  { status: "upload_unavailable" as const },
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
  if (patch.status === "interrupted") {
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

it("offers a real account read retry instead of a coming-soon state after a failed read", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const read = port.read
  port.read = vi
    .fn<typeof port.read>()
    .mockRejectedValueOnce(new Error("Synthetic account read failure"))
    .mockImplementation(read)
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  const retry = await screen.findByRole("button", { name: "Try again" })
  expect(screen.getByText("Could not load Cloud. Try again or continue locally.")).toBeVisible()
  expect(screen.queryByText("Antiburn Cloud is coming soon")).not.toBeInTheDocument()
  expect(screen.queryByRole("button", { name: "Learn more" })).not.toBeInTheDocument()
  fireEvent.click(retry)
  expect(await screen.findByRole("switch", { name: /Sync Studio/ })).toBeVisible()
  expect(ongoing.groups).toHaveBeenCalledOnce()
  expect(ongoing.enable).not.toHaveBeenCalled()
})
