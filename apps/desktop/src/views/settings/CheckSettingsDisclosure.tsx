import { ChevronDown } from "lucide-react"
import { useId, useState, type ReactNode } from "react"
import { detectPlatform } from "../../lib/platform"
import { settingsControlLabel } from "../../lib/settingsSearchTargets"

export function CheckSettingsDisclosure({
  searchId,
  control,
  targetRevision,
  summary,
  children,
}: {
  searchId: "checkDirectories" | "checkModelUsage"
  control?: string | null | undefined
  targetRevision?: number | undefined
  summary?: ReactNode
  children: ReactNode
}) {
  const requested = control === searchId ? `${control}:${targetRevision ?? 0}` : null
  const [state, setState] = useState({ requested, open: Boolean(requested) })
  const open = requested && requested !== state.requested ? true : state.open
  if (requested && requested !== state.requested) setState({ requested, open: true })
  const bodyId = useId()
  const summaryId = useId()
  const label = settingsControlLabel(searchId, detectPlatform())
  return (
    <div data-settings-control={searchId} tabIndex={-1}>
      <button
        type="button"
        aria-label={label}
        aria-describedby={summary ? summaryId : undefined}
        aria-expanded={open}
        aria-controls={bodyId}
        onClick={() => setState({ requested, open: !open })}
        className="flex w-full items-start gap-3 px-4 py-3 text-left hover:bg-surface-hover"
      >
        <ChevronDown
          size={14}
          aria-hidden="true"
          className={`mt-1 shrink-0 text-label-secondary transition-transform duration-[var(--duration-fast)] ${open ? "rotate-180" : ""}`}
        />
        <span className="min-w-0 flex-1">
          <span className="block type-body text-label">{label}</span>
          {summary && (
            <span
              id={summaryId}
              className="mt-1 block wrap-anywhere type-footnote text-label-secondary"
            >
              {summary}
            </span>
          )}
        </span>
      </button>
      {open && (
        <div id={bodyId} className="min-w-0 px-4 pb-4">
          {children}
        </div>
      )}
    </div>
  )
}
