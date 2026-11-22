import { expect, it, vi } from "vitest"
import { emptyCloudSnapshot, desktopCloudPort, type CloudSnapshot } from "../lib/cloud"
import { CloudSession } from "./CloudSession"
import {
  cloudProjectFixture,
  ongoingStatus,
  projectGroup,
  localToolsFixture,
} from "./__tests__/cloudProjectFixtures"

it("does not use an older rendered selection to authorize a legacy provider-source request", async () => {
  const value: CloudSnapshot = {
    ...emptyCloudSnapshot,
    authEnabled: true,
    captureEnabled: true,
    status: "signed_in",
    identity: { id: "synthetic-account", login: "Example" },
    destination: { id: "synthetic-personal", name: "Personal", kind: "personal" },
    draft: {
      screen: "complete",
      folders: [{ path: "/Example/claude/projects/project", selected: true }],
    },
  }
  const port = {
    ...desktopCloudPort,
    ongoing: undefined,
    read: vi.fn(async () => value),
    watch: vi.fn(async () => () => {}),
    localTools: localToolsFixture().port,
    prepareSync: vi.fn(async () => {}),
    start: vi.fn(async () => ({ status: "upload_unavailable" as const })),
  }
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().loaded).toBe(true))
    const rendered = {
      ...session.getSnapshot(),
      projects: {
        groups: [],
        choices: {},
        loaded: true,
        loading: false,
        partial: false,
        hasMore: false,
        needsChoiceReview: false,
        error: null,
        selectionKey: JSON.stringify(["/Example/Studio"]),
      },
    }
    await session.startSync(rendered)
    expect(port.prepareSync).not.toHaveBeenCalled()
    expect(port.start).not.toHaveBeenCalled()
  } finally {
    stop()
  }
})

it("attaches both listeners before reads and keeps initial group defaults in memory", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
    expect(vi.mocked(port.watch).mock.invocationCallOrder[0]).toBeLessThan(
      vi.mocked(port.read).mock.invocationCallOrder[0]!,
    )
    expect(ongoing.watch.mock.invocationCallOrder[0]).toBeLessThan(
      ongoing.status.mock.invocationCallOrder[0]!,
    )
    expect(session.getSnapshot().projects?.choices).toEqual({ studio: true })
    expect(port.save).not.toHaveBeenCalled()
    expect(ongoing.enable).not.toHaveBeenCalled()
  } finally {
    stop()
  }
})

it("loads display home without changing upload authority or an existing rendered Start", async () => {
  const { port, ongoing } = cloudProjectFixture()
  let finish!: (home: string | null) => void
  port.displayHome = vi.fn(
    () =>
      new Promise<string | null>((resolve) => {
        finish = resolve
      }),
  )
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
    const rendered = session.getSnapshot()
    expect(rendered.homeDirectory).toBeNull()
    finish("/Users/avery")
    await vi.waitFor(() => expect(session.getSnapshot().homeDirectory).toBe("/Users/avery"))
    expect(ongoing.enable).not.toHaveBeenCalled()
    await session.startSync(rendered)
    expect(ongoing.enable).toHaveBeenCalledOnce()
  } finally {
    stop()
  }
})

it("keeps raw paths available when the display-home read fails", async () => {
  const { port, ongoing } = cloudProjectFixture()
  port.displayHome = vi.fn(async () => {
    throw new Error("Synthetic display lookup failure")
  })
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
    expect(session.getSnapshot().homeDirectory).toBeNull()
    expect(session.getSnapshot().error).toBeNull()
    await session.startSync()
    expect(ongoing.enable).toHaveBeenCalledOnce()
  } finally {
    stop()
  }
})

