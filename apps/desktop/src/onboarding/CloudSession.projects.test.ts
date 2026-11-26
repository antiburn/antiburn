import { expect, it, vi } from "vitest"
import { CloudSession } from "./CloudSession"
import {
  cloudProjectFixture,
  ongoingStatus,
  projectGroup,
} from "./__tests__/cloudProjectFixtures"

it("requests a full scan only for the current saved folder policy", async () => {
  const { port, ongoing, emitSync } = cloudProjectFixture()
  const saved = ongoingStatus({
    status: "ready",
    consentVersion: "desktop-project-sync-v1",
    deviceUploadEnabled: true,
    projects: projectGroup().projects,
  })
  ongoing.status.mockResolvedValue(saved)
  const current = { ...saved, policyRevision: "new-policy" }
  ongoing.backfill!.mockResolvedValue({ ...current, backfillInProgress: true })
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() =>
      expect(session.getSnapshot().ongoing?.projects).toEqual(saved.projects),
    )
    const displayed = session.getSnapshot()
    emitSync(current)
    expect(await session.syncNow(displayed)).toBe(false)
    expect(ongoing.backfill).not.toHaveBeenCalled()

    expect(await session.syncNow(session.getSnapshot())).toBe(true)
    expect(ongoing.backfill).toHaveBeenCalledExactlyOnceWith(
      { accountId: "synthetic-account", destinationId: "synthetic-personal" },
      "new-policy",
    )
    expect(ongoing.enable).not.toHaveBeenCalled()
    expect(ongoing.retry).not.toHaveBeenCalled()
  } finally {
    stop()
  }
})

it("checks a saved Team even when the Personal scan request fails", async () => {
  const { port, ongoing, emitSync } = cloudProjectFixture()
  const personal = ongoingStatus({
    status: "ready",
    consentVersion: "desktop-project-sync-v1",
    deviceUploadEnabled: true,
    projects: projectGroup().projects,
  })
  const team = { ...personal, destinationId: "synthetic-team", policyRevision: "team-policy" }
  port.workspaces = vi.fn(async () => [
    { id: "synthetic-personal", name: "Personal", kind: "personal" as const },
    { id: "synthetic-team", name: "Team", kind: "team" as const },
  ])
  ongoing.status.mockResolvedValue(personal)
  ongoing.backfill
    .mockRejectedValueOnce("Synthetic scan failure")
    .mockResolvedValueOnce({ ...team, backfillInProgress: true })
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().projects?.workspaces).toHaveLength(2))
    emitSync(team)
    expect(await session.syncNow(session.getSnapshot())).toBe(false)
    expect(ongoing.backfill).toHaveBeenNthCalledWith(
      2,
      { accountId: "synthetic-account", destinationId: "synthetic-team" },
      "team-policy",
    )
    expect(session.getSnapshot().uploadActionError).toBe("Could not start sync.")
  } finally {
    stop()
  }
})

it("does not use an older rendered selection to authorize folder sync", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
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
    expect(ongoing.enable).not.toHaveBeenCalled()
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

it("keeps later folder rows and their staged choices when the first page refreshes", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const later = projectGroup({ id: "later", label: "Later", path: "/Example/Later" })
  const firstPage = {
    groups: [projectGroup()],
    sourceRoots: [
      { sourceKind: "claude_jsonl" as const, path: "/Example/claude/projects/studio" },
    ],
    nextCursor: { environment: "native", cwd: "/Example/Studio" },
    partial: false,
  }
  ongoing.groups
    .mockResolvedValueOnce(firstPage)
    .mockResolvedValueOnce({ ...firstPage, groups: [later], nextCursor: null })
    .mockResolvedValueOnce(firstPage)
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
    await session.loadMoreProjects()
    session.selectProject("later", false)
    await session.loadProjects()

    expect(session.getSnapshot().projects).toMatchObject({
      groups: [{ id: "studio", sessionCount: 12 }, { id: "later" }],
      choices: { studio: true, later: false },
      hasMore: false,
    })
    expect(ongoing.enable).not.toHaveBeenCalled()
  } finally {
    stop()
  }
})

