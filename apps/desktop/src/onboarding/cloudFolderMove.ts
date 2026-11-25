import type {
  CloudDestinationBinding,
  CloudOngoingPort,
  CloudOngoingStatus,
  CloudProjectChoices,
  CloudProjectSelection,
  CloudSnapshot,
  DesktopCloudProjectGroup,
} from "../lib/cloud"
import type { ProjectPages } from "./cloudProjectPages"
import { projectSyncConsent } from "./cloudProjectRoutes"
import {
  cloudUploadStatusUnknown,
  hasSavedProjectPermission,
  matchesUploadPolicy,
  projectBinding,
  projectConfirmationKey,
  savedExactProjects,
  savedRepositoryProjects,
  selectedProjects,
  supportedProjectSelections,
  uploadPolicy,
  type CloudViewSnapshot,
} from "./cloudSessionState"

interface FolderMoveSession {
  getPort: () => CloudOngoingPort | undefined
  getSnapshot: () => CloudViewSnapshot
  getDiscovery: () => { pages: ProjectPages | undefined; choices: CloudProjectChoices }
  canStart: (context: CloudViewSnapshot) => boolean
  scope: () => FolderMoveScope & { generation: number }
  setInFlight: (busy: boolean) => void
  publish: (patch: Pick<Partial<CloudViewSnapshot>, "busy" | "uploadActionError">) => void
  receiveStatus: (status: CloudOngoingStatus) => void
}

interface FolderMoveScope {
  authority: number
  account: number
}

/** Keep the workspace draft only if the same choice can be retried before removal. */
export type FolderMoveOutcome = "keep_draft" | "clear_draft"

const confirmTeamFoldersMessage = "Confirm the saved Team folders before moving this folder."
const folderChangedMessage =
  "The saved folder or workspace changed. Review the folder choices and try again."
const removalUnknownMessage =
  "Could not confirm that the old folder rule is off. Check the saved folder before trying again."

/** Fence policy writes on any authority change and messages on account changes. */
export function advanceFolderMoveScope(
  scope: FolderMoveScope,
  previous: CloudSnapshot,
  next: CloudSnapshot,
): FolderMoveScope {
  const sameAccount = next.status === "signed_in" && previous.identity?.id === next.identity?.id
  return { authority: scope.authority + 1, account: scope.account + Number(!sameAccount) }
}

