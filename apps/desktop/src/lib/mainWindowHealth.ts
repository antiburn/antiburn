import {
  mainWindowHealthAck,
  mainWindowPendingHealthCheck,
  onMainWindowHealthCheck,
  type MainWindowHealthCheckRequest,
} from "./ipc"
import { onRendererHealthSettled } from "./rendererHealth"

function rendererGeneration(): number | null {
  const value = window.__ANTIBURN_WINDOW_GENERATION__
  return typeof value === "number" && Number.isSafeInteger(value) ? value : null
}

/** Install the generation-scoped responder for hidden retained opens. */
export async function installMainWindowHealthResponder(): Promise<() => void> {
  const generation = rendererGeneration()
  if (generation === null) return () => undefined

  let active: { key: string; stop: () => void } | null = null
  let disposed = false

  const answer = (request: MainWindowHealthCheckRequest): void => {
    if (disposed || request.generation !== generation) return
    const key = `${request.generation}:${request.requestId}`
    if (active?.key === key) return
    active?.stop()
    active = { key, stop: () => undefined }
    const stop = onRendererHealthSettled((health) => {
      if (disposed || active?.key !== key) return
      active = null
      void mainWindowHealthAck(
        request.requestId,
        request.generation,
        health === "healthy",
      ).catch(() => undefined)
    })
    if (active?.key === key) active.stop = stop
    else stop()
  }

  const unlisten = await onMainWindowHealthCheck(answer)
  if (disposed) {
    unlisten()
    return () => undefined
  }
  let pending: MainWindowHealthCheckRequest | null
  try {
    pending = await mainWindowPendingHealthCheck(generation)
  } catch (error) {
    disposed = true
    unlisten()
    throw error
  }
  if (pending) answer(pending)

  return () => {
    if (disposed) return
    disposed = true
    active?.stop()
    active = null
    unlisten()
  }
}
