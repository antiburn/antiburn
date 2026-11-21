import { expect, it, vi } from "vitest"
import type { CloudOngoingConsent } from "../lib/cloud"
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
    expect(saved.projectChoices).toEqual({ studio: true, website: false, "new-project": true })
    expect(saved.projects.some((project) => project.projectPath === "/Example/New")).toBe(true)
    expect(saved.projects.some((project) => project.projectPath === "/Example/Website")).toBe(
      false,
    )
  } finally {
    stop()
  }
})

it("requires review instead of restoring unknown choices from an older policy", async () => {
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
    session.selectProject("studio", true)
    await session.startSync()
    expect(ongoing.enable).toHaveBeenCalledWith(
      expect.objectContaining({
        projectChoices: { studio: true },
      }),
    )
  } finally {
    stop()
  }
})
