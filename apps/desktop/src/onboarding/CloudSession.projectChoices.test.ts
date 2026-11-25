import { expect, it, vi } from "vitest"
import type { CloudOngoingConsent, CloudOngoingPort } from "../lib/cloud"
import { CloudSession } from "./CloudSession"
import {
  cloudProjectFixture,
  ongoingStatus,
  projectGroup,
} from "./__tests__/cloudProjectFixtures"

it("keeps confirmed exclusions across reopen while new groups and unconfirmed edits stay local", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const website = projectGroup({
    id: "website",
    label: "Website",
    path: "/Example/Website",
    folderPaths: ["/Example/Website"],
    worktreeCount: 0,
    agents: ["opencode"],
    sessionCount: 3,
    captureSessionCount: 3,
    projects: [
      {
        sourceKind: "opencode_sqlite_v2",
        sourcePath: "/Example/OpenCode/opencode.db",
        projectPath: "/Example/Website",
      },
    ],
  })
  const added = projectGroup({
    id: "new-project",
    label: "New project",
    path: "/Example/New",
    folderPaths: ["/Example/New"],
    worktreeCount: 0,
    agents: ["opencode"],
    sessionCount: 1,
    captureSessionCount: 1,
    projects: [
      {
        sourceKind: "opencode_sqlite_v2",
        sourcePath: "/Example/OpenCode/opencode.db",
        projectPath: "/Example/New",
      },
    ],
  })
  let saved = ongoingStatus()
  ongoing.status.mockImplementation(async () => saved)
  ongoing.enable.mockImplementation(async (consent: CloudOngoingConsent) => {
    saved = ongoingStatus({
      status: "ready",
      consentVersion: consent.version,
      projects: consent.projects,
      selectedSources: consent.selectedSources,
      projectChoices: consent.projectChoices,
    })
    return saved
  })
  ongoing.setProject.mockImplementation(async (change) => {
    saved = {
      ...saved,
      projects: [...saved.projects, ...change.projects],
      projectChoices: { ...saved.projectChoices, [change.groupId]: change.selected },
    }
    return saved
  })
  const discovery = {
    groups: [projectGroup(), website],
    sourceRoots: [
      { sourceKind: "claude_jsonl" as const, path: "/Example/claude/projects/studio" },
      { sourceKind: "opencode_sqlite_v2" as const, path: "/Example/OpenCode/opencode.db" },
    ],
    nextCursor: null,
    partial: false,
  }
  ongoing.groups.mockResolvedValue(discovery)
  let session = new CloudSession(port, false, true)
  let stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
    session.selectProject("website", false)
    expect(saved.projectChoices).toEqual({})
    expect(port.save).not.toHaveBeenCalled()
    await session.startSync()
    expect(saved.projectChoices).toEqual({ studio: true, website: false })
    expect(saved.projects).toEqual(projectGroup().projects)
    stop()

    ongoing.groups.mockResolvedValue({ ...discovery, groups: [...discovery.groups, added] })
    session = new CloudSession(port, false, true)
    stop = session.subscribe(vi.fn())
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
    expect(session.getSnapshot().projects?.choices).toEqual({
      studio: true,
      website: false,
      "new-project": true,
    })
    expect(saved.projectChoices).not.toHaveProperty("new-project")
    expect(saved.projects.some((project) => project.projectPath === "/Example/New")).toBe(false)
    expect(ongoing.enable).toHaveBeenCalledTimes(1)

    session.selectProject("website", true)
    stop()
    session = new CloudSession(port, false, true)
    stop = session.subscribe(vi.fn())
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
    expect(session.getSnapshot().projects?.choices.website).toBe(false)
    expect(saved.projectChoices).toEqual({ studio: true, website: false })

    await session.startSync()
    expect(ongoing.enable).toHaveBeenCalledOnce()
    expect(ongoing.setProject).not.toHaveBeenCalled()
    session.selectProject("new-project", true, "synthetic-personal")
    await session.startPendingProjects(session.getSnapshot())
    expect(ongoing.enable).toHaveBeenCalledOnce()
    expect(ongoing.setProject).toHaveBeenCalledOnce()
    expect(saved.projectChoices).toEqual({ studio: true, website: false, "new-project": true })
    expect(saved.projects.some((project) => project.projectPath === "/Example/New")).toBe(true)
    expect(saved.projects.some((project) => project.projectPath === "/Example/Website")).toBe(
      false,
    )
  } finally {
    stop()
  }
})