it("keeps disconnected rows visible without counting or approving cached members", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const studio = projectGroup()
  const staleWorktree = "/Example/worktrees/older"
  const olderProject = { ...studio.projects[0]!, projectPath: staleWorktree }
  const oldStudio = projectGroup({
    sessionCount: 15,
    folderPaths: [...studio.folderPaths, staleWorktree],
    projects: [...studio.projects, olderProject],
  })
  const later = projectGroup({
    id: "later",
    label: "Later",
    path: staleWorktree,
    folderPaths: [staleWorktree],
    projects: [olderProject],
  })
  const first = {
    groups: [studio],
    sourceRoots: [
      { sourceKind: "claude_jsonl" as const, path: studio.projects[0]!.sourcePath },
    ],
    nextCursor: { environment: "native", cwd: "/Example/Next" },
    partial: false,
  }
  ongoing.groups
    .mockResolvedValueOnce(first)
    .mockResolvedValueOnce({ ...first, groups: [oldStudio, later], nextCursor: null })
    .mockResolvedValueOnce({
      ...first,
      nextCursor: { ...first.nextCursor, cwd: "/Example/NewBoundary" },
    })
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
    await session.loadMoreProjects()
    expect(session.getSnapshot().projects?.groups[0]?.sessionCount).toBe(27)
    await session.loadProjects()
    expect(session.getSnapshot().projects).toMatchObject({
      hasMore: true,
      groups: [
        { id: "studio", sessionCount: 12, memberPaths: studio.folderPaths },
        { id: "later", sessionCount: null, discoveryStale: true, captureSupported: false },
      ],
    })
    await session.startSync()
    expect(ongoing.enable).toHaveBeenCalledExactlyOnceWith(
      expect.objectContaining({ projects: studio.projects }),
    )
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
      partial: false,
    })
    await session.loadMoreProjects()
    expect(ongoing.groups.mock.calls[1]).toEqual([
      { accountId: "synthetic-account", destinationId: "synthetic-personal" },
      cursor,
    ])
    expect(session.getSnapshot().projects).toMatchObject({ hasMore: false, partial: false })
    await session.startSync(rendered)
    expect(ongoing.enable).not.toHaveBeenCalled()
  } finally {
    stop()
  }
})

it("keeps existing folders and advances past an empty partial page", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const studio = projectGroup()
  const secondCursor = { environment: "native", cwd: "/Example/Second" }
  const thirdCursor = { environment: "native", cwd: "/Example/Third" }
  ongoing.groups
    .mockResolvedValueOnce({
      groups: [studio],
      sourceRoots: [{ sourceKind: "claude_jsonl", path: studio.projects[0]!.sourcePath }],
      nextCursor: secondCursor,
      partial: false,
    })
    .mockResolvedValueOnce({
      groups: [],
      sourceRoots: [],
      nextCursor: thirdCursor,
      partial: true,
    })
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
    await session.loadMoreProjects()
    expect(session.getSnapshot().projects).toMatchObject({
      groups: [expect.objectContaining({ id: "studio", captureSupported: true })],
      hasMore: true,
      partial: true,
    })
    await session.loadMoreProjects()
    expect(ongoing.groups.mock.calls.map(([, cursor]) => cursor)).toEqual([
      null,
      secondCursor,
      thirdCursor,
    ])
    expect(ongoing.enable).not.toHaveBeenCalled()
  } finally {
    stop()
  }
})

it("keeps Load more blocked until the timed-out native page releases the worker", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const studio = projectGroup()
  const cursor = { environment: "native", cwd: "/Example/Next" }
  const page = {
    groups: [studio],
    sourceRoots: [
      { sourceKind: "claude_jsonl" as const, path: studio.projects[0]!.sourcePath },
    ],
    nextCursor: null,
    partial: false,
  }
  let finish!: () => void
  ongoing.groups
    .mockResolvedValueOnce({ ...page, nextCursor: cursor })
    .mockImplementationOnce(
      () => new Promise((resolve) => (finish = () => resolve({ ...page, groups: [] }))),
    )
  vi.useFakeTimers()
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.advanceTimersByTimeAsync(0)
    const paging = session.loadMoreProjects()
    await vi.advanceTimersByTimeAsync(7_000)
    expect(session.getSnapshot().projects).toMatchObject({
      paging: "pending",
      loading: false,
      loaded: true,
      hasMore: true,
      error: null,
    })
    await vi.advanceTimersByTimeAsync(23_000)
    await paging
    expect(session.getSnapshot().projects?.paging).toBe("pending")
    await session.loadMoreProjects()
    expect(ongoing.groups).toHaveBeenCalledTimes(2)

    finish()
    await vi.advanceTimersByTimeAsync(0)
    expect(session.getSnapshot().projects?.paging).toBeUndefined()
    expect(session.getSnapshot().projects?.hasMore).toBe(true)
    await session.loadMoreProjects()
    expect(ongoing.groups).toHaveBeenLastCalledWith(expect.anything(), cursor)
    expect(ongoing.groups).toHaveBeenCalledTimes(3)
  } finally {
    stop()
    vi.useRealTimers()
  }
})

