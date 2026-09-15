import { ChevronDown, ChevronRight } from "lucide-react"
import { useId, useState } from "react"

import { cn } from "../../../lib/cn"
import {
  openBurnCheckSample,
  type BurnCheckSamplePayload,
  type BurnCheckTargetPayload,
} from "../../../lib/insightsIpc"
import { CHECK_LABELS } from "../../../lib/presentation/checks"
import { SessionSampleRow } from "../../../components/session/SessionSampleRow"

export function DisclosureChevron({ open }: { open: boolean }) {
  return (
    <ChevronDown
      size={14}
      strokeWidth={2}
      className={cn(
        "text-label-tertiary transition-transform duration-[var(--duration-fast)] ease-out-quart",
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
  const verification = target.watch?.verification
  if (!verification) return null
  switch (verification.status) {
    case "reserved":
      return "The change is reserved. Verification has not started."
    case "watching":
      return null
    case "fixed":
      return null
    case "stillUnresolved":
      return "Fresh evidence still shows this finding."
    case "recurred":
      return "This finding returned after it was verified."
    case "recoveryNeeded":
      return "The write result is uncertain. Review the setting before another change."
    case "verificationUnavailable":
      return null
  }
}

function noActionReason(target: BurnCheckTargetPayload): string | null {
  if (target.autoFix.status === "available" || target.promptFix.status === "available")
    return null
  switch (target.autoFix.reason) {
    case "activeWatch":
      return "An existing change is still being checked."
    case "safetyCheckFailed":
      return "The current setting did not pass the write safety check."
    case "targetNotFound":
      return "This exact setting is no longer available."
    case "unsupportedOrUnprovenTarget":
      return "This target does not support a safe automatic change or prepared prompt."
  }
}

export function ActionLimit({ target }: { target: BurnCheckTargetPayload }) {
  const reason = noActionReason(target)
  if (!reason) return null
  return <p className="mt-2 type-callout text-label-tertiary">{reason}</p>
}

export function SampleSessions({
  samples,
  affectedSessionCount,
  insetRows = false,
}: {
  samples: BurnCheckSamplePayload[]
  affectedSessionCount?: number
  insetRows?: boolean
}) {
  const [manualOpen, setManualOpen] = useState<boolean | null>(null)
  const [status, setStatus] = useState<string | null>(null)
  const [busyHandle, setBusyHandle] = useState<string | null>(null)
  const id = useId()
  const displayedSamples = samples.slice(0, 3)
  if (displayedSamples.length === 0) return null
  const open = manualOpen ?? displayedSamples.length === 1
  return (
    <div className="burn-check-samples">
      <button
        type="button"
        aria-expanded={open}
        aria-controls={id}
        aria-describedby={
          affectedSessionCount != null && affectedSessionCount >= displayedSamples.length
            ? `${id}-summary`
            : undefined
        }
        onClick={() => setManualOpen(!open)}
        className="-mx-2 inline-flex min-h-10 items-center gap-1.5 rounded-control px-2 py-1 text-left type-callout font-semibold! text-label-secondary transition-colors duration-[var(--duration-fast)] ease-out-quart hover:text-label active:transform-none active:opacity-100"
      >
        <ChevronRight
          size={12}
          className={cn(
            "transition-transform duration-[var(--duration-fast)] ease-out-quart",
            open && "rotate-90",
          )}
          aria-hidden="true"
        />
        <span>{displayedSamples.length === 1 ? "Sample session" : "Sample sessions"}</span>{" "}
        <span className="burn-check-group-count type-footnote tabular-nums text-label-tertiary">
          {displayedSamples.length}
        </span>
      </button>
      {affectedSessionCount != null && affectedSessionCount >= displayedSamples.length && (
        <span id={`${id}-summary`} className="sr-only">
          {displayedSamples.length} sample{" "}
          {displayedSamples.length === 1 ? "session" : "sessions"} out of {affectedSessionCount}{" "}
          affected.
        </span>
      )}
      <div id={id} hidden={!open} className="mt-1 flex flex-col gap-1">
        {displayedSamples.map((sample) => (
          <SessionSampleRow
            key={sample.navigationHandle}
            title={sample.title}
            agent={sample.agent}
            surface={sample.surface}
            observedAtMs={sample.observedAtMs}
            busy={busyHandle !== null}
            trailing="up-right"
            appearance={insetRows ? "inset" : "card"}
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
        <p role="status" className="mt-2 type-callout text-label-secondary">
          {status}
        </p>
      )}
    </div>
  )
}