it("keeps an older policy with unknown choices read-only for Start", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const older = ongoingStatus({
    status: "ready",
    consentVersion: "desktop-project-sync-v1",
    projects: projectGroup().projects,
  })
  delete older.projectChoices
  ongoing.status.mockResolvedValue(older)
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
    expect(session.getSnapshot().projects).toMatchObject({
      choices: { studio: false },
      needsChoiceReview: true,
    })
    await session.startSync()
    expect(ongoing.enable).not.toHaveBeenCalled()
    session.selectProject("studio", true, "synthetic-personal")
    await session.startPendingProjects(session.getSnapshot())
    expect(ongoing.enable).not.toHaveBeenCalled()
    expect(ongoing.setProject).not.toHaveBeenCalled()
    expect(session.getSnapshot().ongoing?.projects).toEqual(older.projects)
  } finally {
    stop()
  }
})

const personal = { id: "synthetic-personal", name: "Personal", kind: "personal" } as const
const team = { id: "synthetic-team", name: "Example Team", kind: "team" } as const

function teamFolderFixture() {
  const fixture = cloudProjectFixture()
  const { port, ongoing } = fixture
  const folders = ["Docs", "Website"].map((label) => {
    const id = label.toLowerCase()
    const path = `/Example/${label}`
    return projectGroup({
      id,
      label,
      path,
      folderPaths: [path],
      projects: [{ ...projectGroup().projects[0]!, projectPath: path }],
    })
  })
  const savedTeam = ongoingStatus({
    destinationId: team.id,
    consentVersion: "desktop-project-sync-v1",
    status: "ready",
    policyRevision: "team-policy-1",
    projectChoices: { studio: true, excluded: false },
    projects: projectGroup().projects,
  })
  ongoing.status.mockResolvedValue(ongoingStatus({ teamStatuses: [savedTeam] }))
  ongoing.groups.mockResolvedValue({
    groups: [projectGroup(), ...folders],
    sourceRoots: [{ sourceKind: "claude_jsonl", path: "/Example/claude/projects/studio" }],
    nextCursor: null,
    partial: false,
  })
  port.workspaces = vi.fn(async () => [personal, team])
  return { ...fixture, savedTeam, folders }
}

it.each([
  "team-saved",
  "retry-with-hold",
  "retry-after-removal",
  "hold-between-attempts",
] as const)(
  "checks the saved Personal route when onboarding ends with %s",
  async (scenario) => {
    const { port, ongoing, emit, emitSync } = teamFolderFixture()
    let personalStatus = ongoingStatus()
    let teamStatus: ReturnType<typeof ongoingStatus> | undefined
    let teamAttempt = 0
    const holdPersonal = () => {
      personalStatus = {
        ...personalStatus,
        policyRevision: "paused-personal-policy",
        status: "paused",
        paused: true,
        sourceConfirmationRequired: true,
      }
      emitSync(personalStatus)
    }
    ongoing.status.mockImplementation(async () => ({
      ...personalStatus,
      teamStatuses: teamStatus ? [teamStatus] : [],
    }))
    ongoing.enable.mockImplementation(async (consent) => {
      const saved = ongoingStatus({
        destinationId: consent.destinationId,
        consentVersion: consent.version,
        status: "ready",
        projects: consent.projects,
        projectChoices: consent.projectChoices,
      })
      if (consent.destinationId === personal.id) {
        personalStatus = saved
      } else if (++teamAttempt === 1) {
        if (scenario !== "hold-between-attempts") holdPersonal()
        if (scenario !== "team-saved") throw new Error("Synthetic Team failure")
      }
      if (consent.destinationId === team.id) teamStatus = saved
      return saved
    })
    const session = new CloudSession(port, false, true)
    const stop = session.subscribe(vi.fn())
    try {
      await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
      session.selectProject("studio", true, personal.id)
      session.selectProject("docs", true, team.id)

      await session.startSync()
      const nextStep = "Review saved folders in Settings before trying again."
      const blocked = scenario === "team-saved" ? "Personal" : "Personal, Example Team"
      const error =
        scenario === "hold-between-attempts"
          ? "Could not confirm folder sync for Example Team. Review the remaining choices and try again."
          : `Could not confirm folder sync for ${blocked}. ${nextStep}`
      expect(session.getSnapshot()).toMatchObject({ screen: "folders", error })

      emit({ syncEnabled: true, draft: { screen: "complete", folders: [] } })
      expect(session.getSnapshot()).toMatchObject({ screen: "folders", error })
      const destinations: string[] = [personal.id, team.id]
      if (scenario !== "team-saved") {
        if (scenario === "hold-between-attempts") holdPersonal()
        if (scenario === "retry-after-removal") {
          personalStatus = {
            ...personalStatus,
            policyRevision: "removed-personal-folders",
            projects: [],
            projectChoices: { studio: false },
          }
          emitSync(personalStatus)
        }
        await session.startSync()
        destinations.push(team.id)
        expect(session.getSnapshot()).toMatchObject(
          scenario === "retry-after-removal"
            ? { screen: "complete", error: null }
            : {
                screen: "folders",
                error: `Could not confirm folder sync for Personal. ${nextStep}`,
              },
        )
      }
      expect(ongoing.enable.mock.calls.map(([consent]) => consent.destinationId)).toEqual(
        destinations,
      )
      expect(ongoing.setProject).not.toHaveBeenCalled()
      expect(ongoing.confirmTeamFolders).not.toHaveBeenCalled()
    } finally {
      stop()
    }
  },
)

