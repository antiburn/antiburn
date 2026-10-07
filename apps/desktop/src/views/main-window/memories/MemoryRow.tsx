import { ChevronRight } from "lucide-react"

import { TruncatedText } from "../../../components/presentation/TruncatedText"
import { PushButton } from "../../../components/ui/PushButton"
import { cn } from "../../../lib/cn"
import type { MemoryEntry } from "../../../lib/memoriesIpc"
import { detectPlatform } from "../../../lib/platform"
import { relativeTime } from "../../../lib/presentation/relativeTime"

const NO_HISTORY_NOTE = "No session history recorded for this memory"

/** The platform's own name for its file manager, in the button label. */
function revealLabel(): string {
  switch (detectPlatform()) {
    case "macos":
      return "Reveal in Finder"
    case "windows":
      return "Show in File Explorer"
    default:
      return "Show in file manager"
  }
}

/** A secondary button that shows one path in the file manager. */
export function RevealButton({ onReveal }: { onReveal: () => void }) {
  return <PushButton onClick={onReveal}>{revealLabel()}</PushButton>
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  return `${(bytes / 1024).toFixed(1)} KB`
}

function relativeFact(ms: number | null, now: number): string {
  return ms == null ? "—" : relativeTime(new Date(ms).toISOString(), { compact: true, now })
}

function hookNote(source: MemoryEntry["hookSource"]): string | null {
  switch (source) {
    case "frontmatter":
      return "Hook from frontmatter"
    case "body":
      return "Hook from first line"
    case "index":
      return null
  }
}

/**
 * One memory in the project's shared grid: a header button on the grid's
 * columns, then, when open, the file's text and its actions across the full
 * row.
 */
export function MemoryRow({
  entry,
  expanded,
  now,
  onToggle,
  onReveal,
}: {
  entry: MemoryEntry
  expanded: boolean
  now: number
  onToggle: () => void
  onReveal: () => void
}) {
  const { facts } = entry
  const known = facts.hasHistory
  const note = hookNote(entry.hookSource)
  return (
    <>
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={expanded}
        title={known ? undefined : NO_HISTORY_NOTE}
        className="col-span-full grid min-h-10 grid-cols-subgrid items-center rounded-control px-2 text-left transition-colors duration-[var(--duration-fast)] ease-out hover:bg-surface-hover"
      >
        <ChevronRight
          size={14}
          aria-hidden="true"
          className={cn(
            "shrink-0 text-label-tertiary transition-transform duration-[var(--duration-fast)] ease-out",
            expanded && "rotate-90",
          )}
        />
        <span className="min-w-0">
          <span className="block truncate type-body text-label">{entry.title}</span>
          {entry.hook && (
            <TruncatedText className="type-callout text-label-secondary" text={entry.hook} />
          )}
        </span>
        <span className="type-footnote text-label-tertiary">{entry.kind ?? ""}</span>
        <span className="font-mono type-footnote tabular-nums text-label-secondary">
          {known ? relativeFact(facts.lastReferencedMs, now) : "—"}
        </span>
        <span className="font-mono type-footnote tabular-nums text-label-secondary">
          {known ? relativeFact(facts.lastWrittenMs, now) : "—"}
        </span>
        <span className="font-mono type-footnote tabular-nums text-label-secondary">
          {known && facts.sessionsSinceWritten != null ? facts.sessionsSinceWritten : "—"}
        </span>
      </button>
      {expanded && (
        <div className="col-span-full grid gap-y-2 rounded-control bg-surface-card/50 px-3 py-2">
          <p className="font-mono type-footnote text-label-tertiary">
            {`${entry.fileName} · ${formatSize(entry.sizeBytes)} · frontmatter ${
              entry.hasFrontmatter ? "yes" : "no"
            }`}
            {entry.truncated && " · truncated at 64 KiB"}
          </p>
          {!entry.inIndex && (
            <p className="type-footnote text-system-orange">
              Not in the index: Claude cannot find this memory.
            </p>
          )}
          {note && <p className="type-footnote text-label-tertiary">{note}</p>}
          <pre className="font-mono type-footnote whitespace-pre-wrap break-words text-label">
            {entry.body}
          </pre>
          <div>
            <RevealButton onReveal={onReveal} />
          </div>
        </div>
      )}
    </>
  )
}
