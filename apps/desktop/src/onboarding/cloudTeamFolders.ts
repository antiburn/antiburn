import {
  CLOUD_BUSY_MESSAGE,
  type CloudOngoingStatus,
  type CloudPort,
  type CloudProjectChoices,
  type CloudProjectSelection,
} from "../lib/cloud"
import type { ProjectPages } from "./cloudProjectPages"
import {
  confirmsSavedWrite,
  expectedFolderRoute,
  isCloudBusy,
  projectBinding,
  projectConfirmationKey,
  savedFolderRoute,
  supportedProjectSelections,
  teamFoldersToConfirm,
  type CloudViewSnapshot,
} from "./cloudSessionState"

interface TeamFolderSession {
  getPort: () => CloudPort["ongoing"]
  getSnapshot: () => CloudViewSnapshot
  getDraft: () => {
    pages: ProjectPages | undefined
    choices: CloudProjectChoices
    targets: Readonly<Record<string, string>>
  }
  generation: () => number | undefined
  authority: () => number
  canStart: (context: CloudViewSnapshot) => boolean
  canUseChoices: () => boolean
  hasSavedProject: (id: string) => boolean
  finishRead: () => Promise<void> | undefined
  publish: (patch: Partial<CloudViewSnapshot>) => void
  receive: (status: CloudOngoingStatus) => void
  clearCommittedChoices: (ids: string[], draft: CloudProjectChoices) => void
}

/** Renew the exact saved Team selection and add only explicit same-Team choices. */
export function createTeamFolderConfirmation(session: TeamFolderSession) {
  return async (context: CloudViewSnapshot, destinationId: string) => {
    const snapshot = session.getSnapshot()
    const binding = projectBinding(snapshot.cloud, false)
    const displayed = teamFoldersToConfirm(context, destinationId)
    const current = teamFoldersToConfirm(snapshot, destinationId)
    const port = session.getPort()
    if (
      !binding ||
      binding.destinationId === destinationId ||
      !displayed ||
      displayed.revision !== current?.revision ||
      displayed.paths.length !== current.paths.length ||
      displayed.paths.some((path, index) => path !== current.paths[index]) ||
      !port?.confirmTeamFolders ||
      snapshot.busy ||
      !session.canStart(context)
    )
      return

    const { pages, choices, targets } = session.getDraft()
    const additions: Record<string, CloudProjectSelection[]> = {}
    const expected = savedFolderRoute(context.teamStatuses?.[destinationId])
    for (const [id, target] of Object.entries(targets)) {
      const group = pages?.groups.find((item) => item.id === id)
      if (
        target !== destinationId ||
        session.hasSavedProject(id) ||
        !group ||
        !session.canUseChoices()
      )
        return
      const projects = supportedProjectSelections(group, pages?.sources ?? [])
      if (
        !projects.length ||
        context.projects?.groups.find((item) => item.id === id)?.confirmationKey !==
          projectConfirmationKey(group, projects)
      )
        return
      additions[id] = projects
      expected.push(...expectedFolderRoute([group], projects))
    }

    const addedGroups = Object.keys(additions)
    const generation = session.generation()
    const authority = session.authority()
    const stillBound = () => {
      const anchor = projectBinding(session.getSnapshot().cloud, false)
      return (
        generation === session.generation() &&
        authority === session.authority() &&
        anchor?.accountId === binding.accountId &&
        anchor?.destinationId === binding.destinationId
      )
    }
    session.publish({ busy: true, uploadActionError: null })
    try {
      const read = addedGroups.length ? session.finishRead() : undefined
      if (read) await read
      if (
        !stillBound() ||
        (addedGroups.length && (!session.canStart(context) || !session.canUseChoices())) ||
        teamFoldersToConfirm(session.getSnapshot(), destinationId)?.revision !==
          displayed.revision
      )
        return
      const target = { ...binding, destinationId }
      const status = await port.confirmTeamFolders({
        ...target,
        expectedPolicyRevision: displayed.revision,
        folderPaths: displayed.paths,
        ...(addedGroups.length ? { additions } : {}),
      })
      if (
        stillBound() &&
        status.accountId === binding.accountId &&
        status.destinationId === destinationId
      ) {
        if (
          session.getSnapshot().teamStatuses?.[destinationId]?.policyRevision ===
          displayed.revision
        )
          session.receive(status)
        const confirmed = confirmsSavedWrite(
          session.getSnapshot().teamStatuses?.[destinationId],
          status,
          target,
          expected,
        )
        if (confirmed && addedGroups.length) session.clearCommittedChoices(addedGroups, choices)
        return confirmed
      }
    } catch (error) {
      if (stillBound())
        session.publish({
          uploadActionError: isCloudBusy(error)
            ? CLOUD_BUSY_MESSAGE
            : "Could not save folders. Try again.",
        })
    } finally {
      if (generation === session.generation()) session.publish({ busy: false })
    }
  }
}
