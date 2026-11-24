import { expect, it, vi } from "vitest"
import { CloudSession } from "./CloudSession"
import {
  cloudProjectFixture,
  ongoingStatus,
  projectGroup,
} from "./__tests__/cloudProjectFixtures"

const personal = "synthetic-personal"
const team = "synthetic-team"
const group = projectGroup()

const policy = (destinationId: string, policyRevision: string, selected: boolean) =>
  ongoingStatus({
    destinationId,
    policyRevision,
    status: "ready",
    consentVersion: "desktop-project-sync-v1",
    projects: selected ? group.projects : [],
    projectChoices: { [group.id]: selected },
  })

/** Start a session with workspace and folder evidence from synthetic desktop responses. */
async function savedProjectFixture(from = personal, targetExists = true) {
  const fixture = cloudProjectFixture()
  const { port, ongoing } = fixture
  const savedPersonal =
    from === personal || targetExists
      ? policy(personal, "personal-before", from === personal)
      : ongoingStatus()
  const savedTeam =
    from === team || targetExists ? policy(team, "team-before", from === team) : undefined
  ongoing.status.mockResolvedValue({
    ...savedPersonal,
    teamStatuses: savedTeam ? [savedTeam] : [],
  })
  port.workspaces = vi.fn<NonNullable<typeof port.workspaces>>(async () => [
    { id: personal, kind: "personal", name: "Personal" },
    { id: team, kind: "team", name: "Team" },
  ])
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
  return { ...fixture, session, stop }
}

it.each([
  [personal, team],
  [team, personal],
])("removes %s before adding the same folder to %s", async (from, to) => {
  const { session, ongoing, emitSync, stop } = await savedProjectFixture(from)
  try {
    const removed = policy(from, "removed", false)
    ongoing.setProject
      .mockImplementationOnce(async () => {
        // Team removal reports the saved rule without reading transfer counters.
        if (from === team) {
          removed.status = "reauth_required"
          removed.storageKnown = false
        }
        emitSync(removed)
        return removed
      })
      .mockResolvedValueOnce(policy(to, "added", true))

    await expect(
      session.moveSavedProject(session.getSnapshot(), group.id, from, to),
    ).resolves.toBe("clear_draft")

    expect(ongoing.setProject.mock.calls).toEqual([
      [
        {
          accountId: "synthetic-account",
          destinationId: from,
          groupId: group.id,
          selected: false,
          projects: [],
          folderPaths: group.folderPaths,
          expectedPolicyRevision: from === personal ? "personal-before" : "team-before",
          expectedRemovals: group.projects,
        },
      ],
      [
        {
          accountId: "synthetic-account",
          destinationId: to,
          groupId: group.id,
          selected: true,
          projects: group.projects,
          folderPaths: group.folderPaths,
          expectedPolicyRevision: to === personal ? "personal-before" : "team-before",
          moveFrom: { destinationId: from, policyRevision: "removed" },
        },
      ],
    ])
    expect(session.getSnapshot().uploadActionError).toBeNull()
    expect(ongoing.retry).not.toHaveBeenCalled()
    expect(ongoing.enable).not.toHaveBeenCalled()
  } finally {
    stop()
  }
})

it("creates only the new workspace policy after removal", async () => {
  const { session, ongoing, stop } = await savedProjectFixture(personal, false)
  try {
    ongoing.setProject.mockResolvedValueOnce(policy(personal, "removed", false))
    ongoing.enable.mockResolvedValueOnce(policy(team, "created", true))

    await expect(
      session.moveSavedProject(session.getSnapshot(), group.id, personal, team),
    ).resolves.toBe("clear_draft")

    expect(ongoing.setProject).toHaveBeenCalledOnce()
    expect(ongoing.enable).toHaveBeenCalledExactlyOnceWith(
      expect.objectContaining({
        destinationId: team,
        projects: group.projects,
        projectChoices: { [group.id]: true },
        moveFrom: { destinationId: personal, policyRevision: "removed" },
      }),
    )
    expect(ongoing.setProject.mock.invocationCallOrder[0]).toBeLessThan(
      ongoing.enable.mock.invocationCallOrder[0]!,
    )
  } finally {
    stop()
  }
})

