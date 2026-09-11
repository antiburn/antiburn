import { Check, ChevronDown, Clipboard, Wrench } from "lucide-react"
import { useCallback, useId, useRef, useState } from "react"

import { cn } from "../../../lib/cn"
import { noteInteraction } from "../../../lib/ipc"
import {
  copyPromptFixBurnCheckTargets,
  copyPromptFixBurnCheck,
  openBurnCheckSample,
  type BurnCheckDetectorId,
  type BurnCheckTargetPayload,
} from "../../../lib/insightsIpc"
import { SessionSampleRow } from "../../../components/session/SessionSampleRow"
import { BurnCheckTargetActions } from "./BurnCheckTargetActions"
import { watchStatus } from "./BurnCheckTargetPresentation"

const CHECK_SENTENCES: Record<BurnCheckDetectorId, string> = {
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
  const [open, setOpen] = useState(false)
  const [status, setStatus] = useState<string | null>(null)
  const [busyHandle, setBusyHandle] = useState<string | null>(null)
  const id = useId()
  const samples = Array.from(
    new Map(
      targets
        .flatMap((target) => target.samples)
        .map((sample) => [sample.navigationHandle, sample]),
    ).values(),
  ).slice(0, 3)
  if (samples.length === 0) return null
  return (
    <div className="mt-3">
      <button
        type="button"
        aria-expanded={open}
        aria-controls={id}
        onClick={() => setOpen((value) => !value)}
        className="inline-flex items-center gap-1.5 rounded-control py-1 type-footnote text-label-tertiary hover:text-label-secondary active:transform-none active:opacity-100"
      >
        Sample sessions ({samples.length})
        <ChevronDown
          size={14}
          className={cn(
            "transition-transform duration-[var(--duration-fast)]",
            open && "rotate-180",
          )}
          aria-hidden="true"
        />
      </button>
      <div id={id} hidden={!open} className="mt-1 space-y-1">
        {samples.map((sample) => (
          <SessionSampleRow
            key={sample.navigationHandle}
            title={sample.title}
            agent={sample.agent}
            surface={sample.surface}
            observedAtMs={sample.observedAtMs}
            busy={busyHandle !== null}
            onOpen={async () => {
              if (busyHandle) return
              setBusyHandle(sample.navigationHandle)
              setStatus(null)
              try {
                const result = await openBurnCheckSample(sample.navigationHandle)
                if (result?.outcome === "opened") return
                setStatus(
                  result?.outcome === "deleted"
                    ? "This sample session was deleted."
                    : result?.outcome === "expired"
                      ? "This sample session is no longer available."
                      : "This sample session is unavailable.",
                )
              } catch {
                setStatus("Could not open this sample session. Try again.")
              } finally {
                setBusyHandle(null)
              }
            }}
          />
        ))}
      </div>
      {status && (
        <p role="status" className="mt-2 type-callout text-system-red-text">
          {status}
        </p>
      )}
    </div>
  )
}

function CheckPromptAction({
  detector,
  targets,
  refresh,
}: {
  detector: BurnCheckDetectorId
  targets: BurnCheckTargetPayload[]
  refresh: () => void
}) {
  const [busy, setBusy] = useState(false)
  const [copied, setCopied] = useState(false)
  const [prompt, setPrompt] = useState<string | null>(null)
  const [status, setStatus] = useState<string | null>(null)
  const promptTargets = targets.filter((target) => target.promptFix.status === "available")
  const currentKey =
    promptTargets.length === 0
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
      if (!nextPrompt) {
        const outcome =
          promptTargets.length === 0
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
      await navigator.clipboard.writeText(nextPrompt)
      if (key.current !== startedKey) return
      noteInteraction({ kind: "burnCheckPromptCopied" })
      setPrompt(nextPrompt)
      setCopied(true)
      setBusy(false)
      scheduleCopiedReset(startedKey)
    } catch {
      if (!nextPrompt) noteInteraction({ kind: "burnCheckPromptPrepared", outcome: "failed" })
      if (key.current !== startedKey) return
      setPrompt(nextPrompt)
      setBusy(false)
      setStatus("Could not copy the prompt. Check clipboard access and try again.")
    }
  }
  return (
    <div ref={bindKey}>
      <button
        type="button"
        disabled={busy || copied}
        onClick={() => void copy()}
        className="ui-push-button gap-1.5 disabled:opacity-100"
      >
        {copied ? (
          <Check size={12} className="text-system-green" aria-hidden="true" />
        ) : (
          <Clipboard size={12} aria-hidden="true" />
        )}
        {copied ? "Copied" : "Copy fix prompt"}
      </button>
      {status && (
        <p role="alert" className="mt-3 type-callout text-system-red-text">
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
  const [selectedFindingId, setSelectedFindingId] = useState<string | null>(null)
  const eligible = targets.filter((target) => target.autoFix.status === "available")
  const selected = eligible.find((target) => target.findingId === selectedFindingId) ?? null
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
    <div className="mt-3">
      {selected ? (
        <>
          <BurnCheckTargetActions
            target={selected}
            refresh={refresh}
            showPromptFix={false}
            embedded
          />
          <button
            type="button"
            onClick={() => {
              setSelectedFindingId(null)
              setChoosing(true)
            }}
            className="mt-2 type-footnote text-label-secondary hover:text-label"
          >
            Choose another change
          </button>
        </>
      ) : (
        <>
          <button
            type="button"
            onClick={() => setChoosing(true)}
            className="ui-push-button gap-1.5"
          >
            <Wrench size={12} aria-hidden="true" />
            Fix
          </button>
          {choosing && (
            <div className="mt-2 flex flex-wrap gap-2">
              {eligible.map((target) => (
                <button
                  key={target.actionId}
                  type="button"
                  onClick={() => setSelectedFindingId(target.findingId)}
                  className="ui-push-button"
                >
                  {target.display.currentValue ?? "Review change"}
                </button>
              ))}
            </div>
          )}
        </>
      )}
    </div>
  )
}

export function BurnCheckDetail({
  detector,
  targets,
  refresh,
}: {
  detector: BurnCheckDetectorId
  targets: BurnCheckTargetPayload[]
  refresh: () => void
}) {
  const statuses = Array.from(
    new Set(targets.map(watchStatus).filter((status): status is string => status !== null)),
  )
  return (
    <article className="px-4 py-4">
      <p className="type-callout text-label-secondary">{CHECK_SENTENCES[detector]}</p>
      <div className="mt-3 flex flex-wrap items-center gap-2">
        <CheckPromptAction
          key={targets.map((target) => target.actionId).join(":")}
          detector={detector}
          targets={targets}
          refresh={refresh}
        />
        <FixAction targets={targets} refresh={refresh} />
      </div>
      {statuses.length === 1 && (
        <p role="status" className="mt-3 type-footnote text-label-secondary">
          {statuses[0]}
        </p>
      )}
      <Samples targets={targets} />
    </article>
  )
}
