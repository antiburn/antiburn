import type {
  CloudDestinationBinding,
  CloudOngoingConsent,
  CloudProjectChoices,
  CloudProjectGroup,
  CloudProjectSelection,
  CloudSourceSelection,
  CloudWorkspace,
} from "../lib/cloud"
import {
  cloudUploadStatusUnknown,
  teamFoldersToConfirm,
  type CloudViewSnapshot,
} from "./cloudSessionState"

interface ProjectFolderRoute {
  id: string
  groups: CloudProjectGroup[]
  workspace: CloudWorkspace | undefined
}

function availableWorkspaces(snapshot: CloudViewSnapshot) {
  return (
    snapshot.projects?.workspaces ??
    (snapshot.cloud.destination ? [snapshot.cloud.destination] : [])
  )
}

/** Reject the whole selection if a destination has no workspace or no folders. */
export function projectFolderRoutes(
  snapshot: CloudViewSnapshot,
  groups: CloudProjectGroup[],
  destinations: string[],
  destinationFor: (id: string) => string,
): ProjectFolderRoute[] | undefined {
  const routes = destinations.map((id) => ({
    id,
    groups: groups.filter((group) => destinationFor(group.id) === id),
    workspace: availableWorkspaces(snapshot).find((item) => item.id === id),
  }))
  if (routes.length && routes.every((route) => route.workspace && route.groups.length))
    return routes
}

/** Recheck the displayed folders and workspace before sending each route. */
export function isCurrentProjectFolderRoute(
  snapshot: CloudViewSnapshot,
  { id, groups, workspace }: ProjectFolderRoute,
  destinationFor: (id: string) => string,
  hasSavedProject: (id: string) => boolean,
) {
  const projects = snapshot.projects
  return (
    projects?.loaded &&
    !projects.loading &&
    !projects.error &&
    !cloudUploadStatusUnknown(snapshot.ongoing) &&
    availableWorkspaces(snapshot).some(
      (item) => item.id === id && item.kind === workspace?.kind,
    ) &&
    !teamFoldersToConfirm(snapshot, id) &&
    groups.every(
      (group) =>
        destinationFor(group.id) === id &&
        !hasSavedProject(group.id) &&
        projects.groups.some(
          (item) => item.id === group.id && item.confirmationKey === group.confirmationKey,
        ),
    )
  )
}

/** Send source bindings and folder choices for native validation. Exclude attachments. */
export function projectSyncConsent(
  target: CloudDestinationBinding,
  sources: CloudSourceSelection[],
  projects: CloudProjectSelection[],
  projectChoices: CloudProjectChoices,
): CloudOngoingConsent {
  return {
    ...target,
    version: "desktop-project-sync-v1",
    selectedSources: sources.filter((source) =>
      projects.some(
        (project) =>
          project.sourceKind === source.sourceKind && project.sourcePath === source.path,
      ),
    ),
    projects,
    projectChoices,
    fullContent: true,
    ongoing: true,
    attachments: false,
  }
}