it("moves only the repository when an exact rule shares its saved anchor", async () => {
  const { session, ongoing, emitSync, stop } = await savedProjectFixture()
  try {
    const anchor = group.projects[0]!
    const exact = { ...anchor, sourcePath: "/Example/another/source" }
    const checkout = "/Example/new/Studio"
    const project = { ...anchor, projectPath: checkout }
    const id = "repo-studio"
    ongoing.groups.mockResolvedValue({
      groups: [
        projectGroup({
          id,
          path: checkout,
          folderPaths: [checkout],
          projects: [project],
          repositoryWide: true,
        }),
      ],
      sourceRoots: [{ sourceKind: anchor.sourceKind, path: anchor.sourcePath }],
      nextCursor: null,
      partial: false,
    })
    await session.loadProjects()
    emitSync({
      ...policy(personal, "repo-before", true),
      projects: [anchor, exact],
      exactProjects: [exact],
      projectChoices: { [id]: true },
      savedFolders: [{ groupId: id, path: anchor.projectPath }],
      repositoryGroupIds: [id],
      repositoryFolders: [{ groupId: id, projects: [anchor] }],
    })
    ongoing.setProject
      .mockResolvedValueOnce({
        ...policy(personal, "repo-removed", false),
        projectChoices: { [id]: false },
        projects: [exact],
        exactProjects: [exact],
      })
      .mockResolvedValueOnce({
        ...policy(team, "repo-added", true),
        projects: [project],
        projectChoices: { [id]: true },
        savedFolders: [{ groupId: id, path: checkout }],
        repositoryGroupIds: [id],
        repositoryFolders: [{ groupId: id, projects: [project] }],
        exactProjects: [],
      })

    await session.moveSavedProject(session.getSnapshot(), id, personal, team)

    expect(ongoing.setProject).toHaveBeenCalledTimes(2)
    expect(ongoing.setProject.mock.calls[0]?.[0]).toEqual({
      accountId: "synthetic-account",
      destinationId: personal,
      groupId: id,
      selected: false,
      projects: [],
      folderPaths: [checkout],
      expectedPolicyRevision: "repo-before",
      expectedRemovals: [anchor],
    })
    expect(ongoing.setProject).toHaveBeenLastCalledWith(
      expect.objectContaining({
        groupId: id,
        destinationId: team,
        selected: true,
        projects: [project],
        moveFrom: { destinationId: personal, policyRevision: "repo-removed" },
      }),
    )
    expect(session.getSnapshot().uploadActionError).toBeNull()
    expect(session.getSnapshot().ongoing?.exactProjects).toEqual([exact])
  } finally {
    stop()
  }
})

it("only removes the old grant when the user chooses not to sync", async () => {
  const { session, ongoing, emitSync, stop } = await savedProjectFixture()
  try {
    const savedProject = group.projects[0]!
    emitSync({ ...policy(personal, "removed-subset-before", true), projects: [savedProject] })
    await session.moveSavedProject(session.getSnapshot(), group.id, personal, team)
    expect(ongoing.setProject).not.toHaveBeenCalled()

    ongoing.setProject.mockResolvedValueOnce(policy(personal, "removed", false))
    await session.moveSavedProject(session.getSnapshot(), group.id, personal, undefined)
    expect(ongoing.setProject).toHaveBeenCalledExactlyOnceWith(
      expect.objectContaining({
        selected: false,
        expectedPolicyRevision: "removed-subset-before",
        expectedRemovals: [savedProject],
      }),
    )
    expect(ongoing.enable).not.toHaveBeenCalled()
    expect(session.getSnapshot().ongoing?.projects).toEqual([])
  } finally {
    stop()
  }
})

