import type { CloudPort, CloudTeamJoinAction } from "../lib/cloud"
import { projectBinding, type CloudViewSnapshot } from "./cloudSessionState"

/** Bind account actions to the current view. Membership does not change sync consent. */
export function createCloudAccountActions(
  session: {
    getSnapshot(): CloudViewSnapshot
    readonly projectSyncSupported: boolean
    retryFolderScope(fresh?: boolean): Promise<unknown> | undefined
  },
  getPort: () => CloudPort,
) {
  return {
    openDashboard: () =>
      getPort().openDashboard?.() ??
      Promise.reject(new Error("The Cloud dashboard is unavailable.")),
    canJoinTeam: () => {
      const { cloud } = session.getSnapshot()
      return !!(
        getPort().teamJoin &&
        cloud.dashboardAvailable &&
        cloud.teamJoinAllowed &&
        projectBinding(cloud, true)
      )
    },
    joinTeam: async (action: CloudTeamJoinAction) => {
      const port = getPort()
      const { cloud: before } = session.getSnapshot()
      const binding = projectBinding(before, true)
      if (!binding || !port.teamJoin || !before.dashboardAvailable || !before.teamJoinAllowed)
        throw new Error("team_join_unavailable")
      const result = await port.teamJoin(binding, action)
      const { cloud: after } = session.getSnapshot()
      if (
        after.status !== "signed_in" ||
        !after.teamJoinAllowed ||
        after.identity?.id !== before.identity?.id ||
        after.grantExpiresAt !== before.grantExpiresAt
      )
        throw new Error("team_join_account_changed")
      if (result.status === "joined" && session.projectSyncSupported)
        void session.retryFolderScope(true)
      return result
    },
  }
}
