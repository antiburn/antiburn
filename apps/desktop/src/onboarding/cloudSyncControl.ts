import type { CloudOngoingStatus, CloudPort } from "../lib/cloud"
import { projectBinding, uploadActionError, type CloudViewSnapshot } from "./cloudSessionState"

type SyncControlResult = "requested" | "stopped" | { error: string }

/** Serialize upload controls for one Cloud view. Pause can fence an active scan. */
export function createSyncControl({
  getPort,
  getSnapshot,
  generation,
  policyRevision,
  publish,
  receive,
}: {
  getPort: () => CloudPort["ongoing"]
  getSnapshot: () => CloudViewSnapshot
  generation: () => number | undefined
  policyRevision: () => number
  publish: (patch: Partial<CloudViewSnapshot>) => void
  receive: (status: CloudOngoingStatus) => void
}) {
  let pending: { generation: number; count: number; pauseRequested: boolean } | undefined
  return async (
    action: (port: NonNullable<CloudPort["ongoing"]>) => Promise<CloudOngoingStatus | void>,
    {
      team,
      backfillCurrent,
      pause,
    }: {
      team?: CloudOngoingStatus | undefined
      backfillCurrent?: () => boolean
      pause?: boolean
    } = {},
  ): Promise<SyncControlResult> => {
    const port = getPort()
    const started = generation()
    const view = getSnapshot()
    const pausing =
      pause && view.canPauseBackfill && pending?.generation === started ? pending : undefined
    if (
      started === undefined ||
      !port ||
      ((view.busy || pending?.generation === started) && !pausing)
    )
      return "stopped"
    const operation = pausing ?? { generation: started, count: 0, pauseRequested: false }
    pending = operation
    operation.count++
    if (pausing) operation.pauseRequested = true
    const revision = policyRevision()
    const current = () =>
      started === generation() &&
      (!backfillCurrent || (!operation.pauseRequested && backfillCurrent())) &&
      (!team || team.accountId === projectBinding(getSnapshot().cloud, true)?.accountId)
    publish({ busy: true, canPauseBackfill: !!backfillCurrent, uploadActionError: null })
    try {
      const status = await action(port)
      if (
        status &&
        current() &&
        (team
          ? !!team.destinationId &&
            status.destinationId === team.destinationId &&
            status.accountId === team.accountId &&
            getSnapshot().teamStatuses?.[team.destinationId] === team
          : revision === policyRevision())
      )
        receive(status)
      return current() ? "requested" : "stopped"
    } catch (error) {
      if (!current()) return "stopped"
      const message = uploadActionError(error, !!backfillCurrent, !!team)
      publish({ uploadActionError: message })
      return { error: message }
    } finally {
      operation.count--
      if (!operation.count && pending === operation) {
        pending = undefined
        if (started === generation()) publish({ busy: false, canPauseBackfill: false })
      }
    }
  }
}