it("starts Personal only for the explicitly staged folder when Team is already saved", async () => {
  const { port, ongoing, savedTeam, folders } = teamFolderFixture()
  ongoing.enable.mockRejectedValueOnce(new Error("Synthetic failure"))
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
    const original = session.getSnapshot()
    session.selectProject("studio", true, personal.id)
    expect(session.getSnapshot().projects?.targets).toEqual({})
    await session.startPendingProjects(session.getSnapshot())
    expect(ongoing.enable).not.toHaveBeenCalled()

    session.selectProject("docs", true, personal.id)
    await session.startPendingProjects(original)
    expect(ongoing.enable).not.toHaveBeenCalled()
    expect(session.getSnapshot().projects?.targets).toEqual({ docs: personal.id })
    await session.startPendingProjects(session.getSnapshot())
    expect(session.getSnapshot().projects?.targets).toEqual({ docs: personal.id })
    await session.startPendingProjects(session.getSnapshot())
    expect(ongoing.enable).toHaveBeenLastCalledWith(
      expect.objectContaining({
        destinationId: personal.id,
        projects: folders[0]!.projects,
        projectChoices: { docs: true },
      }),
    )
    expect(session.getSnapshot().projects?.targets).toEqual({})
    expect(session.getSnapshot().teamStatuses?.[team.id]).toMatchObject(savedTeam)
  } finally {
    stop()
  }
})

it("keeps the first Settings draft when a newer policy supersedes the save reply", async () => {
  const { port, ongoing, emitSync } = teamFolderFixture()
  ongoing.enable.mockImplementationOnce(async (consent) => {
    emitSync(ongoingStatus({ policyRevision: "newer-personal-policy" }))
    return ongoingStatus({
      status: "ready",
      policyRevision: "saved-personal-policy",
      consentVersion: consent.version,
      projects: consent.projects,
      projectChoices: consent.projectChoices,
    })
  })
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
    session.selectProject("docs", true, personal.id)
    expect(await session.startPendingProjects(session.getSnapshot())).toBe(false)
    expect(session.getSnapshot()).toMatchObject({
      ongoing: { policyRevision: "newer-personal-policy" },
      projects: { choices: { docs: true }, targets: { docs: personal.id } },
      uploadActionError: "Could not save the folder choices.",
    })
    expect(ongoing.enable).toHaveBeenCalledOnce()

    expect(await session.startPendingProjects(session.getSnapshot())).toBe(true)
    expect(ongoing.enable.mock.calls[1]).toEqual(ongoing.enable.mock.calls[0])
    expect(session.getSnapshot().projects?.targets).toEqual({})
  } finally {
    stop()
  }
})

it.each(["rejected", "unconfirmed"] as const)(
  "keeps a %s incremental Team addition staged and clears only confirmed additions",
  async (failure) => {
    const { port, ongoing, savedTeam, folders } = teamFolderFixture()
    const updated = {
      ...savedTeam,
      policyRevision: "team-policy-2",
      projectChoices: { ...savedTeam.projectChoices, docs: true },
      projects: [...savedTeam.projects, ...folders[0]!.projects],
    }
    ongoing.setProject.mockResolvedValueOnce(updated)
    if (failure === "rejected")
      ongoing.setProject.mockRejectedValueOnce(new Error("Synthetic failure"))
    else ongoing.setProject.mockResolvedValueOnce(updated)
    const session = new CloudSession(port, false, true)
    const stop = session.subscribe(vi.fn())
    try {
      await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
      session.selectProject("docs", true, team.id)
      session.selectProject("website", true, team.id)
      expect(await session.startPendingProjects(session.getSnapshot())).not.toBe(true)
      expect(ongoing.enable).not.toHaveBeenCalled()
      expect(ongoing.setProject).toHaveBeenNthCalledWith(
        1,
        expect.objectContaining({
          destinationId: team.id,
          groupId: "docs",
          selected: true,
          expectedPolicyRevision: savedTeam.policyRevision,
        }),
      )
      expect(ongoing.setProject).toHaveBeenNthCalledWith(
        2,
        expect.objectContaining({
          destinationId: team.id,
          groupId: "website",
          selected: true,
          expectedPolicyRevision: updated.policyRevision,
        }),
      )
      expect(session.getSnapshot().teamStatuses?.[team.id]).toMatchObject(updated)
      expect(session.getSnapshot().projects).toMatchObject({
        choices: { website: true },
        targets: { website: team.id },
      })
      expect(session.getSnapshot().projects?.targets).not.toHaveProperty("docs")
      expect(session.getSnapshot().sync.status).toBe("ambiguous")
    } finally {
      stop()
    }
  },
)

