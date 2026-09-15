import { Check, Clipboard, Wrench } from "lucide-react"
import { useCallback, useRef, useState } from "react"

import { cn } from "../../../lib/cn"
import { Tooltip } from "../../../components/presentation/Tooltip"
import { noteInteraction } from "../../../lib/ipc"
import { writeClipboardText } from "../../../lib/clipboard"
import {
  copyPromptFixBurnCheckTargets,
  copyPromptFixBurnCheck,
  type BurnCheckDetectorId,
  type BurnCheckTargetPayload,
} from "../../../lib/insightsIpc"
import { BurnCheckTargetActions } from "./BurnCheckTargetActions"
import { RemindLaterAction } from "./RemindLaterAction"
import { SampleSessions, watchStatus } from "./BurnCheckTargetPresentation"
import { BurnCheckTargetChooserDialog } from "./BurnCheckTargetChooserDialog"

export const CHECK_SENTENCES: Record<BurnCheckDetectorId, string> = {
  sessionsOverDepth: "Some sessions carried context after it stopped helping.",
  modelOverthinking: "Some work used more reasoning than it needed.",
  overpoweredSubagents: "Some helper work used more model power than it needed.",
  unusedMcpServers: "Some MCP servers were loaded but not used.",
  unusedBuiltInTools: "Some built-in tools were loaded but not used.",
  unusedSkills: "Some skills were loaded but not used.",
  oldModelUsage: "Some sessions used an older model when a newer one was available.",
  overuseOfFastMode: "Some work paid for speed it did not need.",
  cacheChurn: "Some sessions kept paying to reload the same context.",
}

function Samples({ targets }: { targets: BurnCheckTargetPayload[] }) {
  const samples = Array.from(
    new Map(
      targets
        .flatMap((target) => target.samples)
        .map((sample) => [sample.navigationHandle, sample]),
    ).values(),
  ).slice(0, 3)
  return <SampleSessions samples={samples} />
}

export function CheckPromptAction({
  detector,
  targets,
  refresh,
  comingSoon = false,
}: {
  detector: BurnCheckDetectorId
  targets: BurnCheckTargetPayload[]
  refresh: () => void
  comingSoon?: boolean
}) {
  const [busy, setBusy] = useState(false)
  const [copied, setCopied] = useState(false)
  const [prompt, setPrompt] = useState<string | null>(null)
  const [status, setStatus] = useState<string | null>(null)
  const promptTargets = targets.filter((target) => target.promptFix.status === "available")
  const currentKey =
    targets.length === 0
      ? `fallback:${detector}`
      : promptTargets.map((target) => target.actionId).join(":")
  const key = useRef("")
  const copiedTimeout = useRef<ReturnType<typeof setTimeout> | null>(null)
  const scheduleCopiedReset = (startedKey: string) => {
    if (copiedTimeout.current) clearTimeout(copiedTimeout.current)
    copiedTimeout.current = setTimeout(() => {
      copiedTimeout.current = null
      if (key.current !== startedKey) return
      setCopied(false)
    }, 3_000)
  }
  const bindKey = useCallback(
    (node: HTMLDivElement | null) => {
      if (node) key.current = currentKey
      else {
        key.current = ""
        if (copiedTimeout.current) clearTimeout(copiedTimeout.current)
      }
    },
    [currentKey],
  )
  const copy = async () => {
    if (busy || copied) return
    const startedKey = currentKey
    setBusy(true)
    setStatus(null)
    let nextPrompt = prompt
    try {
      if (nextPrompt === null) {
        const outcome =
          targets.length === 0
            ? await copyPromptFixBurnCheck(detector)
            : await copyPromptFixBurnCheckTargets(
                promptTargets.map((target) => target.actionId),
              )
        noteInteraction({
          kind: "burnCheckPromptPrepared",
          outcome:
            outcome?.outcome === "promptReady"
              ? "ready"
              : outcome?.outcome === "unavailable"
                ? "unavailable"
                : "failed",
        })
        if (key.current !== startedKey) return
        if (outcome?.outcome !== "promptReady") {
          setBusy(false)
          setStatus("Checking the current change.")
          refresh()
          return
        }
        nextPrompt = outcome.prompt
      }
      await writeClipboardText(nextPrompt)
      if (key.current !== startedKey) return
      noteInteraction({ kind: "burnCheckPromptCopied" })
      setPrompt(nextPrompt)
      setCopied(true)
      setBusy(false)
      scheduleCopiedReset(startedKey)
    } catch {
      const preparationFailed = nextPrompt === null
      if (preparationFailed)
        noteInteraction({ kind: "burnCheckPromptPrepared", outcome: "failed" })
      if (key.current !== startedKey) return
      setPrompt(nextPrompt)
      setBusy(false)
      setStatus(
        preparationFailed
          ? "Could not prepare the prompt. Try again."
          : "Could not copy the prompt. Try again.",
      )
    }
  }
  if (targets.length > 0 && promptTargets.length === 0) return null
  if (comingSoon) {
    return (
      <Tooltip label="Coming soon" side="bottom">
        <button
          type="button"
          aria-disabled="true"
          className="burn-check-action type-callout gap-1"
        >
          <Clipboard size={12} aria-hidden="true" />
          Copy fix prompt
        </button>
      </Tooltip>
    )
  }
  return (
    <div ref={bindKey}>
      <button
        type="button"
        disabled={busy || copied}
        onClick={() => void copy()}
        className="burn-check-action type-callout gap-1 disabled:opacity-100"
      >
        {copied ? (
          <Check size={12} className="text-token-in" aria-hidden="true" />
        ) : (
          <Clipboard size={12} aria-hidden="true" />
        )}
        {copied ? "Copied" : "Copy fix prompt"}
      </button>
      {status && (
        <p role="alert" className="mt-3 type-callout text-label-secondary">
          {status}
        </p>
      )}
    </div>
  )
}

