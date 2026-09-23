import { ChevronDown } from "lucide-react"
import { useRef, useState } from "react"

import { cn } from "../../../lib/cn"
import {
  openBurnCheckSample,
  type BurnCheckSamplePayload,
  type BurnCheckTargetPayload,
} from "../../../lib/insightsIpc"
import { CHECK_LABELS } from "../../../lib/presentation/checks"
import { ScrollPane } from "../../../components/ui/ScrollPane"
import { SessionRow } from "../../../components/session/SessionList"
import { toActivityEntry } from "../../../lib/activityEntries"
import { renderAgentIcon } from "../../../lib/agentIcon"
import { snoozedDetectorIds, useSnoozedBurnChecks } from "../../../lib/snoozedBurnChecks"

export function DisclosureChevron({ open }: { open: boolean }) {
  return (
    <ChevronDown
      size={14}
      strokeWidth={2}
      className={cn(
        "text-label-tertiary transition-transform duration-(--duration-fast) ease-out-quart",
        open && "rotate-180",
      )}
      aria-hidden="true"
    />
  )
}

export function scopeLabel(scope: BurnCheckTargetPayload["display"]["scopeKind"]): string {
  switch (scope) {
    case "global":
      return "Global configuration"
    case "project":
      return "Project configuration"
    case "session":
      return "Session scope"
    case "worker":
      return "Worker scope"
  }
}

export function targetTitle(target: BurnCheckTargetPayload): string {
  return (
    target.display.resourceIdentity ??
    CHECK_LABELS[target.finding.detector] ??
    target.display.resourceKind
  )
}

export function watchStatus(target: BurnCheckTargetPayload): string | null {
  const watch = target.watch
  if (watch?.lifecycle === "waitingForPromptUse") {
    return "The prompt is ready. Verification starts after you use it."
  }
  const verification = watch?.verification
  if (!verification) return null
  switch (verification.status) {
    case "reserved":
      return "Verification has not started."
    case "watching":
      return "Waiting for a later complete session."
    case "fixed":
      return watch.origin === "passive" ? "Verified improvement." : "Verified after your fix."
    case "stillUnresolved":
      return "A later session still has this finding."
    case "recurred":
      return "This finding returned."
    case "recoveryNeeded":
      return null
    case "verificationUnavailable":
      return null
  }
}

function sizeSessionList(viewport: HTMLDivElement | null) {
  const list = viewport?.querySelector("[data-failed-session-cards]")
  if (!viewport || !list) return
  const measure = () => {
    const first = list.children.item(0)
    const fifth = list.children.item(4)
    if (!first || !fifth) return
    const height = fifth.getBoundingClientRect().bottom - first.getBoundingClientRect().top
    if (height > 0) viewport.style.maxHeight = `${height}px`
  }
  measure()
  const observer = new ResizeObserver(measure)
  observer.observe(list)
  for (const card of Array.from(list.children).slice(0, 5)) observer.observe(card)
  return () => {
    observer.disconnect()
    viewport.style.removeProperty("max-height")
  }
}

export function FailedSessions({
  samples,
  total,
}: {
  samples: BurnCheckSamplePayload[]
  total?: number
}) {
  const [status, setStatus] = useState<string | null>(null)
  const [busyHandle, setBusyHandle] = useState<string | null>(null)
  const opening = useRef(false)
  const scrollable = samples.length > 5
  const snoozes = useSnoozedBurnChecks()
  if (snoozes.status !== "ready") return null
  const snoozedDetectors = snoozedDetectorIds(snoozes.records)
  if (total === 0) return null
  const cards = (
    <div data-failed-session-cards className="flex flex-col gap-2">
      {samples.map((sample) => (
        <SessionRow
          key={sample.navigationHandle}
          entry={toActivityEntry(sample)}
          hygiene={sample.hygiene}
          snoozedDetectors={snoozedDetectors}
          renderAgentIcon={renderAgentIcon}
          showAgentLabel
          busy={busyHandle !== null}
          onOpen={async () => {
            if (opening.current) return
            opening.current = true
            setBusyHandle(sample.navigationHandle)
            setStatus(null)
            try {
              const result = await openBurnCheckSample(sample.navigationHandle)
              if (result?.outcome === "opened") return
              setStatus(
                result?.outcome === "deleted"
                  ? "This session was deleted."
                  : result?.outcome === "expired"
                    ? "This session is no longer available."
                    : "This session is unavailable.",
              )
            } catch {
              setStatus("Could not open this session. Try again.")
            } finally {
              opening.current = false
              setBusyHandle(null)
            }
          }}
        />
      ))}
    </div>
  )
  return (
    <div className="burn-check-samples">
      <div
        role={scrollable ? undefined : "region"}
        aria-label={scrollable ? undefined : "Failed sessions"}
        className="mt-1"
      >
        {scrollable ? (
          <ScrollPane
            topEdgeFade
            bottomEdgeFade
            className="flex-none"
            viewportRef={sizeSessionList}
            viewportTabIndex={0}
            viewportLabel="Failed sessions"
            viewportClassName="pr-3 overscroll-y-contain"
          >
            {cards}
          </ScrollPane>
        ) : (
          cards
        )}
      </div>
      {status && (
        <p role="status" className="mt-2 type-callout text-label-secondary">
          {status}
        </p>
      )}
    </div>
  )
}