it("keeps a Team addition staged until the renewal confirms its grant", async () => {
  const { port, ongoing, savedTeam, folders } = teamFolderFixture()
  const stale = {
    ...savedTeam,
    folderConfirmationRequired: true,
    savedFolders: savedTeam.projects.map(({ projectPath }) => ({
      groupId: "studio",
      path: projectPath,
    })),
  }
  ongoing.status.mockResolvedValue(ongoingStatus({ teamStatuses: [stale] }))
  const confirmTeamFolders = vi
    .fn<NonNullable<CloudOngoingPort["confirmTeamFolders"]>>()
    .mockResolvedValueOnce(stale)
    .mockResolvedValueOnce({
      ...savedTeam,
      policyRevision: "renewed-team-policy",
      projects: [...savedTeam.projects, ...folders[0]!.projects],
    })
  ongoing.confirmTeamFolders = confirmTeamFolders
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
    session.selectProject("docs", true, team.id)
    expect(await session.startPendingProjects(session.getSnapshot(), true, team.id)).toBe(false)
    expect(session.getSnapshot().projects).toMatchObject({
      choices: { docs: true },
      targets: { docs: team.id },
    })
    expect(confirmTeamFolders).toHaveBeenCalledOnce()

    expect(await session.startPendingProjects(session.getSnapshot(), true, team.id)).toBe(true)
    expect(confirmTeamFolders.mock.calls[1]).toEqual(confirmTeamFolders.mock.calls[0])
    expect(session.getSnapshot().projects?.targets).toEqual({})
  } finally {
    stop()
  }
})

it("keeps old Team consent and does not fall back to Personal when a relink is rejected", async () => {
  const { port, ongoing, savedTeam, folders } = teamFolderFixture()
  const begin = vi.spyOn(port, "begin")
  ongoing.setProject.mockRejectedValueOnce(
    "Turn off all folders under the old Team policy before approving new uploads.",
  )
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
    session.selectProject("docs", true, team.id)
    await session.startPendingProjects(session.getSnapshot())

    expect(ongoing.setProject).toHaveBeenCalledExactlyOnceWith({
      accountId: "synthetic-account",
      destinationId: team.id,
      groupId: "docs",
      selected: true,
      projects: folders[0]!.projects,
      folderPaths: folders[0]!.folderPaths,
      expectedPolicyRevision: savedTeam.policyRevision,
    })
    expect(ongoing.enable).not.toHaveBeenCalled()
    expect(begin).not.toHaveBeenCalled()
    expect(session.getSnapshot()).toMatchObject({
      uploadActionError: "Review saved folders before saving new Team choices.",
      projects: { targets: { docs: team.id } },
      teamStatuses: { [team.id]: savedTeam },
    })
  } finally {
    stop()
  }
})

it("requires turning off a saved source before staging that folder for another workspace", async () => {
  const { port, ongoing, savedTeam } = teamFolderFixture()
  ongoing.setProject.mockResolvedValueOnce({
    ...savedTeam,
    policyRevision: "team-policy-2",
    projects: [],
    projectChoices: { ...savedTeam.projectChoices, studio: false },
  })
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
    session.selectProject("studio", true, personal.id)
    expect(session.getSnapshot().projects?.targets).toEqual({})

    await session.setSavedProject(session.getSnapshot(), "studio", false, team.id)
    expect(ongoing.setProject).toHaveBeenCalledExactlyOnceWith(
      expect.objectContaining({
        destinationId: team.id,
        groupId: "studio",
        selected: false,
        expectedPolicyRevision: savedTeam.policyRevision,
      }),
    )
    expect(ongoing.enable).not.toHaveBeenCalled()
    expect(session.getSnapshot().projects?.targets).toEqual({})
    session.selectProject("studio", true, personal.id)
    await session.startPendingProjects(session.getSnapshot())
    expect(ongoing.enable).toHaveBeenCalledExactlyOnceWith(
      expect.objectContaining({ destinationId: personal.id, projects: savedTeam.projects }),
    )
    expect(session.getSnapshot().teamStatuses?.[team.id]?.projectChoices).toEqual({
      studio: false,
      excluded: false,
    })
  } finally {
    stop()
  }
})
