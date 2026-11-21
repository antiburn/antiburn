import type {
  CloudDestinationBinding,
  CloudEligibleDestination,
  CloudOngoingConsent,
  CloudOngoingPort,
  CloudOngoingStatus,
  CloudProjectGroup,
  CloudProjectSelection,
  CloudSnapshot,
  CloudSourceSelection,
  DesktopCloudProjectGroup,
} from "../lib/cloud"
import { cloudSourceKey } from "../lib/cloud"
import type { CloudViewSnapshot } from "./CloudSession"

/** Compare consent-relevant roots without treating display-count changes as edits. */
export function selectedFoldersKey(value: CloudSnapshot) {
  return JSON.stringify(
    value.draft.folders
      .filter((folder) => folder.selected)
      .map(cloudSourceKey)
      .sort(),
  )
}

/** Bind a legacy Start action to authority and exact selection, not display counts. */
export function confirmationContextKey(value: CloudViewSnapshot) {
  return JSON.stringify([
    value.cloud.identity?.id,
    value.cloud.destination?.id,
    value.cloud.destination?.kind,
    value.cloud.status,
    value.cloud.captureEnabled,
    value.cloud.setupError,
    selectedFoldersKey(value.cloud),
    value.projects?.selectionKey,
  ])
}

/** A draft edit never changes the policy. Null is an explicit opt-out. */
export type FolderRoutes = Readonly<Partial<Record<string, string | null>>>

/** The rendered selection carries IDs only. The session checks the exact source roots. */
export interface FolderAssignment {
  folderId: string
  destinationId: string
}

/** Inventory order is stable. A missing or unreadable inventory is not empty. */
export function sameSavedFolderInventory(
  left: readonly string[] | null | undefined,
  right: readonly string[] | null | undefined,
) {
  return JSON.stringify(left) === JSON.stringify(right)
}

/** Pause a saved route by ID. Resume also requires current destination permission. */
export function folderRouteControlBinding(
  cloud: CloudSnapshot,
  statuses: readonly CloudOngoingStatus[] | null | undefined,
  destinationId: string | undefined,
  paused: boolean,
): CloudDestinationBinding | null {
  const policy = statuses?.find((status) => status.destinationId === destinationId)
  const eligible = cloud.destinations?.some((destination) => destination.id === destinationId)
  return cloud.identity &&
    cloud.status === "signed_in" &&
    destinationId &&
    cloud.savedDestinationIds?.includes(destinationId) &&
    policy?.storageKnown &&
    policy.policyRevision &&
    (paused || (eligible && !policy.requiresDestinationReview))
    ? { accountId: cloud.identity.id, destinationId }
    : null
}

/** Report save outcomes without exposing source or provider details. */
export function folderRouteResults(
  entries: readonly { consent: CloudOngoingConsent; name: string; saved: boolean }[],
) {
  return entries.map(({ consent, name, saved }) => ({ id: consent.destinationId, name, saved }))
}

/** Read the full saved policy set. Do not turn a stale or missing read into no consent. */
export async function readSavedFolderPolicies(
  port: CloudOngoingPort,
  accountId: string,
  destinationIds: readonly string[],
  current: () => boolean,
): Promise<Map<string, CloudOngoingStatus> | null> {
  const policies = new Map<string, CloudOngoingStatus>()
  for (const destinationId of destinationIds) {
    const status = await port.status({ accountId, destinationId })
    if (!current()) return null
    if (
      !status?.storageKnown ||
      status.accountId !== accountId ||
      status.destinationId !== destinationId ||
      !status.policyRevision
    )
      throw new Error("Cloud policy is unavailable.")
    policies.set(destinationId, status)
  }
  return policies
}

/** A partial status cache cannot report the complete set of saved routes. */
export function savedFolderPolicyStatuses(
  destinationIds: readonly string[] | null | undefined,
  policies: ReadonlyMap<string, CloudOngoingStatus>,
): readonly CloudOngoingStatus[] | null {
  if (!destinationIds) return null
  const states = destinationIds.flatMap((id) => {
    const status = policies.get(id)
    return status ? [status] : []
  })
  return states.length === destinationIds.length ? states : null
}