/** Remove the old rule before saving a new one. A failed move never restores the old rule. */
export function createFolderMove(session: FolderMoveSession) {
  const {
    getPort,
    getSnapshot,
    getDiscovery,
    canStart,
    scope,
    setInFlight,
    publish,
    receiveStatus,
  } = session
  return async (
    context: CloudViewSnapshot,
    id: string,
    from: string,
    to: string | undefined,
  ): Promise<FolderMoveOutcome> => {
    const port = getPort()
    const snapshot = getSnapshot()
    const anchor = projectBinding(snapshot.cloud, false)
    const { pages } = getDiscovery()
    const group = pages?.groups.find((item) => item.id === id)
    if (
      !port ||
      !anchor ||
      !group ||
      from === to ||
      to === "" ||
      snapshot.busy ||
      !canStart(context)
    )
      return "clear_draft"

    const started = scope()
    const sources = pages?.sources ?? []
    const confirmationKey = projectConfirmationKey(
      group,
      supportedProjectSelections(group, sources),
    )
    const sameGeneration = () => started.generation === scope().generation
    const sameAccount = () => sameGeneration() && started.account === scope().account
    const current = () => sameAccount() && started.authority === scope().authority
    const verified = (view: CloudViewSnapshot, workspace: string) =>
      workspaceVerified(view, workspace, anchor.destinationId)
    const routeCurrent = (view: CloudViewSnapshot) =>
      groupCurrent(view, id, confirmationKey) &&
      verified(view, from) &&
      (!to || verified(view, to))
    const policyOf = (workspace: string, view: CloudViewSnapshot = getSnapshot()) =>
      uploadPolicy(view, workspace === anchor.destinationId ? undefined : workspace)
    const targetPolicy = to ? policyOf(to, snapshot) : null
    const targetKey = policyKey(targetPolicy)
    const sourcePolicy = policyOf(from, snapshot)
    const sourceRevision = sourcePolicy?.policyRevision
    const source = { ...anchor, destinationId: from }
    const target = to ? { ...anchor, destinationId: to } : null
    const projects = selectedProjects([group], sources, { [id]: true })
    const expectedRemovals =
      sourcePolicy &&
      (group.repositoryWide
        ? savedRepositoryProjects(sourcePolicy, id)
        : savedExactProjects(sourcePolicy).filter((project) =>
            hasSavedProjectPermission(group, { projects: [project] }, true),
          ))
    const hasOtherGrant = (view: CloudViewSnapshot) => hasGrant(view, group, from)
    const sourceCurrent = () => policyKey(policyOf(from)) === policyKey(sourcePolicy)
    const targetCurrent = () => !to || policyKey(policyOf(to)) === targetKey
    const displayedTarget = to ? policyOf(to, context) : null
    const readyToRemove = () => {
      const latest = getSnapshot()
      return (
        current() &&
        routeCurrent(latest) &&
        sourceCurrent() &&
        targetCurrent() &&
        !hasOtherGrant(latest)
      )
    }

    if (
      target &&
      (displayedTarget?.folderConfirmationRequired || targetPolicy?.folderConfirmationRequired)
    ) {
      publish({ uploadActionError: confirmTeamFoldersMessage })
      return "clear_draft"
    }
    if (
      !sourceRevision ||
      !matchesUploadPolicy(sourcePolicy, source, sourceRevision) ||
      sourcePolicy.consentVersion !== "desktop-project-sync-v1" ||
      sourcePolicy.status === "storage_unavailable" ||
      (from === anchor.destinationId && !sourcePolicy.storageKnown) ||
      policyKey(policyOf(from, context)) !== policyKey(sourcePolicy) ||
      policyKey(displayedTarget) !== targetKey ||
      !targetReady(targetPolicy, target) ||
      cloudUploadStatusUnknown(snapshot.ongoing) ||
      !snapshot.ongoing ||
      !routeCurrent(context) ||
      !routeCurrent(snapshot) ||
      !expectedRemovals?.length ||
      (target && !policyMatchesGroup(sourcePolicy, group, projects)) ||
      hasOtherGrant(context) ||
      hasOtherGrant(snapshot)
    ) {
      publish({ uploadActionError: folderChangedMessage })
      return "clear_draft"
    }

    let removal: "not_started" | "unconfirmed" | "confirmed" = "not_started"
    const confirmedRemoval = (status: CloudOngoingStatus | null | undefined) =>
      removalConfirmed(status, sourcePolicy, source, sourceRevision, group, expectedRemovals)
    const confirmedTarget = (status: CloudOngoingStatus | null | undefined) =>
      !!target &&
      !!status?.policyRevision &&
      status.policyRevision !== targetPolicy?.policyRevision &&
      matchesUploadPolicy(status, target, status.policyRevision) &&
      status.consentVersion === "desktop-project-sync-v1" &&
      (status.storageKnown ||
        (target.destinationId !== anchor.destinationId &&
          status.status === "reauth_required")) &&
      status.projectChoices?.[id] === true &&
      policyMatchesGroup(status, group, projects)
    setInFlight(true)
    publish({ busy: true, uploadActionError: null })
    try {
      if (target && to !== anchor.destinationId && targetPolicy?.projects.length) {
        // A local Team policy event can omit the membership check.
        const status = await port.status()
        if (!readyToRemove()) {
          if (sameAccount()) publish({ uploadActionError: folderChangedMessage })
          return "clear_draft"
        }
        const checked = checkedPolicy(status, anchor, target.destinationId)
        const checkedSource = checkedPolicy(status, anchor, from)
        if (checked?.folderConfirmationRequired) {
          receiveStatus(checked)
          publish({ uploadActionError: confirmTeamFoldersMessage })
          return "clear_draft"
        }
        if (
          (checked && policyKey(checked) !== targetKey) ||
          (checkedSource && policyKey(checkedSource) !== policyKey(sourcePolicy))
        ) {
          publish({ uploadActionError: folderChangedMessage })
          return "clear_draft"
        }
        if (!checked || checked.status === "offline" || checked.status === "route_denied")
          throw new Error("Could not verify the Team folder rules.")
      }
      if (!readyToRemove()) {
        if (sameAccount()) publish({ uploadActionError: folderChangedMessage })
        return "clear_draft"
      }
      removal = "unconfirmed"
      const receipt = await port.setProject({
        ...source,
        groupId: id,
        selected: false,
        projects: [],
        folderPaths: [...new Set([group.path, ...group.folderPaths])],
        expectedPolicyRevision: sourceRevision,
        expectedRemovals,
      })
      if (!current()) return "clear_draft"
      const removalRevision = receipt.policyRevision
      if (!removalRevision || !confirmedRemoval(receipt))
        throw new Error("Cloud did not confirm the folder removal.")

      // The matching event can arrive first. Do not replace it or a newer policy.
      if (sourceCurrent()) receiveStatus(receipt)
      if (!current()) return "clear_draft"
      const observedSource = policyOf(from)
      if (
        observedSource?.policyRevision !== removalRevision ||
        !confirmedRemoval(observedSource)
      )
        throw new Error("The saved folder changed after removal.")
      removal = "confirmed"
      if (!target) return "clear_draft"
      if (!routeCurrent(getSnapshot()) || !targetCurrent() || hasOtherGrant(getSnapshot()))
        throw new Error("The folder or target workspace changed after removal.")

      const moveFrom = { destinationId: from, policyRevision: removalRevision }
      const result =
        targetPolicy?.consentVersion === "desktop-project-sync-v1" &&
        targetPolicy.policyRevision
          ? await port.setProject({
              ...target,
              groupId: id,
              selected: true,
              projects,
              folderPaths: group.folderPaths,
              expectedPolicyRevision: targetPolicy.policyRevision,
              moveFrom,
            })
          : await port.enable({
              ...projectSyncConsent(target, getDiscovery().pages?.sources ?? [], projects, {
                ...(target.destinationId === anchor.destinationId
                  ? getDiscovery().choices
                  : {}),
                [id]: true,
              }),
              moveFrom,
            })
      if (!current()) return "clear_draft"
      if (targetCurrent() && confirmedTarget(result)) receiveStatus(result)
      if (!confirmedTarget(policyOf(target.destinationId)))
        throw new Error("Cloud did not confirm the new folder rule.")
      return "clear_draft"
    } catch {
      if (removal === "not_started") {
        if (!readyToRemove()) {
          if (sameAccount()) publish({ uploadActionError: folderChangedMessage })
          return "clear_draft"
        }
        publish({
          uploadActionError:
            "Could not verify the Team folder rules. Check the workspace and try again.",
        })
        return "keep_draft"
      }
      if (!current()) return "clear_draft"

      const removalUnconfirmed = removal === "unconfirmed"
      const targetToCheck = removal === "confirmed" ? target : null
      if (removalUnconfirmed || targetToCheck) {
        if (targetToCheck && confirmedTarget(policyOf(targetToCheck.destinationId)))
          return "clear_draft"
        const before = getSnapshot()
        const status = await port.status().catch(() => undefined)
        if (!current()) return "clear_draft"
        const checkedPersonal = checkedPolicy(status, anchor, anchor.destinationId)
        const checkedPolicies = checkedPersonal
          ? [checkedPersonal, ...(checkedPersonal.teamStatuses ?? [])].filter(
              (policy) => policy.accountId === anchor.accountId,
            )
          : undefined
        // Readback for an unconfirmed removal can only update the source. An incomplete
        // event can already show the saved revision, so allow complete proof for that revision.
        for (const policy of checkedPolicies ?? []) {
          const workspace = policy.destinationId
          if (!workspace || policy.status === "storage_unavailable") continue
          const observed = policyOf(workspace)
          if (observed !== policyOf(workspace, before)) continue
          if (removalUnconfirmed) {
            const revision = policy.policyRevision
            if (workspace !== from || !revision || !confirmedRemoval(policy)) continue
            if (
              !sourceCurrent() &&
              (confirmedRemoval(observed) || !matchesUploadPolicy(observed, source, revision))
            )
              continue
            // The policy check does not cover device switch events.
            const { deviceUploadEnabled: _deviceUploadEnabled, ...sourceStatus } = policy
            receiveStatus(sourceStatus)
          } else receiveStatus(policy)
        }
        if (
          !current() ||
          (targetToCheck && confirmedTarget(policyOf(targetToCheck.destinationId)))
        )
          return "clear_draft"
        const oldOff = confirmedRemoval(policyOf(from))
        const noCheckedGrant = checkedPolicies?.every(
          (policy) =>
            policy.status !== "storage_unavailable" &&
            !hasSavedProjectPermission(group, policy, false),
        )
        publish({
          uploadActionError:
            oldOff && noCheckedGrant && !hasGrant(getSnapshot(), group)
              ? "Sync stopped for this folder. Choose a workspace to sync it again."
              : oldOff
                ? "Sync stopped in the old workspace. Check the saved folder before choosing a workspace."
                : removalUnconfirmed
                  ? removalUnknownMessage
                  : "Could not confirm where this folder syncs. Check the saved folder before choosing a workspace.",
        })
        return "clear_draft"
      }
      publish({ uploadActionError: removalUnknownMessage })
      return "clear_draft"
    } finally {
      setInFlight(false)
      if (sameGeneration()) publish({ busy: false })
    }
  }
}

