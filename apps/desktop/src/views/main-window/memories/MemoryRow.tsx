import { ChevronRight } from "lucide-react"

import { TruncatedText } from "../../../components/presentation/TruncatedText"
import { PushButton } from "../../../components/ui/PushButton"
import { cn } from "../../../lib/cn"
import type { MemoryEntry } from "../../../lib/memoriesIpc"
import type { ArchivedMemory, RowError } from "./MemoriesSession"
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

const TEXT_BUTTON = "type-footnote font-medium text-accent hover:underline"

/** A destructive action: red text, no fill. */
export const DESTRUCTIVE_TEXT_BUTTON = "type-footnote text-system-red-text hover:underline"

/** An inline error with a Reload button. */
export function RowErrorNotice({
  error,
  failedText,
  onReload,
  className,
}: {
  error: RowError
  failedText: string
  onReload: () => void
  className?: string
}) {
  return (
    <div role="alert" className={cn("flex items-baseline gap-x-3", className)}>
      <span className="type-footnote text-system-orange">
        {error === "changedOnDisk"
          ? "This memory changed on disk. Reload to see the current state."
          : failedText}
      </span>
      <button type="button" onClick={onReload} className={TEXT_BUTTON}>
        Reload
      </button>
    </div>
  )
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
  archived,
  error,
  writesSupported,
  onToggle,
  onReveal,
  onDelete,
  onUndo,
  onReload,
  focusRequested = false,
  onFocused,
}: {
  entry: MemoryEntry
  expanded: boolean
  now: number
  archived: ArchivedMemory | undefined
  error: RowError | undefined
  writesSupported: boolean
  onToggle: () => void
  onReveal: () => void
  onDelete: () => void
  onUndo: () => void
  onReload: () => void
  /** Scroll this row to the middle and focus its header, then call `onFocused`. */
  focusRequested?: boolean
  onFocused?: () => void
}) {
  const deleted = archived !== undefined
  const open = expanded && !deleted
  const { facts } = entry
  const known = facts.hasHistory
  const note = hookNote(entry.hookSource)
  return (
    <>
      <button
        type="button"
        ref={
          focusRequested
            ? (button) => {
                if (!button) return
                button.scrollIntoView({ block: "center" })
                button.focus()
                onFocused?.()
              }
            : undefined
        }
        onClick={onToggle}
        aria-expanded={open}
        title={known ? undefined : NO_HISTORY_NOTE}
        className="col-span-full grid min-h-10 grid-cols-subgrid items-center rounded-control px-2 text-left transition-colors duration-[var(--duration-fast)] ease-out hover:bg-surface-hover"
      >
        <ChevronRight
          size={14}
          aria-hidden="true"
          className={cn(
            "shrink-0 text-label-tertiary transition-transform duration-[var(--duration-fast)] ease-out",
            open && "rotate-90",
          )}
        />
        <span className="min-w-0">
          <span
            className={cn(
              "block truncate type-body",
              deleted ? "text-label-tertiary line-through" : "text-label",
            )}
          >
            {entry.title}
          </span>
          {entry.hook && (
            <TruncatedText
              className={cn(
                "type-callout",
                deleted ? "text-label-tertiary line-through" : "text-label-secondary",
              )}
              text={entry.hook}
            />
          )}
        </span>
        {deleted ? (
          <span className="col-span-4" />
        ) : (
          <>
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
          </>
        )}
      </button>
      {deleted && (
        <div className="col-span-full flex items-baseline gap-x-3 px-2">
          <span className="type-footnote text-label-secondary">
            Deleted · moved to antiburn&apos;s archive
          </span>
          <button type="button" onClick={onUndo} className={TEXT_BUTTON}>
            Undo
          </button>
        </div>
      )}
      {error && (
        <RowErrorNotice
          error={error}
          failedText={
            deleted ? "Could not restore this memory." : "Could not delete this memory."
          }
          onReload={onReload}
          className="col-span-full px-2"
        />
      )}
      {open && (
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
          <div className="flex items-center gap-x-4">
            <RevealButton onReveal={onReveal} />
            {writesSupported && (
              <button type="button" onClick={onDelete} className={DESTRUCTIVE_TEXT_BUTTON}>
                Delete
              </button>
            )}
          </div>
        </div>
      )}
    </>
  )
}