it("checks Team membership before removal when the target already has saved folders", async () => {
  const { session, ongoing, emitSync, stop } = await savedProjectFixture()
  try {
    const project = { ...group.projects[0]!, projectPath: "/Example/Other" }
    const target = { ...policy(team, "team-before", false), projects: [project] }
    emitSync(target)
    ongoing.status.mockResolvedValueOnce({
      ...policy(personal, "personal-before", true),
      teamStatuses: [
        {
          ...target,
          savedFolders: [{ groupId: "saved-other", path: project.projectPath }],
          folderConfirmationRequired: true,
        },
      ],
    })

    await session.moveSavedProject(session.getSnapshot(), group.id, personal, team)

    expect(ongoing.setProject).not.toHaveBeenCalled()
    expect(ongoing.enable).not.toHaveBeenCalled()
    expect(session.getSnapshot().teamStatuses?.[team]?.folderConfirmationRequired).toBe(true)
    expect(session.getSnapshot().uploadActionError).toMatch(/confirm the saved Team folders/i)
  } finally {
    stop()
  }
})

it.each(["rejects", "wrong workspace", "same revision", "remaining grant"] as const)(
  "does not start the new workspace when the old removal %s",
  async (result) => {
    const { session, ongoing, stop } = await savedProjectFixture()
    try {
      if (result === "rejects") ongoing.setProject.mockRejectedValueOnce(new Error("Failure"))
      else {
        const reply = policy(
          result === "wrong workspace" ? team : personal,
          result === "same revision" ? "personal-before" : "removed",
          false,
        )
        if (result === "remaining grant") reply.projects = group.projects
        ongoing.setProject.mockResolvedValueOnce(reply)
      }

      await expect(
        session.moveSavedProject(session.getSnapshot(), group.id, personal, team),
      ).resolves.toBe("clear_draft")
      expect(ongoing.setProject).toHaveBeenCalledOnce()
      expect(ongoing.enable).not.toHaveBeenCalled()
      expect(session.getSnapshot().uploadActionError).toMatch(/could not confirm.*old/i)
      expect(session.getSnapshot().projects?.targets?.[group.id]).toBeUndefined()
    } finally {
      stop()
    }
  },
)

it("keeps the workspace draft when Team verification fails before removal", async () => {
  const { session, ongoing, emitSync, stop } = await savedProjectFixture()
  try {
    const other = { ...group.projects[0]!, projectPath: "/Example/Other" }
    emitSync({ ...policy(team, "team-before", false), projects: [other] })
    ongoing.status.mockRejectedValueOnce(new Error("Status unavailable"))

    await expect(
      session.moveSavedProject(session.getSnapshot(), group.id, personal, team),
    ).resolves.toBe("keep_draft")

    expect(ongoing.setProject).not.toHaveBeenCalled()
    expect(ongoing.enable).not.toHaveBeenCalled()
    expect(session.getSnapshot().uploadActionError).toMatch(
      /could not verify the Team folder rules/i,
    )
  } finally {
    stop()
  }
})

it.each([
  [personal, team],
  [team, personal],
  [personal, undefined],
])("confirms a rejected removal reply for %s without writing to %s", async (from, to) => {
  const { session, ongoing, stop } = await savedProjectFixture(from)
  try {
    const removed = policy(from, "removed", false)
    ongoing.setProject.mockRejectedValueOnce(new Error("Removal reply unavailable"))
    ongoing.status.mockResolvedValueOnce(
      from === personal
        ? { ...removed, teamStatuses: [policy(team, "read-team", false)] }
        : { ...policy(personal, "read-personal", false), teamStatuses: [removed] },
    )

    await expect(
      session.moveSavedProject(session.getSnapshot(), group.id, from, to),
    ).resolves.toBe("clear_draft")

    const { ongoing: personalPolicy, teamStatuses, uploadActionError } = session.getSnapshot()
    expect(personalPolicy?.policyRevision).toBe(
      from === personal ? "removed" : "personal-before",
    )
    expect(teamStatuses?.[team]?.policyRevision).toBe(from === team ? "removed" : "team-before")
    expect(uploadActionError).toBe(
      "Sync stopped for this folder. Choose a workspace to sync it again.",
    )
    expect(ongoing.setProject).toHaveBeenCalledExactlyOnceWith(
      expect.objectContaining({ destinationId: from, selected: false }),
    )
    await session.startPendingProjects(session.getSnapshot())
    expect(ongoing.setProject).toHaveBeenCalledOnce()
    expect(ongoing.enable).not.toHaveBeenCalled()
  } finally {
    stop()
  }
})