/** Read only policies bound to the checked account and workspace. */
function checkedPolicy(
  status: CloudOngoingStatus | null | undefined,
  anchor: CloudDestinationBinding,
  workspace: string,
) {
  if (
    status?.accountId !== anchor.accountId ||
    status.destinationId !== anchor.destinationId ||
    cloudUploadStatusUnknown(status)
  )
    return
  return workspace === anchor.destinationId
    ? status
    : status.teamStatuses?.find(
        (team) => team.accountId === anchor.accountId && team.destinationId === workspace,
      )
}

/** Get the policy fields that can invalidate a pending folder move. */
function policyKey(policy: CloudOngoingStatus | null | undefined) {
  return policy
    ? JSON.stringify([
        policy.accountId,
        policy.destinationId,
        policy.consentVersion,
        policy.policyRevision,
        !!policy.folderConfirmationRequired,
        !!policy.sourceConfirmationRequired,
        policy.status === "storage_unavailable",
      ])
    : null
}

function workspaceVerified(view: CloudViewSnapshot, workspace: string, personal: string) {
  return view.projects?.workspaces?.some(
    ({ id, kind }) => id === workspace && kind === (id === personal ? "personal" : "team"),
  )
}

function groupCurrent(view: CloudViewSnapshot, groupId: string, confirmationKey: string) {
  return (
    view.cloud.captureEnabled === true &&
    view.projects?.loaded &&
    !view.projects.loading &&
    !view.projects.scopeInvalid &&
    !view.projects.error &&
    view.projects.groups.some(
      (item) =>
        item.id === groupId &&
        item.captureSupported &&
        !item.discoveryStale &&
        item.confirmationKey === confirmationKey,
    )
  )
}