it.each(["onboarding", "settings"] as const)(
  "saves folder choices without waiting for a canceled local page in %s",
  async (entry) => {
    const { port, ongoing } = cloudProjectFixture()
    const studio = projectGroup()
    const firstPage = {
      groups: [studio],
      sourceRoots: [
        { sourceKind: "claude_jsonl" as const, path: studio.projects[0]!.sourcePath },
      ],
      nextCursor: { environment: "native", cwd: "/Example/Studio" },
      partial: false,
    }
    let finishPage!: (page: Awaited<ReturnType<typeof ongoing.groups>>) => void
    ongoing.groups.mockResolvedValueOnce(firstPage).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishPage = resolve
        }),
    )
    const session = new CloudSession(port, false, true)
    const stop = session.subscribe(vi.fn())
    try {
      await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
      if (entry === "settings") session.selectProject("studio", true, "synthetic-personal")
      const paging = session.loadMoreProjects()
      expect(session.getSnapshot().projects?.paging).toBe("loading")

      const saving =
        entry === "onboarding"
          ? session.startSync()
          : session.startPendingProjects(session.getSnapshot())
      expect(session.getSnapshot().projects?.paging).toBe("pending")
      await saving
      expect(ongoing.enable).toHaveBeenCalledExactlyOnceWith(
        expect.objectContaining({ projects: studio.projects }),
      )

      finishPage({
        ...firstPage,
        groups: [projectGroup({ id: "later" })],
        nextCursor: null,
      })
      await paging
      expect(session.getSnapshot().projects?.paging).toBeUndefined()
      expect(session.getSnapshot().projects?.groups.map(({ id }) => id)).toEqual(["studio"])
    } finally {
      stop()
    }
  },
)

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

it.each([
  ["receipt", "sourceConfirmationRequired"],
  ["readback", "sourceConfirmationRequired"],
  ["readback", "folderConfirmationRequired"],
] as const)("keeps onboarding open when the %s has %s", async (evidence, flag) => {
  const { port, ongoing } = cloudProjectFixture()
  const review = ongoingStatus({
    status: "paused",
    paused: true,
    consentVersion: "desktop-project-sync-v1",
    policyRevision: "saved-policy",
    projects: projectGroup().projects,
    projectChoices: { studio: true },
    [flag]: true,
  })
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
    session.selectProject("studio", true, "synthetic-personal")
    ongoing.status.mockResolvedValue(review)
    if (evidence === "receipt") ongoing.enable.mockResolvedValueOnce(review)
    else ongoing.enable.mockRejectedValueOnce(new Error("Synthetic lost reply"))

    await session.startSync()
    expect(session.getSnapshot()).toMatchObject({
      screen: "folders",
      error:
        "Could not confirm folder sync for Personal. Review saved folders in Settings before trying again.",
      projects: { targets: { studio: "synthetic-personal" } },
    })
    expect(ongoing.status).toHaveBeenCalledTimes(2)
    expect(ongoing.enable).toHaveBeenCalledOnce()
    expect(ongoing.setProject).not.toHaveBeenCalled()
    expect(ongoing.confirmTeamFolders).not.toHaveBeenCalled()
  } finally {
    stop()
  }
})

it("uses readback after a superseded onboarding receipt without clearing the draft", async () => {
  const { port, ongoing, emitSync } = cloudProjectFixture()
  ongoing.enable.mockImplementationOnce(async (consent) => {
    const receipt = ongoingStatus({
      status: "ready",
      policyRevision: "saved-policy",
      consentVersion: consent.version,
      projects: consent.projects,
      projectChoices: consent.projectChoices,
    })
    const latest = { ...receipt, policyRevision: "newer-policy" }
    ongoing.status.mockResolvedValue(latest)
    emitSync(latest)
    return receipt
  })
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
    session.selectProject("studio", true, "synthetic-personal")
    await session.startSync()
    expect(session.getSnapshot()).toMatchObject({
      screen: "complete",
      error: null,
      ongoing: { policyRevision: "newer-policy" },
      projects: { targets: { studio: "synthetic-personal" } },
    })
    expect(ongoing.status).toHaveBeenCalledTimes(2)
    expect(ongoing.enable).toHaveBeenCalledOnce()
    expect(ongoing.setProject).not.toHaveBeenCalled()
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

it("does not let an unchanged account event replace an active folder confirmation", async () => {
  const { port, ongoing, emit } = cloudProjectFixture()
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
    expect(ongoing.enable).not.toHaveBeenCalled()
    const pending = session.startSync()
    emit({})
    expect(session.getSnapshot()).toMatchObject({
      screen: "folders",
      sync: { status: "starting" },
    })
    finish(
      ongoingStatus({
        status: "ready",
        consentVersion: "desktop-project-sync-v1",
        projects: projectGroup().projects,
      }),
    )
    await pending
    expect(session.getSnapshot().screen).toBe("complete")
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
          partial: false,
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