it.each(["foreign status", "account switch", "newer source"] as const)(
  "does not apply removal recovery with %s",
  async (change) => {
    const { session, ongoing, emit, emitSync, stop } = await savedProjectFixture()
    try {
      let finish!: (status: ReturnType<typeof ongoingStatus>) => void
      ongoing.setProject.mockRejectedValueOnce(new Error("Removal reply unavailable"))
      ongoing.status.mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            finish = resolve
          }),
      )
      const pending = session.moveSavedProject(session.getSnapshot(), group.id, personal, team)
      await vi.waitFor(() => expect(finish).toBeTypeOf("function"))
      if (change === "newer source") emitSync(policy(personal, "newer-personal", true))
      if (change === "account switch")
        emit({ identity: { id: "other-account", login: "Other" } })
      const removed = policy(personal, "removed", false)
      if (change === "foreign status") removed.accountId = "other-account"
      finish(removed)
      await expect(pending).resolves.toBe("clear_draft")

      const { ongoing: personalPolicy, uploadActionError } = session.getSnapshot()
      if (change === "account switch") {
        expect(personalPolicy).toBeNull()
        expect(uploadActionError).toBeNull()
      } else {
        expect(personalPolicy?.policyRevision).toBe(
          change === "newer source" ? "newer-personal" : "personal-before",
        )
        expect(uploadActionError).toMatch(/could not confirm.*old/i)
      }
      expect(ongoing.setProject).toHaveBeenCalledOnce()
      expect(ongoing.enable).not.toHaveBeenCalled()
    } finally {
      stop()
    }
  },
)

it("rechecks the rules after a target save fails and requires a fresh workspace choice", async () => {
  const { session, ongoing, stop } = await savedProjectFixture()
  try {
    const removed = policy(personal, "removed", false)
    ongoing.setProject
      .mockResolvedValueOnce(removed)
      .mockRejectedValueOnce(new Error("Failure"))
    ongoing.status.mockResolvedValueOnce({
      ...removed,
      teamStatuses: [policy(team, "team-before", false)],
    })

    await session.moveSavedProject(session.getSnapshot(), group.id, personal, team)
    expect(ongoing.setProject).toHaveBeenCalledTimes(2)
    expect(session.getSnapshot().ongoing?.projects).toEqual([])
    expect(session.getSnapshot().projects?.targets?.[group.id]).toBeUndefined()
    expect(session.getSnapshot().uploadActionError).toBe(
      "Sync stopped for this folder. Choose a workspace to sync it again.",
    )

    await session.startPendingProjects(session.getSnapshot())
    expect(ongoing.setProject).toHaveBeenCalledTimes(2)
    expect(ongoing.enable).not.toHaveBeenCalled()
  } finally {
    stop()
  }
})

it("shows the new workspace if a status read confirms a save whose reply failed", async () => {
  const { session, ongoing, stop } = await savedProjectFixture()
  try {
    const removed = policy(personal, "removed", false)
    const saved = {
      ...policy(team, "saved", true),
      status: "reauth_required" as const,
      storageKnown: false,
    }
    ongoing.setProject
      .mockResolvedValueOnce(removed)
      .mockRejectedValueOnce(new Error("Failure"))
    ongoing.status.mockResolvedValueOnce({ ...removed, teamStatuses: [saved] })

    await session.moveSavedProject(session.getSnapshot(), group.id, personal, team)

    expect(session.getSnapshot().teamStatuses?.[team]?.policyRevision).toBe("saved")
    expect(session.getSnapshot().uploadActionError).toBeNull()
    expect(session.getSnapshot().projects?.targets?.[group.id]).toBeUndefined()
    expect(ongoing.setProject).toHaveBeenCalledTimes(2)
  } finally {
    stop()
  }
})