it.each(["worktree", "source"])(
  "rejects a stale excluded-group confirmation after a %s change",
  async (change) => {
    const { port, ongoing } = cloudProjectFixture()
    ongoing.status.mockResolvedValue(
      ongoingStatus({
        status: "ready",
        consentVersion: "desktop-project-sync-v1",
        projectChoices: { studio: false },
      }),
    )
    const session = new CloudSession(port, false, true)
    const stop = session.subscribe(vi.fn())
    try {
      await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
      const rendered = session.getSnapshot()
      const group = projectGroup()
      const sourcePath =
        change === "source"
          ? "/Example/claude/projects/new-source"
          : group.projects[0]!.sourcePath
      const folderPaths =
        change === "worktree"
          ? [...group.folderPaths, "/Example/new-worktree"]
          : group.folderPaths
      const updated = projectGroup({
        folderPaths,
        projects: folderPaths.map((projectPath) => ({
          ...group.projects[0]!,
          sourcePath,
          projectPath,
        })),
      })
      ongoing.groups.mockResolvedValue({
        groups: [updated],
        sourceRoots: [{ sourceKind: "claude_jsonl", path: sourcePath }],
        nextCursor: null,
        partial: false,
      })
      await session.loadProjects()
      await session.setSavedProject(rendered, group.id, true)
      expect(ongoing.setProject).not.toHaveBeenCalled()
      await session.setSavedProject(session.getSnapshot(), group.id, true)
      expect(ongoing.setProject).toHaveBeenCalledExactlyOnceWith(
        expect.objectContaining({
          selected: true,
          projects: updated.projects,
          folderPaths,
        }),
      )
    } finally {
      stop()
    }
  },
)

it.each(["account", "sign-out"])(
  "ignores an old folder-save failure after %s changes",
  async (change) => {
    const { port, ongoing, emit } = cloudProjectFixture()
    ongoing.status.mockResolvedValue(
      ongoingStatus({
        status: "ready",
        consentVersion: "desktop-project-sync-v1",
        projects: projectGroup().projects,
      }),
    )
    const session = new CloudSession(port, false, true)
    const stop = session.subscribe(vi.fn())
    try {
      await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
      let reject!: (error: Error) => void
      ongoing.setProject.mockImplementationOnce(
        () =>
          new Promise((_, fail) => {
            reject = fail
          }),
      )
      const pending = session.setSavedProject(session.getSnapshot(), "studio", false)
      if (change === "account") {
        ongoing.status.mockResolvedValue(ongoingStatus({ accountId: "second-account" }))
        emit({ identity: { id: "second-account", login: "Second" } })
      } else {
        emit({ identity: null, destination: null, status: "signed_out" })
      }
      reject(new Error("Synthetic old account failure"))
      await pending
      expect(session.getSnapshot().error).toBeNull()
    } finally {
      stop()
    }
  },
)

it("keeps exact saved choices ahead of defaults and local choices ahead of refreshes", async () => {
  const { port, ongoing, emit } = cloudProjectFixture()
  ongoing.status.mockResolvedValue(ongoingStatus({ projectChoices: { studio: false } }))
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
    expect(session.getSnapshot().projects?.choices.studio).toBe(false)
    session.selectProject("studio", true)
    await session.loadProjects()
    expect(session.getSnapshot().projects?.choices.studio).toBe(true)
    expect(port.save).not.toHaveBeenCalled()
    ongoing.status.mockResolvedValueOnce(
      ongoingStatus({
        accountId: "second-account",
        projectChoices: { studio: false },
      }),
    )
    emit({
      identity: { id: "second-account", login: "Second" },
      draft: {
        screen: "folders",
        folders: [],
      },
    })
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
    expect(session.getSnapshot().projects?.choices.studio).toBe(false)
  } finally {
    stop()
  }
})

