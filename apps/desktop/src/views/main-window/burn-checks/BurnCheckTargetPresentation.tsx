import { ChevronDown } from "lucide-react"
import { useId, useState } from "react"

import { cn } from "../../../lib/cn"
import { openBurnCheckSample, type BurnCheckTargetPayload } from "../../../lib/insightsIpc"
import { CHECK_LABELS } from "../../../lib/presentation/checks"
import { SessionSampleRow } from "../../../components/session/SessionSampleRow"

export function DisclosureChevron({ open }: { open: boolean }) {
  return (
    <ChevronDown
      size={14}
      strokeWidth={2}
      className={cn(
        "text-label-tertiary transition-transform duration-[var(--duration-fast)]",
        open && "rotate-180",
      )}
      aria-hidden="true"
    />
  )
}

export function scopeLabel(scope: BurnCheckTargetPayload["display"]["scopeKind"]): string {
  return `${scope.charAt(0).toUpperCase()}${scope.slice(1)} scope`
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
      return "Watching for fresh evidence. The finding is not fixed yet."
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
  return <p className="mt-2 type-footnote text-label-tertiary">{reason}</p>
}

export function SampleSessions({ target }: { target: BurnCheckTargetPayload }) {
  const [open, setOpen] = useState(false)
  const [status, setStatus] = useState<string | null>(null)
  const [busyHandle, setBusyHandle] = useState<string | null>(null)
  const id = useId()
  const samples = target.samples.slice(0, 3)
  if (samples.length === 0) return null
  return (
    <div className="mt-3">
      <button
        type="button"
        aria-expanded={open}
        aria-controls={id}
        onClick={() => setOpen((value) => !value)}
        className="inline-flex items-center gap-1.5 rounded-control py-1 text-left type-footnote text-label-tertiary hover:text-label-secondary active:transform-none active:opacity-100"
      >
        <span>
          Sample sessions <span>({samples.length})</span>
        </span>
        <DisclosureChevron open={open} />
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
                      ? "This sample link expired. Refresh Burn checks and try again."
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