it("does not claim all sync stopped if the recheck cannot read the new rule", async () => {
  const { session, ongoing, stop } = await savedProjectFixture()
  try {
    ongoing.setProject
      .mockResolvedValueOnce(policy(personal, "removed", false))
      .mockRejectedValueOnce(new Error("Failure"))
    ongoing.status.mockRejectedValueOnce(new Error("Failure"))

    await session.moveSavedProject(session.getSnapshot(), group.id, personal, team)

    expect(session.getSnapshot().uploadActionError).toMatch(
      /^Sync stopped in the old workspace/,
    )
    expect(session.getSnapshot().projects?.targets?.[group.id]).toBeUndefined()
  } finally {
    stop()
  }
})

it("does not claim sync stopped if another workspace gained a grant during the move", async () => {
  const { session, ongoing, stop } = await savedProjectFixture()
  try {
    const removed = policy(personal, "removed", false)
    ongoing.setProject
      .mockResolvedValueOnce(removed)
      .mockRejectedValueOnce(new Error("Failure"))
    ongoing.status.mockResolvedValueOnce({
      ...removed,
      teamStatuses: [
        policy(team, "team-before", false),
        policy("other-team", "other-grant", true),
      ],
    })

    await session.moveSavedProject(session.getSnapshot(), group.id, personal, team)

    expect(session.getSnapshot().teamStatuses?.["other-team"]?.policyRevision).toBe(
      "other-grant",
    )
    expect(session.getSnapshot().uploadActionError).toMatch(
      /^Sync stopped in the old workspace/,
    )
  } finally {
    stop()
  }
})

it.each([
  "source",
  "workspace",
  "signed out",
  "account switched",
  "signed out and back",
] as const)(
  "handles %s changes during a Team preflight without removing any rule",
  async (change) => {
    const { session, ongoing, emit, emitSync, stop } = await savedProjectFixture()
    try {
      const other = { ...group.projects[0]!, projectPath: "/Example/Other" }
      const target = { ...policy(team, "team-before", false), projects: [other] }
      emitSync(target)
      let finish!: (value: ReturnType<typeof ongoingStatus>) => void
      ongoing.status.mockImplementationOnce(() => new Promise((resolve) => (finish = resolve)))
      const pending = session.moveSavedProject(session.getSnapshot(), group.id, personal, team)
      if (change === "source") {
        const before = session.getSnapshot().cloud.draft
        emit({
          draft: { ...before, folders: [{ path: "/Example/claude/projects", selected: true }] },
        })
      } else if (change === "workspace") {
        emitSync({ ...target, policyRevision: "team-changed" })
      } else if (change === "account switched") {
        emit({ identity: { id: "second-account", login: "Second" } })
      } else {
        const { identity, destination } = session.getSnapshot().cloud
        emit({ identity: null, destination: null, status: "signed_out" })
        if (change === "signed out and back")
          emit({ identity, destination: destination ?? null, status: "signed_in" })
      }
      finish({ ...policy(personal, "personal-before", true), teamStatuses: [target] })
      await expect(pending).resolves.toBe("clear_draft")

      const expected =
        change === "source" || change === "workspace"
          ? "The saved folder or workspace changed. Review the folder choices and try again."
          : null
      expect(session.getSnapshot().uploadActionError).toBe(expected)
      expect(ongoing.setProject).not.toHaveBeenCalled()
      expect(ongoing.enable).not.toHaveBeenCalled()
    } finally {
      stop()
    }
  },
)