it("starts desktop ongoing sync with exact current members and matching folders only", async () => {
  const { port, ongoing } = cloudProjectFixture()
  ongoing.groups.mockResolvedValue({
    groups: [
      projectGroup({ projects: [...projectGroup().projects, ...projectGroup().projects] }),
      projectGroup({
        id: "suggestion",
        label: "Other agent",
        path: "/Example/Other",
        folderPaths: ["/Example/Other"],
        captureSupported: false,
        projects: [],
      }),
    ],
    sourceRoots: [{ sourceKind: "claude_jsonl", path: "/Example/claude/projects/studio" }],
    nextCursor: null,
    partial: false,
  })
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
    expect(ongoing.enable).not.toHaveBeenCalled()
    await session.startSync()
    expect(ongoing.enable).toHaveBeenCalledExactlyOnceWith({
      version: "desktop-project-sync-v1",
      accountId: "synthetic-account",
      destinationId: "synthetic-personal",
      selectedSources: [
        { sourceKind: "claude_jsonl", path: "/Example/claude/projects/studio" },
      ],
      projects: projectGroup().projects,
      projectChoices: { studio: true, suggestion: true },
      fullContent: true,
      ongoing: true,
      attachments: false,
    })
    expect(port.prepareSync).not.toHaveBeenCalled()
    expect(port.start).not.toHaveBeenCalled()
    expect(session.getSnapshot()).toMatchObject({
      screen: "complete",
      busy: false,
      ongoing: { status: "ready", acceptedCount: 0 },
    })
  } finally {
    stop()
  }
})

it("rejects an older rendered Start after a later page adds a group member", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const cursor = { environment: "native", cwd: "/Example/next" }
  ongoing.groups.mockResolvedValueOnce({
    groups: [projectGroup()],
    sourceRoots: [{ sourceKind: "claude_jsonl", path: "/Example/claude/projects/studio" }],
    nextCursor: cursor,
    partial: false,
  })
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
    expect(ongoing.groups).toHaveBeenCalledTimes(1)
    expect(session.getSnapshot().projects?.hasMore).toBe(true)
    const rendered = session.getSnapshot()
    ongoing.groups.mockResolvedValueOnce({
      groups: [
        projectGroup({
          folderPaths: [...projectGroup().folderPaths, "/Example/external/new-worktree"],
          worktreeCount: 2,
          projects: [
            ...projectGroup().projects,
            {
              sourceKind: "claude_jsonl",
              sourcePath: "/Example/claude/projects/studio",
              projectPath: "/Example/external/new-worktree",
            },
          ],
        }),
      ],
      sourceRoots: [{ sourceKind: "claude_jsonl", path: "/Example/claude/projects/studio" }],
      nextCursor: null,
      partial: true,
    })
    await session.loadMoreProjects()
    expect(ongoing.groups.mock.calls[1]).toEqual([
      { accountId: "synthetic-account", destinationId: "synthetic-personal" },
      cursor,
    ])
    expect(session.getSnapshot().projects).toMatchObject({ hasMore: false, partial: true })
    await session.startSync(rendered)
    expect(ongoing.enable).not.toHaveBeenCalled()
  } finally {
    stop()
  }
})

it("does not admit a project whose desktop session folder was not returned", async () => {
  const { port, ongoing } = cloudProjectFixture()
  ongoing.groups.mockResolvedValueOnce({
    groups: [projectGroup()],
    sourceRoots: [],
    nextCursor: null,
    partial: false,
  })
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
    await session.startSync()
    expect(ongoing.enable).not.toHaveBeenCalled()
  } finally {
    stop()
  }
})

it("keeps a newer desktop status instead of a stale pause reply", async () => {
  const { port, ongoing, emitSync } = cloudProjectFixture()
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
    let finish!: (value: ReturnType<typeof ongoingStatus>) => void
    ongoing.setEnabled.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    const pending = session.controlSync(true)
    emitSync(
      ongoingStatus({
        status: "offline",
        paused: true,
        consentVersion: "desktop-project-sync-v1",
      }),
    )
    finish(ongoingStatus({ status: "paused", paused: true }))
    await pending
    expect(session.getSnapshot().ongoing?.status).toBe("offline")
    expect(ongoing.setEnabled).toHaveBeenCalledWith(false)
    expect(ongoing.retry).not.toHaveBeenCalled()
  } finally {
    stop()
  }
})