/** Find the saved Personal policy without using the last approved destination. */
export function savedPersonalFolderPolicy(
  cloud: CloudSnapshot,
  policies: readonly CloudOngoingStatus[] | null | undefined,
) {
  const personalId = cloud.destinations?.find(({ kind }) => kind === "personal")?.id
  if (!personalId || !cloud.identity || !cloud.savedDestinationIds?.includes(personalId))
    return null
  return (
    policies?.find(
      (policy) =>
        policy.accountId === cloud.identity?.id &&
        policy.destinationId === personalId &&
        policy.consentVersion === "desktop-project-sync-v1",
    ) ?? null
  )
}

/** Keep folder revocation bound to the same Personal policy and group the user saw. */
export function personalFolderRemovalBinding(
  cloud: CloudSnapshot,
  policy: CloudOngoingStatus | null | undefined,
  prior: CloudOngoingStatus | null | undefined,
  discovery: CloudDestinationBinding | null,
): CloudDestinationBinding | null {
  return cloud.status === "signed_in" &&
    cloud.identity &&
    policy?.storageKnown &&
    policy.policyRevision &&
    policy.policyRevision === prior?.policyRevision &&
    policy.accountId === prior.accountId &&
    policy.destinationId === prior.destinationId &&
    discovery?.accountId === policy.accountId &&
    discovery?.destinationId === policy.destinationId
    ? discovery
    : null
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

/** Use an explicit authorized destination for discovery, not for consent. */
export function folderDiscoveryBinding(cloud: CloudSnapshot): CloudDestinationBinding | null {
  const { destinations, identity, status, setupError } = cloud
  const savedPersonal = destinations?.find(
    ({ id, kind }) => kind === "personal" && cloud.savedDestinationIds?.includes(id),
  )
  const first = savedPersonal?.id ?? destinations?.map((entry) => entry.id).sort()[0]
  return identity && first && status === "signed_in" && !setupError
    ? { accountId: identity.id, destinationId: first }
    : null
}

function pathsOverlap(left: string, right: string) {
  return (
    left === right ||
    left.startsWith(`${right.replace(/\/+$/, "")}/`) ||
    right.startsWith(`${left.replace(/\/+$/, "")}/`)
  )
}

/** Show saved routes above or inside a local folder group. */
export function folderGroupHasPolicy(
  group: DesktopCloudProjectGroup | undefined,
  policy: CloudOngoingStatus | null | undefined,
) {
  return (
    group?.environment === "native" &&
    !!policy?.projects.some((project) =>
      [group.path, ...group.folderPaths].some((path) =>
        pathsOverlap(path, project.projectPath),
      ),
    )
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
  policy: CloudOngoingStatus | undefined,
  folderIds: readonly string[],
  projects: readonly CloudProjectSelection[],
): CloudOngoingConsent | null {
  if (
    !projects.length ||
    policy?.paused ||
    policy?.requiresDestinationReview ||
    (policy?.consentVersion && !policy.policyRevision)
  )
    return null
  const allProjects = new Map(
    [...(policy?.projects ?? []), ...projects].map((project) => [projectKey(project), project]),
  )
  const sources = new Map(policy?.selectedSources.map((source) => [sourceKey(source), source]))
  for (const project of projects) {
    const source = { sourceKind: project.sourceKind, path: project.sourcePath }
    sources.set(sourceKey(source), source)
  }
  return {
    accountId,
    destinationId: destination.id,
    membershipGeneration: destination.membershipGeneration,
    ...(policy?.consentVersion && policy.policyRevision
      ? { expectedPolicyRevision: policy.policyRevision }
      : {}),
    version: "desktop-project-sync-v1",
    fullContent: true,
    ongoing: true,
    attachments: false,
    selectedSources: [...sources.values()],
    projects: [...allProjects.values()],
    projectChoices: {
      ...policy?.projectChoices,
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