it.each([
  "sign-out",
  "account",
  "target revision",
  "stale Team",
  "group members",
  "newer old rule",
] as const)("does not start the new workspace if %s changes during removal", async (change) => {
  const { session, ongoing, emit, emitSync, stop } = await savedProjectFixture()
  try {
    let finish!: (status: ReturnType<typeof ongoingStatus>) => void
    ongoing.setProject.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    const pending = session.moveSavedProject(session.getSnapshot(), group.id, personal, team)
    if (change === "sign-out") {
      emit({ identity: null, destination: null, status: "signed_out" })
    } else if (change === "account") {
      emit({ identity: { id: "other-account", login: "Other" } })
    } else if (change === "target revision") {
      emitSync(policy(team, "different-team", false))
    } else if (change === "stale Team") {
      emitSync({ ...policy(team, "team-before", false), folderConfirmationRequired: true })
    } else if (change === "newer old rule") {
      emitSync(policy(personal, "newer-personal", true))
    } else {
      const changedPath = "/Example/worktrees/studio-new"
      ongoing.groups.mockResolvedValueOnce({
        groups: [
          projectGroup({
            folderPaths: [...group.folderPaths, changedPath],
            projects: [...group.projects, { ...group.projects[0]!, projectPath: changedPath }],
          }),
        ],
        sourceRoots: [{ sourceKind: "claude_jsonl", path: group.projects[0]!.sourcePath }],
        nextCursor: null,
        partial: false,
      })
      await session.loadProjects()
    }
    finish(policy(personal, "removed", false))
    await pending

    expect(ongoing.setProject).toHaveBeenCalledOnce()
    expect(ongoing.enable).not.toHaveBeenCalled()
    if (change === "sign-out" || change === "account")
      expect(session.getSnapshot().uploadActionError).toBeNull()
    if (change === "group members")
      expect(session.getSnapshot().projects?.targets?.[group.id]).toBeUndefined()
  } finally {
    stop()
  }
})

it("does not publish a late new rule or draft after the account changes", async () => {
  const { session, ongoing, emit, stop } = await savedProjectFixture()
  try {
    let finish!: (status: ReturnType<typeof ongoingStatus>) => void
    ongoing.setProject.mockResolvedValueOnce(policy(personal, "removed", false))
    ongoing.setProject.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    const pending = session.moveSavedProject(session.getSnapshot(), group.id, personal, team)
    await vi.waitFor(() => expect(ongoing.setProject).toHaveBeenCalledTimes(2))
    await session.moveSavedProject(session.getSnapshot(), group.id, personal, team)
    expect(ongoing.setProject).toHaveBeenCalledTimes(2)

    emit({ identity: { id: "other-account", login: "Other" } })
    finish(policy(team, "added", true))
    await pending

    expect(session.getSnapshot().teamStatuses?.[team]).toBeUndefined()
    expect(session.getSnapshot().projects?.targets?.[group.id]).toBeUndefined()
    expect(session.getSnapshot().uploadActionError).toBeNull()
  } finally {
    stop()
  }
})

it.each(["duplicate", "stale Team", "unknown workspace", "stale revision"] as const)(
  "rejects a move before turning anything off for %s",
  async (reason) => {
    const { session, ongoing, emitSync, stop } = await savedProjectFixture()
    try {
      const context = session.getSnapshot()
      if (reason === "duplicate") emitSync(policy(team, "team-before", true))
      if (reason === "stale Team")
        emitSync({ ...policy(team, "team-before", false), folderConfirmationRequired: true })
      if (reason === "stale revision") emitSync(policy(personal, "newer-personal", true))

      await session.moveSavedProject(
        reason === "stale revision" ? context : session.getSnapshot(),
        group.id,
        personal,
        reason === "unknown workspace" ? "unverified-team" : team,
      )
      expect(ongoing.setProject).not.toHaveBeenCalled()
      expect(ongoing.enable).not.toHaveBeenCalled()
      if (reason === "stale Team")
        expect(session.getSnapshot().uploadActionError).toMatch(
          /confirm the saved Team folders/i,
        )
    } finally {
      stop()
    }
  },
)