it("requires a new rendered Start when the provider changes at the same project paths", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
    const rendered = session.getSnapshot()
    const selected = projectGroup().folderPaths.map((projectPath) => ({
      sourceKind: "opencode_sqlite_v2" as const,
      sourcePath: "/Example/OpenCode/opencode.db",
      projectPath,
    }))
    ongoing.groups.mockResolvedValueOnce({
      groups: [projectGroup({ agents: ["opencode"], projects: selected })],
      sourceRoots: [
        { sourceKind: "opencode_sqlite_v2", path: "/Example/OpenCode/opencode.db" },
      ],
      nextCursor: null,
      partial: false,
    })
    await session.loadProjects()
    await session.startSync(rendered)
    expect(ongoing.enable).not.toHaveBeenCalled()
    await session.startSync()
    expect(ongoing.enable).toHaveBeenCalledWith(
      expect.objectContaining({
        version: "desktop-project-sync-v1",
        selectedSources: [
          { sourceKind: "opencode_sqlite_v2", path: "/Example/OpenCode/opencode.db" },
        ],
        projects: selected,
      }),
    )
  } finally {
    stop()
  }
})

it("does not let recurring desktop snapshots restart the verified identity delay", async () => {
  vi.useFakeTimers()
  vi.stubGlobal("matchMedia", () => ({ matches: false }))
  const { port, emit } = cloudProjectFixture()
  emit({ draft: { screen: "identity", folders: [] } })
  const session = new CloudSession(port, false)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.advanceTimersByTimeAsync(2000)
    emit({})
    await vi.advanceTimersByTimeAsync(1100)
    expect(session.getSnapshot().screen).toBe("folders")
  } finally {
    stop()
    vi.useRealTimers()
    vi.unstubAllGlobals()
  }
})

it("does not resume legacy saved requests or let them replace ongoing preparation", async () => {
  const { port, ongoing, emit } = cloudProjectFixture()
  const legacy = {
    requestId: "legacy-request",
    result: { status: "upload_unavailable" as const },
  }
  emit({ pendingSync: legacy })
  port.reconcile = vi.fn(async () => ({ status: "upload_unavailable" as const }))
  let finish!: (value: ReturnType<typeof ongoingStatus>) => void
  ongoing.enable.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = resolve
      }),
  )
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
    expect(session.getSnapshot().screen).toBe("folders")
    expect(port.reconcile).not.toHaveBeenCalled()
    const pending = session.startSync()
    emit({ pendingSync: legacy })
    expect(session.getSnapshot()).toMatchObject({
      screen: "complete",
      sync: { status: "starting" },
    })
    finish(ongoingStatus({ status: "ready", consentVersion: "desktop-project-sync-v1" }))
    await pending
  } finally {
    stop()
  }
})

it("rejects an older rendered Start even when a folder is deselected and selected again", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
    const rendered = session.getSnapshot()
    session.selectProject("studio", false)
    session.selectProject("studio", true)
    await session.startSync(rendered)
    expect(ongoing.enable).not.toHaveBeenCalled()
    await session.startSync(session.getSnapshot())
    expect(ongoing.enable).toHaveBeenCalledOnce()
  } finally {
    stop()
  }
})

it.each(["account", "source", "selection"] as const)(
  "does not apply a late enable reply after its %s changes",
  async (change) => {
    const { port, ongoing, emit } = cloudProjectFixture()
    let finish!: (status: ReturnType<typeof ongoingStatus>) => void
    ongoing.enable.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    const session = new CloudSession(port, false, true)
    const stop = session.subscribe(vi.fn())
    try {
      await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
      const rendered = session.getSnapshot()
      const pending = session.startSync(rendered)
      if (change === "account") {
        emit({ identity: { id: "second-account", login: "Second" } })
      } else if (change === "source") {
        ongoing.groups.mockResolvedValueOnce({
          groups: [projectGroup({ projects: [] })],
          sourceRoots: [],
          nextCursor: null,
          partial: true,
        })
        await session.loadProjects()
      } else {
        session.selectProject("studio", false)
      }
      finish(ongoingStatus({ status: "ready", consentVersion: "desktop-project-sync-v1" }))
      await pending
      expect(session.getSnapshot().ongoing?.status).not.toBe("ready")
      await session.startSync(rendered)
      expect(ongoing.enable).toHaveBeenCalledOnce()
    } finally {
      stop()
    }
  },
)
