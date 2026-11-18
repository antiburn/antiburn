import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { useSyncExternalStore } from "react"
import { expect, it, vi } from "vitest"
import { CloudSession } from "../../onboarding/CloudSession"
import {
  cloudProjectFixture,
  ongoingStatus,
  projectGroup,
} from "../../onboarding/__tests__/cloudProjectFixtures"
import { CloudAccount } from "./CloudAccount"
import { SettingsTargetFocus } from "./SettingsTargetFocus"

function Account({ session }: { session: CloudSession }) {
  const value = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
    session.getSnapshot,
  )
  return <CloudAccount session={session} value={value} />
}

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
  "restores source access for a cached $sourceKind group without granting consent",
  async ({ sourceKind, sourcePath, picker }) => {
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
    fireEvent.click(screen.getByRole("button", { name: picker }))
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Start syncing" })).toBeEnabled(),
    )
    expect(port.pick).toHaveBeenCalledExactlyOnceWith(sourceKind)
    expect(ongoing.groups).toHaveBeenCalledTimes(2)
    expect(ongoing.enable).not.toHaveBeenCalled()
    await act(() => session.startSync(beforeAccess))
    expect(ongoing.enable).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole("button", { name: "Start syncing" }))
    await waitFor(() => expect(ongoing.enable).toHaveBeenCalledOnce())
    expect(ongoing.enable).toHaveBeenCalledWith(
      expect.objectContaining({
        selectedSources: [{ sourceKind, path: sourcePath }],
        projects,
        projectChoices: { studio: true },
      }),
    )
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
  expect(screen.getByRole("button", { name: "Choose Claude session folders…" })).toBeEnabled()
  expect(screen.getByRole("button", { name: "Choose OpenCode databases…" })).toBeEnabled()
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
  await screen.findByRole("button", { name: "Update sync selection" })
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
  expect(screen.getByText("Some uploads are blocked.")).toBeVisible()
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
  expect(screen.getByRole("button", { name: "Update sync selection" })).toBeEnabled()
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
  expect(screen.getByRole("button", { name: "Update sync selection" })).toBeDisabled()
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
  expect(await screen.findByRole("switch", { name: /Sync Tools/ })).toBeChecked()
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
  ongoing.groups.mockRejectedValueOnce(new Error("Synthetic read failure"))
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  expect(await screen.findByText("Could not load folders.")).toBeVisible()
  expect(screen.queryByText("No session folders found.")).not.toBeInTheDocument()
  expect(
    screen.queryByRole("button", { name: "Choose Claude session folders…" }),
  ).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Retry folder list" })).toBeEnabled()
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it.each([
  { partial: true, nextCursor: null },
  { partial: false, nextCursor: { environment: "native", cwd: "/Example/Next" } },
])("does not claim an incomplete empty discovery is complete: %j", async (page) => {
  const { port, ongoing } = cloudProjectFixture()
  ongoing.groups.mockResolvedValue({ groups: [], sourceRoots: [], ...page })
  const session = new CloudSession(port, false, true)
  render(<Account session={session} />)
  await waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
  expect(screen.queryByText("No session folders found.")).not.toBeInTheDocument()
  expect(
    screen.queryByRole("button", { name: "Choose Claude session folders…" }),
  ).not.toBeInTheDocument()
  expect(screen.queryByText(/Could not|unavailable|blocked/)).not.toBeInTheDocument()
  expect(ongoing.enable).not.toHaveBeenCalled()
})

it.each([
  { status: "syncing" as const, fencedCount: 1, message: "Some uploads are blocked." },
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
