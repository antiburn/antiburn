import type {
  CloudEligibleDestination,
  CloudOngoingConsent,
  CloudOngoingStatus,
  CloudProjectGroup,
  CloudProjectSelection,
  CloudSnapshot,
  CloudSourceSelection,
} from "../lib/cloud"

/** A draft edit never changes the policy. Null is an explicit opt-out. */
export type FolderRoutes = Readonly<Partial<Record<string, string | null>>>

/** The rendered selection carries IDs only. The session checks the exact source roots. */
export interface FolderAssignment {
  folderId: string
  destinationId: string
}

/** Fence consent on membership changes, not destination order or display names. */
export function folderRoutingAuthority(value: CloudSnapshot) {
  return JSON.stringify([
    value.identity?.id,
    value.status,
    value.setupError,
    value.captureEnabled,
    value.destinations === undefined
      ? "legacy"
      : (value.destinations
          ?.map((entry) => JSON.stringify([entry.id, entry.kind, entry.membershipGeneration]))
          .sort() ?? null),
  ])
}

function pathsOverlap(left: string, right: string) {
  return (
    left === right ||
    left.startsWith(`${right.replace(/\/+$/, "")}/`) ||
    right.startsWith(`${left.replace(/\/+$/, "")}/`)
  )
}

/** Do not offer to move a folder that already has approved paths. */
export function lockedFolderRoutes(
  groups: readonly CloudProjectGroup[],
  policies: ReadonlyMap<string, CloudOngoingStatus>,
): FolderRoutes {
  return Object.fromEntries(
    groups.flatMap((group) => {
      for (const [id, policy] of policies) {
        if (
          policy.projects.some((project) =>
            group.memberPaths.some((path) => pathsOverlap(path, project.projectPath)),
          )
        )
          return [[group.id, id]]
      }
      return []
    }),
  )
}

function projectKey(project: CloudProjectSelection) {
  return JSON.stringify([project.sourceKind, project.sourcePath, project.projectPath])
}

function sourceKey(source: CloudSourceSelection) {
  return JSON.stringify([source.sourceKind, source.path])
}

/** Add exact new bindings without deleting prior approvals or clearing their pause. */
export function folderRouteConsent(
  accountId: string,
  destination: CloudEligibleDestination,
  policy: CloudOngoingStatus,
  folderIds: readonly string[],
  projects: readonly CloudProjectSelection[],
): CloudOngoingConsent | null {
  if (
    !projects.length ||
    policy.paused ||
    policy.requiresDestinationReview ||
    (policy.consentVersion && !policy.policyRevision)
  )
    return null
  const allProjects = new Map(
    [...policy.projects, ...projects].map((project) => [projectKey(project), project]),
  )
  const sources = new Map(policy.selectedSources.map((source) => [sourceKey(source), source]))
  for (const project of projects) {
    const source = { sourceKind: project.sourceKind, path: project.sourcePath }
    sources.set(sourceKey(source), source)
  }
  return {
    accountId,
    destinationId: destination.id,
    membershipGeneration: destination.membershipGeneration,
    ...(policy.consentVersion && policy.policyRevision
      ? { expectedPolicyRevision: policy.policyRevision }
      : {}),
    version: "desktop-project-sync-v1",
    fullContent: true,
    ongoing: true,
    attachments: false,
    selectedSources: [...sources.values()],
    projects: [...allProjects.values()],
    projectChoices: {
      ...policy.projectChoices,
      ...Object.fromEntries(folderIds.map((id) => [id, true])),
    },
  }
}

/** Overlapping folder trees cannot create new routes to different destinations. */
export function folderRoutesOverlap(consents: readonly CloudOngoingConsent[]) {
  return consents.some((consent, index) =>
    consents
      .slice(index + 1)
      .some((other) =>
        consent.projects.some((project) =>
          other.projects.some((candidate) =>
            pathsOverlap(project.projectPath, candidate.projectPath),
          ),
        ),
      ),
  )
}

/** A lost reply counts as saved only when the exact binding and policy are present. */
export function matchesFolderConsent(status: CloudOngoingStatus, consent: CloudOngoingConsent) {
  return (
    status.storageKnown &&
    status.accountId === consent.accountId &&
    status.destinationId === consent.destinationId &&
    status.consentVersion === consent.version &&
    !status.requiresDestinationReview &&
    !status.paused &&
    JSON.stringify(status.projects.map(projectKey).sort()) ===
      JSON.stringify(consent.projects.map(projectKey).sort()) &&
    JSON.stringify(status.selectedSources.map(sourceKey).sort()) ===
      JSON.stringify(consent.selectedSources.map(sourceKey).sort()) &&
    JSON.stringify(Object.entries(status.projectChoices ?? {}).sort()) ===
      JSON.stringify(Object.entries(consent.projectChoices).sort())
  )
}
