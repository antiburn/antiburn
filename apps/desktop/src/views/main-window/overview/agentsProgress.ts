import { getMainWindowVisible, onMainWindowVisibilityChanged } from "../../../lib/mainWindowIpc"
import { SurfaceExposureTracker } from "../../../lib/surfaceExposure"
import { overviewProgress, subscribeOverviewProgress } from "./overviewProgressStore"

export function subscribeAgentsProgress(listener: () => void): () => void {
  const exposure = new SurfaceExposureTracker()
  let disposed = false
  let visible = false
  let revision = 0
  let stopVisibility: (() => void) | undefined
  function sync(): void {
    if (disposed) return
    if (visible) {
      const progress = overviewProgress()
      exposure.expose({
        surface: "agents",
        origin: "user",
        state: progress.agents.rows.some((row) => row.sessions > 0)
          ? "ready"
          : progress.agents.done
            ? "empty"
            : null,
      })
    } else exposure.conceal()
    listener()
  }
  const stopProgress = subscribeOverviewProgress(sync)
  void onMainWindowVisibilityChanged((next) => {
    revision += 1
    visible = next
    sync()
  })
    .then((stop) => {
      if (disposed) stop()
      else stopVisibility = stop
    })
    .catch(() => undefined)
  const requestedAt = revision
  void getMainWindowVisible()
    .then((next) => {
      if (disposed || requestedAt !== revision) return
      visible = next
      sync()
    })
    .catch(() => undefined)
  return () => {
    disposed = true
    stopProgress()
    stopVisibility?.()
    exposure.conceal()
  }
}
