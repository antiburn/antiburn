/**
 * Asking the reader for protected folders, one at a time.
 *
 * The operating system prompts per folder, so the flow is a queue rather than a
 * single question: ask, wait for the answer, pause long enough that the next
 * dialog is legibly a *new* question, then ask again. Firing them together
 * would stack system dialogs on top of each other with no way to tell which
 * folder each one is for.
 *
 * Two states deserve their own handling rather than collapsing into "denied":
 *
 * - **recorded-denial** — the system refused from a decision it already held,
 *   without showing anything. Asking again is futile; the only way out is
 *   system settings. This is tracked per folder, so one stuck folder never
 *   hides the working control for the others.
 * - **error** — the request never reached the system. Retrying is reasonable.
 */

import { useCallback, useRef, useState } from "react"

import { requestFolderAccess } from "./ipc"
import type { DeferredPermissionDir } from "./types/repository"
import { useExternalSubscription } from "./useExternalSubscription"

/** Where the flow is. */
export type FlowPhase =
  | "idle"
  /** A system dialog is up, or about to be. */
  | "asking"
  /** The reader answered; the pass that follows is re-reading the folder. */
  | "settling"
  | "done"

/** How one folder's request ended. */
export type FolderVerdict = "granted" | "denied" | "recorded-denial" | "error"

/**
 * How long to wait between folders.
 *
 * Long enough that a second dialog reads as a new question rather than a
 * flicker of the first, short enough not to feel stalled.
 */
export const BETWEEN_FOLDERS_MS = 800

export type FolderPermissionFlow = {
  phase: FlowPhase
  /** The folder currently being asked about, if any. */
  current: string | null
  /** How each folder answered so far. */
  verdicts: Record<string, FolderVerdict>
  /** Folders the system refuses to re-prompt for. */
  recordedDenials: string[]
  /** 1-based position of the current folder, for "2 of 3". */
  position: number
  /** How many folders the flow set out to ask about. */
  total: number
  /** Begin asking. Ignored while a run is in flight. */
  start: () => void
  /** Abandon the run and clear its state. */
  reset: () => void
}

/**
 * Drive a sequential permission request over `deferred`.
 *
 * `onGranted` fires once per granted folder, so the caller can refresh the list
 * the reader is watching without waiting for the whole queue.
 */
export function useFolderPermissionFlow(
  deferred: DeferredPermissionDir[],
  onGranted?: (dir: string) => void,
): FolderPermissionFlow {
  const [phase, setPhase] = useState<FlowPhase>("idle")
  const [current, setCurrent] = useState<string | null>(null)
  const [verdicts, setVerdicts] = useState<Record<string, FolderVerdict>>({})
  const [position, setPosition] = useState(0)
  const [total, setTotal] = useState(0)

  // A run owns its queue: folders discovered by a refresh mid-run must not
  // lengthen the queue under the reader.
  const queue = useRef<string[]>([])
  const running = useRef(false)
  const cancelled = useRef(false)
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)
  // Held in a ref so a caller passing a fresh closure every render does not
  // restart the queue. Written directly in the render body (the "latest ref"
  // pattern), not through an effect: it is only ever read inside the async
  // `step()` below, never during render, so there is no tearing risk.
  const onGrantedRef = useRef(onGranted)
  // Deliberate render-body ref write (the "latest ref" pattern): only ever
  // read inside `step()`, an async callback, never during render, so there is
  // no tearing risk.
  // eslint-disable-next-line react-hooks/refs
  onGrantedRef.current = onGranted

  const clearTimer = useCallback(() => {
    if (timer.current !== null) {
      clearTimeout(timer.current)
      timer.current = null
    }
  }, [])

  // A component unmounting mid-flow must not leave a timer that resumes into a
  // dead tree, and must not keep answering for a reader who has moved on.
  useExternalSubscription(
    useCallback(
      () => () => {
        cancelled.current = true
        clearTimer()
      },
      [clearTimer],
    ),
  )

  const reset = useCallback(() => {
    cancelled.current = true
    clearTimer()
    running.current = false
    queue.current = []
    setPhase("idle")
    setCurrent(null)
    setVerdicts({})
    setPosition(0)
    setTotal(0)
  }, [clearTimer])

  const start = useCallback(() => {
    if (running.current) return

    // Folders the system already refuses to re-prompt for are skipped: asking
    // again shows nothing and would read as the button being broken.
    const pending = deferred
      .map((entry) => entry.dir)
      .filter((dir) => verdicts[dir] !== "recorded-denial" && verdicts[dir] !== "granted")
    if (pending.length === 0) return

    cancelled.current = false
    running.current = true
    queue.current = pending
    setTotal(pending.length)
    setPosition(0)

    const step = async (index: number): Promise<void> => {
      if (cancelled.current) return
      const dir = queue.current[index]
      if (dir === undefined) {
        running.current = false
        setPhase("done")
        setCurrent(null)
        return
      }

      setPhase("asking")
      setCurrent(dir)
      setPosition(index + 1)

      let verdict: FolderVerdict
      try {
        const outcome = await requestFolderAccess(dir)
        verdict = outcome.outcome
      } catch {
        verdict = "error"
      }
      if (cancelled.current) return

      setVerdicts((previous) => ({ ...previous, [dir]: verdict }))
      if (verdict === "granted") {
        // The shell has already asked for a rescan; this lets the surface the
        // reader is looking at refresh without waiting for the whole queue.
        setPhase("settling")
        onGrantedRef.current?.(dir)
      }

      timer.current = setTimeout(() => {
        timer.current = null
        void step(index + 1)
      }, BETWEEN_FOLDERS_MS)
    }

    void step(0)
  }, [deferred, verdicts])

  const recordedDenials = Object.entries(verdicts)
    .filter(([, verdict]) => verdict === "recorded-denial")
    .map(([dir]) => dir)

  return {
    phase,
    current,
    verdicts,
    recordedDenials,
    position,
    total,
    start,
    reset,
  }
}
