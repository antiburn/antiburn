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

it("saves a folder off immediately and confirms turning it back on", async () => {
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
  expect(screen.getByRole("dialog")).toHaveAccessibleName("Upload sessions from Studio?")
  expect(ongoing.setProject).toHaveBeenCalledOnce()
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
  expect(ongoing.setProject).toHaveBeenCalledOnce()
  fireEvent.click(toggle)
  fireEvent.click(screen.getByRole("button", { name: "Allow uploads" }))
  await waitFor(() => expect(ongoing.setProject).toHaveBeenCalledTimes(2))
  expect(ongoing.setProject).toHaveBeenLastCalledWith(
    expect.objectContaining({
      selected: true,
      projects: projectGroup().projects,
    }),
  )
  expect(ongoing.enable).not.toHaveBeenCalled()
  expect(await screen.findByText("Uploads on")).toBeVisible()
})

it("does not apply a dialog after its saved policy changes", async () => {
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
  fireEvent.click(await screen.findByRole("switch", { name: /Sync Studio/ }))
  act(() => emitSync({ ...saved, policyRevision: "new-policy" }))
  fireEvent.click(screen.getByRole("button", { name: "Allow uploads" }))
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
    expect(
      await screen.findByText("Could not confirm the folder change. Try the switch again."),
    ).toBeVisible()
    expect(toggle).toBeChecked()
    expect(toggle).toBeEnabled()
  },
)

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

it.each([
  ["/Example", true],
  ["/Example/Studio/src", true],
  ["/Example/Stu", false],
  ["/Example/StudioSibling", false],
])("shows directory-boundary permission for %s", async (projectPath, selected) => {
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
})

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
  const start = screen.getByRole("button", { name: "Start syncing" })
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
    expect(screen.getByRole("button", { name: "Start syncing" })).toBeDisabled()
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
  expect(screen.getByRole("button", { name: "Start syncing" })).toBeEnabled()
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
  fireEvent.click(screen.getByRole("button", { name: "Start syncing" }))
  await waitFor(() => expect(ongoing.enable).toHaveBeenCalledOnce())
  act(() =>
    emitSync(
      ongoingStatus({
        status: "fenced",
        consentVersion: "desktop-project-sync-v1",
        projectChoices: { studio: true },
        projects: projectGroup().projects,
      }),
    ),
  )
  expect(screen.getByRole("alert")).toHaveTextContent("Could not resume saved uploads.")
  ongoing.status.mockResolvedValue(
    ongoingStatus({
      status: "fenced",
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
  expect(screen.getByRole("button", { name: "Retry saved uploads" })).toBeEnabled()
  expect(
    screen.queryByRole("button", { name: "Update sync selection" }),
  ).not.toBeInTheDocument()
  expect(screen.queryByRole("button", { name: "Refresh folders" })).not.toBeInTheDocument()
  expect(screen.getByRole("switch", { name: /Sync Studio/ })).toBeVisible()
  ongoing.retry.mockResolvedValue(healthy)
  fireEvent.click(screen.getByRole("button", { name: "Retry saved uploads" }))
  await waitFor(() =>
    expect(screen.queryByText("Could not read saved uploads.")).not.toBeInTheDocument(),
  )
  expect(session.getSnapshot().ongoing?.acceptedCount).toBe(7)
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
  { status: "syncing" as const, fencedCount: 1, message: "Could not resume saved uploads." },
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
        ...patch,
      }),
    )
    const session = new CloudSession(port, false, true)
    render(<Account session={session} />)
    expect(await screen.findByText(message)).toBeVisible()
    expect(screen.getAllByText(message)).toHaveLength(1)
    expect(screen.getByRole("button", { name: "Retry saved uploads" })).toBeEnabled()
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
  { status: "storage_unavailable" as const, storageKnown: false },
])("keeps upload retry for other saved-work problems: %j", async (patch) => {
  const { port, ongoing } = cloudProjectFixture()
  ongoing.status.mockResolvedValue(
    ongoingStatus({
      consentVersion: "desktop-project-sync-v1",
      projectChoices: { studio: true },
      projects: projectGroup().projects,
      interruptedCount: 1,
      recoveryRequiredCount: 1,
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