function FixAction({
  targets,
  refresh,
}: {
  targets: BurnCheckTargetPayload[]
  refresh: () => void
}) {
  const [choosing, setChoosing] = useState(false)
  const eligible = targets.filter((target) => target.autoFix.status === "available")
  if (eligible.length === 0) return null
  if (eligible.length === 1)
    return (
      <BurnCheckTargetActions
        target={eligible[0]!}
        refresh={refresh}
        showPromptFix={false}
        embedded
      />
    )
  return (
    <div>
      <button
        type="button"
        onClick={() => setChoosing(true)}
        className="burn-check-action type-callout gap-1"
      >
        <Wrench size={12} aria-hidden="true" />
        Fix
      </button>
      {choosing && (
        <BurnCheckTargetChooserDialog
          targets={eligible}
          refresh={refresh}
          close={() => setChoosing(false)}
        />
      )}
    </div>
  )
}

export function CheckDetailActions({
  detector,
  targets,
  refresh,
  reportRow = false,
  snoozed = false,
}: {
  detector: BurnCheckDetectorId
  targets: BurnCheckTargetPayload[]
  refresh: () => void
  reportRow?: boolean
  snoozed?: boolean
}) {
  return (
    <div className="flex flex-wrap items-start gap-2">
      {reportRow && !snoozed && <RemindLaterAction detector={detector} />}
      <CheckPromptAction
        comingSoon={reportRow}
        key={targets.map((target) => target.actionId).join(":")}
        detector={detector}
        targets={targets}
        refresh={refresh}
      />
      <FixAction targets={targets} refresh={refresh} />
    </div>
  )
}

export function BurnCheckDetail({
  detector,
  targets,
  refresh,
  contained = false,
  reportRow = false,
}: {
  detector: BurnCheckDetectorId
  targets: BurnCheckTargetPayload[]
  refresh: () => void
  contained?: boolean
  reportRow?: boolean
}) {
  const statuses = Array.from(
    new Set(targets.map(watchStatus).filter((status): status is string => status !== null)),
  )
  return (
    <article
      className={cn(
        "min-w-0",
        contained && !reportRow && "max-w-3xl rounded-control bg-surface-card/75 p-4",
      )}
    >
      {!reportRow && (
        <div>
          <p className="type-body text-pretty text-label-secondary">
            {CHECK_SENTENCES[detector]}
          </p>
          <div className="mt-2">
            <CheckDetailActions detector={detector} targets={targets} refresh={refresh} />
          </div>
        </div>
      )}
      {statuses.length === 1 && (
        <p role="status" className="mt-3 type-callout text-label-secondary">
          {statuses[0]}
        </p>
      )}
      <Samples targets={targets} />
    </article>
  )
}
