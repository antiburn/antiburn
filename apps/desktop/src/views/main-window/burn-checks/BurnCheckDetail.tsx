import { Check, Clipboard, Wrench } from "lucide-react"
import { useState } from "react"

import { pluralize } from "../../../lib/presentation/text"
import { cn } from "../../../lib/cn"
import {
  copyPromptFixBurnCheck,
  type BurnCheckDetectorId,
  type BurnCheckTargetPayload,
  type BurnCheckSamplePayload,
} from "../../../lib/insightsIpc"
import { BurnCheckTargetActions } from "./BurnCheckTargetActions"
import { RemindLaterAction } from "./RemindLaterAction"
import { FailedSessions, watchStatus } from "./BurnCheckTargetPresentation"
import { BurnCheckTargetChooserDialog } from "./BurnCheckTargetChooserDialog"
import { useBurnCheckPromptCopy } from "./useBurnCheckPromptCopy"

export const CHECK_SENTENCES: Record<BurnCheckDetectorId, string> = {
  sessionsOverDepth: "Some sessions carried context after it stopped helping.",
  modelOverthinking: "Some work used more reasoning than it needed.",
  overpoweredSubagents: "Some helper work used a stronger model than it needed.",
  unusedMcpServers: "Some connected tool servers loaded tools that were not used.",
  unusedBuiltInTools: "Some built-in tools were loaded but not used.",
  unusedSkills: "Some skills were loaded but not used.",
  oldModelUsage: "Some sessions used an older model when a newer one was available.",
  overuseOfFastMode: "Some work paid for speed it did not need.",
  cacheChurn: "Some sessions kept paying to reload the same context.",
}

export function CheckPromptAction({
  detector,
  refresh,
  active = true,
}: {
  detector: BurnCheckDetectorId
  targets: BurnCheckTargetPayload[]
  refresh: () => void
  active?: boolean
}) {
  const { busy, copied, status, bindRoot, copy } = useBurnCheckPromptCopy({
    subject: detector,
    active,
    prepare: () => copyPromptFixBurnCheck(detector),
    refresh,
  })
  return (
    <div ref={bindRoot}>
      <button
        type="button"
        disabled={busy}
        onClick={() => void copy()}
        className="burn-check-action burn-check-action-primary type-callout gap-1 disabled:opacity-100"
      >
        {copied ? (
          <Check size={12} className="text-token-in" aria-hidden="true" />
        ) : (
          <Clipboard size={12} aria-hidden="true" />
        )}
        {copied ? "Copied" : busy ? "Preparing…" : "Copy fix prompt"}
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
  active = true,
  showFindingActions = true,
}: {
  detector: BurnCheckDetectorId
  targets: BurnCheckTargetPayload[]
  refresh: () => void
  reportRow?: boolean
  active?: boolean
  showFindingActions?: boolean
}) {
  return (
    <div className="flex flex-wrap items-start gap-2">
      {reportRow && showFindingActions && <RemindLaterAction detector={detector} />}
      {showFindingActions && <FixAction targets={targets} refresh={refresh} />}
      <CheckPromptAction
        detector={detector}
        targets={targets}
        refresh={refresh}
        active={active}
      />
    </div>
  )
}

export function BurnCheckDetail({
  detector,
  targets,
  samples,
  failedSessionCount,
  refresh,
  contained = false,
  reportRow = false,
  active = true,
}: {
  detector: BurnCheckDetectorId
  targets: BurnCheckTargetPayload[]
  samples: BurnCheckSamplePayload[]
  failedSessionCount: number
  refresh: () => void
  contained?: boolean
  reportRow?: boolean
  active?: boolean
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
            <CheckDetailActions
              detector={detector}
              targets={targets}
              refresh={refresh}
              active={active}
            />
          </div>
        </div>
      )}
      {statuses.length === 1 && (
        <p role="status" className="mt-3 type-callout text-label-secondary">
          {statuses[0]}
        </p>
      )}
      {reportRow && failedSessionCount > 0 && (
        <p className="mt-1 type-callout tabular-nums text-label-secondary">
          {`${failedSessionCount} ${pluralize(failedSessionCount, "session", "sessions")} affected`}
        </p>
      )}
      <FailedSessions samples={samples} total={failedSessionCount} />
    </article>
  )
}
