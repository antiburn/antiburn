import { invoke, isTauri } from "@tauri-apps/api/core"

type Request<T> = {
  value: T
  promise: Promise<void>
  resolve: () => void
  reject: (error: unknown) => void
}

/** Keep one native request active and replace pending measurements with the newest value. */
function latestSize<T>(apply: (value: T) => Promise<void>): (value: T) => Promise<void> {
  let running = false
  let pending: Request<T> | null = null

  async function drain(): Promise<void> {
    running = true
    while (pending) {
      const request = pending
      pending = null
      try {
        await apply(request.value)
        request.resolve()
      } catch (error) {
        request.reject(error)
      }
    }
    running = false
  }

  return (value) => {
    if (!isTauri()) return Promise.resolve()
    if (pending) {
      pending.value = value
      return pending.promise
    }
    let resolve!: () => void
    let reject!: (error: unknown) => void
    const promise = new Promise<void>((accept, fail) => {
      resolve = accept
      reject = fail
    })
    pending = { value, promise, resolve, reject }
    if (!running) void drain()
    return promise
  }
}

const resizeHud = latestSize(
  (size: {
    height: number
    anchorBottom: boolean
    animate: boolean
    geometryRevision?: number
  }) => invoke<void>("resize_overlay_window", size),
)
const resizeDetail = latestSize((height: number) =>
  invoke<void>("set_hud_detail_size", { height }),
)

/** Resize the HUD without letting older measurements overwrite newer ones. */
export function resizeOverlayWindow(
  height: number,
  anchorBottom: boolean,
  animate: boolean,
  geometryRevision?: number,
): Promise<void> {
  return resizeHud({
    height,
    anchorBottom,
    animate,
    ...(geometryRevision === undefined ? {} : { geometryRevision }),
  })
}

/** Size the detail window in measurement order, with only the newest request pending. */
export function setHudDetailSize(height: number): Promise<void> {
  return resizeDetail(height)
}
