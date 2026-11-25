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
const repository = {
  groupId: "repo-other",
  projects: [{ ...group.projects[0]!, projectPath: "/Example/Other" }],
}
const removedPolicy = {
  consentVersion: "desktop-project-sync-v1",
  policyRevision: "removed",
  projectChoices: { [group.id]: false, [repository.groupId]: true },
  projects: repository.projects,
}
const removed = ongoingStatus({
  ...removedPolicy,
  status: "ready",
  repositoryGroupIds: [repository.groupId],
  repositoryFolders: [repository],
  exactProjects: [],
})
const savedPersonal = {
  ...removed,
  policyRevision: "personal-before",
  projectChoices: { ...removed.projectChoices, [group.id]: true },
  projects: [...group.projects, ...repository.projects],
  exactProjects: group.projects,
}
const savedTeam = ongoingStatus({
  destinationId: team,
  status: "ready",
  consentVersion: "desktop-project-sync-v1",
  policyRevision: "team-before",
})
const fallback = ongoingStatus({
  ...removedPolicy,
  status: "storage_unavailable",
  storageKnown: false,
})

/** Start with an exact folder and an unrelated repository saved in Personal. */
async function startSession() {
  const fixture = cloudProjectFixture()
  fixture.ongoing.status.mockResolvedValue({ ...savedPersonal, teamStatuses: [savedTeam] })
  fixture.port.workspaces = vi.fn<NonNullable<typeof fixture.port.workspaces>>(async () => [
    { id: personal, kind: "personal", name: "Personal" },
    { id: team, kind: "team", name: "Team" },
  ])
  const session = new CloudSession(fixture.port, false, true)
  const stop = session.subscribe(vi.fn())
  await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
  fixture.ongoing.status.mockClear()
  return { ...fixture, session, stop }
}

it.each([
  "incomplete reply",
  "incomplete reply and event",
  "rejected reply and event",
] as const)(
  "confirms source removal by readback after an %s without writing again",
  async (result) => {
    const { session, ongoing, emitSync, stop } = await startSession()
    try {
      ongoing.setProject.mockImplementationOnce(async () => {
        if (result !== "incomplete reply") emitSync(fallback)
        if (result === "rejected reply and event") throw new Error("Removal reply unavailable")
        return fallback
      })
      ongoing.status.mockResolvedValueOnce({
        ...removed,
        teamStatuses: [{ ...savedTeam, policyRevision: "team-readback" }],
      })

      await expect(
        session.moveSavedProject(session.getSnapshot(), group.id, personal, team),
      ).resolves.toBe("clear_draft")

      const view = session.getSnapshot()
      expect(ongoing.status).toHaveBeenCalledOnce()
      expect(view.ongoing?.policyRevision).toBe("removed")
      expect(view.ongoing?.repositoryGroupIds).toEqual([repository.groupId])
      expect(view.teamStatuses?.[team]?.policyRevision).toBe("team-before")
      expect(view.uploadActionError).toBe(
        "Sync stopped for this folder. Choose a workspace to sync it again.",
      )
      expect(ongoing.setProject).toHaveBeenCalledExactlyOnceWith(
        expect.objectContaining({ destinationId: personal, selected: false }),
      )
      await session.startPendingProjects(view)
      expect(ongoing.setProject).toHaveBeenCalledOnce()
      expect(ongoing.enable).not.toHaveBeenCalled()
    } finally {
      stop()
    }
  },
)

it.each(["different revision", "missing repository"] as const)(
  "does not accept readback with %s after a fallback event",
  async (change) => {
    const { session, ongoing, emitSync, stop } = await startSession()
    try {
      ongoing.setProject.mockImplementationOnce(async () => {
        emitSync(fallback)
        return fallback
      })
      ongoing.status.mockResolvedValueOnce(
        change === "different revision"
          ? { ...removed, policyRevision: "different" }
          : { ...removed, repositoryGroupIds: [], repositoryFolders: [] },
      )

      await expect(
        session.moveSavedProject(session.getSnapshot(), group.id, personal, team),
      ).resolves.toBe("clear_draft")

      expect(ongoing.status).toHaveBeenCalledOnce()
      expect(session.getSnapshot().ongoing).toEqual(fallback)
      expect(session.getSnapshot().uploadActionError).toMatch(/could not confirm.*old/i)
      expect(ongoing.setProject).toHaveBeenCalledOnce()
      expect(ongoing.enable).not.toHaveBeenCalled()
    } finally {
      stop()
    }
  },
)

it("keeps a device Pause that arrives during source removal readback", async () => {
  const { session, ongoing, emitSync, stop } = await startSession()
  try {
    const deviceStatus = ongoingStatus({
      accountId: null,
      destinationId: null,
      deviceUploadEnabled: true,
    })
    emitSync(deviceStatus)
    expect(session.getSnapshot().deviceUploadEnabled).toBe(true)

    let finish!: (status: ReturnType<typeof ongoingStatus>) => void
    ongoing.setProject.mockRejectedValueOnce(new Error("Removal reply unavailable"))
    ongoing.status.mockImplementationOnce(() => new Promise((resolve) => (finish = resolve)))

    const move = session.moveSavedProject(session.getSnapshot(), group.id, personal, team)
    await vi.waitFor(() => expect(finish).toBeTypeOf("function"))
    emitSync({ ...deviceStatus, deviceUploadEnabled: false })
    expect(session.getSnapshot().deviceUploadEnabled).toBe(false)
    finish({ ...removed, deviceUploadEnabled: true })
    await expect(move).resolves.toBe("clear_draft")

    const view = session.getSnapshot()
    expect(view.ongoing?.policyRevision).toBe("removed")
    expect(view.deviceUploadEnabled).toBe(false)
    expect(view.ongoing).not.toHaveProperty("deviceUploadEnabled")
    expect(ongoing.setProject).toHaveBeenCalledOnce()
    expect(ongoing.enable).not.toHaveBeenCalled()
  } finally {
    stop()
  }
})

it("does not treat unavailable source storage as a confirmed removal", async () => {
  const { session, ongoing, emitSync, stop } = await startSession()
  try {
    emitSync(
      ongoingStatus({
        status: "ready",
        consentVersion: "desktop-project-sync-v1",
        policyRevision: "only-folder",
        projectChoices: { [group.id]: true },
        projects: group.projects,
      }),
    )
    const unavailable = ongoingStatus({
      status: "storage_unavailable",
      storageKnown: false,
      consentVersion: "desktop-project-sync-v1",
      policyRevision: "only-folder-removed",
      projectChoices: { [group.id]: false },
    })
    ongoing.setProject.mockImplementationOnce(async () => {
      emitSync(unavailable)
      return unavailable
    })
    ongoing.status.mockRejectedValueOnce(new Error("Status unavailable"))

    await expect(
      session.moveSavedProject(session.getSnapshot(), group.id, personal, team),
    ).resolves.toBe("clear_draft")

    expect(ongoing.status).toHaveBeenCalledOnce()
    expect(session.getSnapshot().uploadActionError).toMatch(/could not confirm.*old/i)
    expect(ongoing.setProject).toHaveBeenCalledOnce()
    expect(ongoing.enable).not.toHaveBeenCalled()
  } finally {
    stop()
  }
})