function hasGrant(view: CloudViewSnapshot, group: DesktopCloudProjectGroup, except?: string) {
  return [view.ongoing, ...Object.values(view.teamStatuses ?? {})].some(
    (policy) =>
      !!policy &&
      policy.destinationId !== except &&
      hasSavedProjectPermission(group, policy, false),
  )
}

/** Confirm a repository by ID. Exact folder moves require the same provider and path pairs. */
function policyMatchesGroup(
  policy: CloudOngoingStatus,
  group: DesktopCloudProjectGroup,
  projects: CloudProjectSelection[],
) {
  if (group.repositoryWide) {
    const saved = savedRepositoryProjects(policy, group.id)
    return (
      projects.length > 0 &&
      saved.length === projects.length &&
      projects.every((project) =>
        saved.some(
          (anchor) =>
            anchor.sourceKind === project.sourceKind &&
            anchor.sourcePath === project.sourcePath,
        ),
      )
    )
  }
  const matching = savedExactProjects(policy).filter((project) =>
    hasSavedProjectPermission(group, { projects: [project] }, false),
  )
  return (
    projects.length > 0 &&
    matching.length === projects.length &&
    projects.every((project) => matching.some((saved) => sameSelection(project, saved)))
  )
}

function targetReady(
  policy: CloudOngoingStatus | null | undefined,
  binding: CloudDestinationBinding | null,
) {
  if (!policy) return true
  if (
    policy.accountId !== binding?.accountId ||
    policy.destinationId !== binding.destinationId ||
    policy.status === "storage_unavailable" ||
    policy.sourceConfirmationRequired ||
    policy.folderConfirmationRequired
  )
    return false
  return policy.consentVersion === "desktop-project-sync-v1"
    ? !!policy.policyRevision
    : !policy.consentVersion && !policy.projects.length && !policy.savedFolders?.length
}

function sameSelection(a: CloudProjectSelection, b: CloudProjectSelection) {
  return (
    a.sourceKind === b.sourceKind &&
    a.sourcePath === b.sourcePath &&
    a.projectPath === b.projectPath
  )
}

/** Compare complete provider bindings without relying on array order. */
function sameSelections(a: CloudProjectSelection[], b: CloudProjectSelection[]) {
  return (
    a.length === b.length && a.every((item) => b.some((other) => sameSelection(item, other)))
  )
}

/** A Team removal receipt includes saved rules even when upload counters are unavailable. */
function removalConfirmed(
  policy: CloudOngoingStatus | null | undefined,
  previous: CloudOngoingStatus,
  binding: CloudDestinationBinding,
  previousRevision: string,
  group: DesktopCloudProjectGroup,
  removed: CloudProjectSelection[],
) {
  const keptProjects = savedExactProjects(previous).filter(
    (project) => group.repositoryWide || !removed.includes(project),
  )
  const keptRepositories = (previous.repositoryGroupIds ?? []).filter(
    (id) => !group.repositoryWide || id !== group.id,
  )
  return (
    !!policy?.policyRevision &&
    policy.status !== "storage_unavailable" &&
    policy.policyRevision !== previousRevision &&
    matchesUploadPolicy(policy, binding, policy.policyRevision) &&
    policy.consentVersion === "desktop-project-sync-v1" &&
    policy.projectChoices?.[group.id] === false &&
    !hasSavedProjectPermission(group, policy, false) &&
    (policy.repositoryGroupIds?.length ?? 0) === keptRepositories.length &&
    keptRepositories.every(
      (id) =>
        policy.repositoryGroupIds?.includes(id) &&
        sameSelections(
          savedRepositoryProjects(previous, id),
          savedRepositoryProjects(policy, id),
        ),
    ) &&
    sameSelections(keptProjects, savedExactProjects(policy))
  )
}
