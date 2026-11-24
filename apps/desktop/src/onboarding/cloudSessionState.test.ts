import { expect, it } from "vitest"
import { mergeCloudProjectGroups } from "../lib/cloud"
import {
  expectedFolderRoute,
  folderRouteConfirmed,
  hasSavedProjectPermission,
} from "./cloudSessionState"
import { ongoingStatus, projectGroup } from "./__tests__/cloudProjectFixtures"

it("does not treat a matching checkout path as approved repository scope", () => {
  const group = projectGroup({ repositoryWide: true })
  const policy = ongoingStatus({
    consentVersion: "desktop-project-sync-v1",
    projects: group.projects,
    exactProjects: [],
    repositoryGroupIds: ["different-repository"],
    repositoryFolders: [{ groupId: "different-repository", projects: group.projects }],
  })

  expect(hasSavedProjectPermission(group, policy, true)).toBe(false)
  expect(hasSavedProjectPermission(group, policy, false)).toBe(false)
  const exact = { ...group, repositoryWide: false }
  expect(hasSavedProjectPermission(exact, policy, true)).toBe(false)
  expect(
    hasSavedProjectPermission(exact, { ...policy, exactProjects: group.projects }, true),
  ).toBe(true)
  const binding = { accountId: "synthetic-account", destinationId: "synthetic-personal" }
  expect(folderRouteConfirmed(policy, binding, group.projects)).toBe(false)
  expect(
    folderRouteConfirmed(policy, binding, expectedFolderRoute([group], group.projects)),
  ).toBe(false)
  const legacy = ongoingStatus({
    consentVersion: "desktop-project-sync-v1",
    projects: group.projects,
  })
  expect(folderRouteConfirmed(legacy, binding, group.projects)).toBe(true)
})

it("confirms repository sources by the saved ID even when native chose a different display anchor", () => {
  const requested = projectGroup().projects[0]!
  const group = projectGroup({ repositoryWide: true, projects: [requested] })
  const receiptPath = "/Example/elsewhere/Studio"
  const anchor = { ...requested, projectPath: receiptPath }
  const status = ongoingStatus({
    consentVersion: "desktop-project-sync-v1",
    projects: [anchor],
    exactProjects: [],
    repositoryGroupIds: [group.id],
    repositoryFolders: [{ groupId: group.id, projects: [anchor] }],
  })
  const expected = expectedFolderRoute([group], [requested])
  const binding = { accountId: "synthetic-account", destinationId: "synthetic-personal" }
  expect(folderRouteConfirmed(status, binding, expected)).toBe(true)
  expect(folderRouteConfirmed(status, binding, [requested])).toBe(false)
  expect(
    folderRouteConfirmed(
      {
        ...status,
        repositoryFolders: [
          { groupId: group.id, projects: [{ ...anchor, sourcePath: "/wrong/source" }] },
        ],
      },
      binding,
      expected,
    ),
  ).toBe(false)
})

it("checks exact and repository grants independently when they share the same anchor", () => {
  const shared = projectGroup().projects[0]!
  const repo = projectGroup({ id: "repo-studio", repositoryWide: true, projects: [shared] })
  const exact = projectGroup({ projects: [shared] })
  const repoOnly = ongoingStatus({
    consentVersion: "desktop-project-sync-v1",
    projects: [shared],
    exactProjects: [],
    repositoryGroupIds: [repo.id],
    repositoryFolders: [{ groupId: repo.id, projects: [shared] }],
  })
  const binding = { accountId: "synthetic-account", destinationId: "synthetic-personal" }
  const expected = expectedFolderRoute([repo, exact], [shared])

  expect(folderRouteConfirmed(repoOnly, binding, expected)).toBe(false)
  expect(
    folderRouteConfirmed(
      { ...repoOnly, exactProjects: [shared], repositoryFolders: [] },
      binding,
      expected,
    ),
  ).toBe(false)
  expect(
    folderRouteConfirmed({ ...repoOnly, exactProjects: [shared] }, binding, expected),
  ).toBe(true)
})

it("keeps one repository anchor per source when discovery pages include another checkout", () => {
  const first = projectGroup().projects[0]!
  const checkout = "/Example/elsewhere/Studio"
  const sameSource = { ...first, projectPath: checkout }
  const otherSource = { ...sameSource, sourcePath: "/Example/another/source" }
  const group = projectGroup({
    repositoryWide: true,
    folderPaths: [first.projectPath],
    projects: [first],
  })
  const page = projectGroup({
    repositoryWide: true,
    path: checkout,
    folderPaths: [checkout],
    projects: [sameSource, otherSource],
  })

  const [merged] = mergeCloudProjectGroups([group], [page])
  expect(merged?.projects).toEqual([first, otherSource])
  expect(merged?.folderPaths).toEqual([first.projectPath, checkout])
  expect(merged?.path).toBe(first.projectPath)
})
