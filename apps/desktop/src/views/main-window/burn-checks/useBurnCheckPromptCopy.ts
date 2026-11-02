import { useCallback, useRef, useState } from "react"

import { writeClipboardText } from "../../../lib/clipboard"
import { noteInteraction, type PromptPreparationAnalyticsOutcome } from "../../../lib/ipc"
import type {
  CopyPromptFixBurnCheckOutcome,
  CopyPromptFixBurnCheckTargetOutcome,
} from "../../../lib/insightsIpc"

type PromptOutcome = CopyPromptFixBurnCheckOutcome | CopyPromptFixBurnCheckTargetOutcome | null
type CopyFeedback = { busy: boolean; copied: boolean; status: string | null }

const INITIAL_FEEDBACK: CopyFeedback = { busy: false, copied: false, status: null }

export function useBurnCheckPromptCopy({
  subject,
  active,
  prepare,
  refresh,
  onCopied,
}: {
  subject: string
  active: boolean
  prepare: () => Promise<PromptOutcome>
  refresh: () => void
  onCopied?: () => void
}) {
  const [feedback, setFeedback] = useState(INITIAL_FEEDBACK)
  const currentSubject = useRef<string | null>(null)
  const owner = useRef<{ pending: boolean } | null>(null)
  const copiedTimeout = useRef<ReturnType<typeof setTimeout> | null>(null)
  const bindRoot = useCallback(
    (node: HTMLElement | null) => {
      currentSubject.current = node && active ? subject : null
      owner.current = null
      if (copiedTimeout.current) clearTimeout(copiedTimeout.current)
      copiedTimeout.current = null
      if (node) setFeedback(INITIAL_FEEDBACK)
    },
    [active, subject],
  )

  const copy = async () => {
    if (currentSubject.current !== subject || owner.current?.pending) return
    const request = { pending: true }
    owner.current = request
    if (copiedTimeout.current) clearTimeout(copiedTimeout.current)
    setFeedback({ busy: true, copied: false, status: null })
    const isCurrent = () => owner.current === request && currentSubject.current === subject
    const reportPrepared = (outcome: PromptPreparationAnalyticsOutcome) =>
      noteInteraction({
        kind: "burnCheckPromptPrepared",
        outcome,
      })
    const fail = (status: string) => {
      if (!isCurrent()) return
      request.pending = false
      setFeedback({ busy: false, copied: false, status })
    }

    let outcome: PromptOutcome
    try {
      outcome = await prepare()
    } catch {
      reportPrepared("failed")
      fail("The fix prompt could not be prepared. Try again.")
      return
    }
    reportPrepared(
      outcome?.outcome === "promptReady" ? "ready" : (outcome?.outcome ?? "failed"),
    )
    if (!isCurrent()) return
    if (outcome?.outcome !== "promptReady") {
      const stale = outcome?.outcome === "expired" || outcome?.outcome === "stale"
      fail(
        stale
          ? "Checking the current change."
          : outcome?.outcome === "unavailable"
            ? "A fix prompt is not available for this finding."
            : "The fix prompt could not be prepared. Try again.",
      )
      if (stale) refresh()
      return
    }

    if (!isCurrent()) return
    try {
      await writeClipboardText(outcome.prompt)
    } catch {
      fail("The fix prompt could not be copied. Try again.")
      return
    }
    noteInteraction({ kind: "burnCheckPromptCopied" })
    if (!isCurrent()) return
    request.pending = false
    setFeedback({ busy: false, copied: true, status: null })
    copiedTimeout.current = setTimeout(() => {
      copiedTimeout.current = null
      if (isCurrent()) setFeedback(INITIAL_FEEDBACK)
    }, 3_000)
    onCopied?.()
  }

  return { ...feedback, bindRoot, copy }
}
