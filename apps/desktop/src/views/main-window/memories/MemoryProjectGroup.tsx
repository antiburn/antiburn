import { ChevronRight } from "lucide-react"
import type { ReactNode } from "react"

import { agentDisplayName } from "../../../lib/presentation/agents"
import { renderAgentIcon } from "../../../lib/agentIcon"
import { cn } from "../../../lib/cn"
import type { DanglingIndexEntry, MemoryEntry, MemoryProject } from "../../../lib/memoriesIpc"
import { relativeTime } from "../../../lib/presentation/relativeTime"
import type { ArchivedMemory, MemoriesSnapshot, RowError } from "./MemoriesSession"
import { indexLineKey, sortMemories } from "./MemoriesSession"
import { detectPlatform } from "../../../lib/platform"

const FACT = "text-center type-footnote text-label-secondary"

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

const TEXT_BUTTON = "font-medium text-accent hover:underline"
const DESTRUCTIVE_TEXT_BUTTON = "text-system-red-text hover:underline"

function RowErrorNotice({
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

function RevealTextButton({ onReveal }: { onReveal: () => void }) {
  return (
    <button type="button" onClick={onReveal} className={TEXT_BUTTON}>
      {revealLabel()}
    </button>
  )
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  return `${(bytes / 1024).toFixed(1)} KB`
}

function relativeFact(type: string, ms: number | null, now: number): string {
  return ms == null
    ? ""
    : `${type} ${relativeTime(new Date(ms).toISOString(), { compact: true, now })} ago`
}

function SourceBlock({
  caption,
  children,
  className,
  label,
}: {
  caption: string
  children: ReactNode
  className?: string
  label: string
}) {
  return (
    <div className={cn("grid gap-y-1 mt-2", className)}>
      <div className="flex justify-between gap-x-2 items-baseline pb-1 border-b border-separator text-label-tertiary type-body">
        <span>{label}</span>
        <span>{caption}</span>
      </div>

      {children}
    </div>
  )
}

const SOURCE_TEXT = "font-mono type-footnote whitespace-pre-wrap break-words text-label"

function MemoryRow({
  entry,
  expanded,
  now,
  archived,
  error,
  writesSupported,
  showFacts,
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
  showFacts: boolean
  onToggle: () => void
  onReveal: () => void
  onDelete: () => void
  onUndo: () => void
  onReload: () => void
  focusRequested?: boolean
  onFocused?: () => void
}) {
  const deleted = archived !== undefined
  const open = expanded && !deleted
  const { facts } = entry
  const known = facts.hasHistory

  return (
    <div
      className={cn(
        "col-span-full grid grid-cols-subgrid rounded-control px-2",
        open && "pb-3 bg-surface-card/70",
      )}
    >
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
        className={cn(
          "col-span-full grid min-h-10 grid-cols-subgrid items-center -ml-2 gap-x-2 px-2 py-2 rounded-control text-left transition-colors duration-[var(--duration-fast)] ease-out hover:bg-surface-hover",
          open && "border-b border-separator",
        )}
      >
        <ChevronRight
          size={14}
          aria-hidden="true"
          className={cn(
            "shrink-0 text-label-tertiary transition-transform duration-[var(--duration-fast)] ease-out",
            open && "rotate-90",
          )}
        />

        <span className="flex items-baseline">
          <span
            className={cn(
              "truncate type-body",
              deleted ? "text-label-tertiary line-through" : "text-label",
            )}
          >
            {entry.title}
          </span>
          {!entry.inIndex && !deleted && (
            <span className="ms-2 shrink-0 type-metadata text-system-orange">not in index</span>
          )}
        </span>

        {deleted ? (
          <span className={showFacts ? "col-span-4" : "col-span-2"} />
        ) : (
          <>
            <span className="type-footnote text-label-tertiary text-center">
              {entry.kind ?? ""}
            </span>

            {showFacts && (
              <>
                <span className={FACT}>
                  {known ? relativeFact("read", facts.lastReferencedMs, now) : ""}
                </span>
                <span className={FACT}>
                  {known ? relativeFact("written", facts.lastWrittenMs, now) : ""}
                </span>
              </>
            )}
            <span className="text-right font-mono type-footnote tabular-nums text-label-tertiary">
              {formatSize(entry.sizeBytes)}
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
        <div className="col-span-full grid gap-y-4 px-3 py-2">
          <div className="flex justify-between type-body">
            <p className="text-label-tertiary">
              Filename: {entry.fileName}{" "}
              {entry.truncated && <span> (truncated at 64 KiB)</span>}
            </p>

            <div className="flex items-center gap-x-4">
              <RevealTextButton onReveal={onReveal} />

              {writesSupported && (
                <button type="button" onClick={onDelete} className={DESTRUCTIVE_TEXT_BUTTON}>
                  Delete
                </button>
              )}
            </div>
          </div>

          <SourceBlock
            label="MEMORY.md entry"
            caption="this is always read by Claude at the start of every session"
          >
            {entry.indexEntry ? (
              <div className="grid grid-cols-[auto_1fr] gap-x-2 type-footnote text-label">
                <span>Title:</span>
                <span className={SOURCE_TEXT}>{entry.indexEntry.title}</span>

                {entry.indexEntry.hook ? (
                  <>
                    <span>Hook:</span>
                    <span className={SOURCE_TEXT}>{entry.indexEntry.hook}</span>
                  </>
                ) : (
                  <span className="col-span-full type-callout text-label-tertiary">
                    No hook on this line.
                  </span>
                )}
              </div>
            ) : (
              <p className="type-footnote text-system-orange">
                Not in the index. Claude cannot find this memory.
              </p>
            )}
          </SourceBlock>

          <SourceBlock
            label="Frontmatter"
            caption="this was written when the memory was saved; the description is what Claude searches"
          >
            {entry.frontmatter != null ? (
              <pre className={SOURCE_TEXT}>{entry.frontmatter}</pre>
            ) : (
              <p className="type-footnote text-label-tertiary">No frontmatter</p>
            )}
          </SourceBlock>

          <SourceBlock
            label="Body"
            caption="this is loaded only when Claude decides to read this memory"
          >
            {entry.body.trim() ? (
              <pre className={SOURCE_TEXT}>{entry.body}</pre>
            ) : (
              <p className="type-footnote text-label-tertiary">Empty</p>
            )}
          </SourceBlock>
        </div>
      )}
    </div>
  )
}

function plural(count: number, word: string): string {
  return `${count} ${word}${count === 1 ? "" : "s"}`
}

function entries(count: number): string {
  return count === 1 ? "1 entry" : `${count} entries`
}

function folderName(project: MemoryProject): string {
  if (project.displayPath === project.slug) return project.slug
  const segments = project.displayPath.split(/[\\/]/).filter(Boolean)
  return segments[segments.length - 1] ?? project.displayPath
}

export function MemoryProjectGroup({
  project,
  collapsed,
  state,
  writesSupported,
  now,
  onToggleProject,
  onToggleMemory,
  onReveal,
  onDelete,
  onUndo,
  onRemoveLine,
  onReload,
  onFocusHandled,
}: {
  project: MemoryProject
  collapsed: boolean
  state: Pick<
    MemoriesSnapshot,
    | "expandedMemories"
    | "archived"
    | "rowErrors"
    | "removedIndexLines"
    | "indexBackupWritten"
    | "focusRequest"
  >
  writesSupported: boolean
  now: number
  onToggleProject: () => void
  onToggleMemory: (path: string) => void
  onReveal: (path: string) => void
  onDelete: (entry: MemoryEntry) => void
  onUndo: (entry: MemoryEntry) => void
  onRemoveLine: (entry: DanglingIndexEntry) => void
  onReload: () => void
  onFocusHandled: (revision: number) => void
}) {
  const {
    expandedMemories,
    archived,
    rowErrors,
    removedIndexLines,
    indexBackupWritten,
    focusRequest,
  } = state
  // Deleted rows stay in the list but no longer count.
  const live = project.memories.filter((entry) => !archived.has(entry.path))
  const dangling = project.dangling.filter(
    (entry) => !removedIndexLines.has(indexLineKey(project.slug, entry.lineNumber)),
  )
  const orphanCount = live.filter((entry) => !entry.inIndex).length
  const attention = dangling.length + orphanCount
  const hasHistory = project.sessionCount > 0

  return (
    <section className="session-card rounded-(--radius-popover) bg-session-card">
      <button
        type="button"
        onClick={onToggleProject}
        aria-expanded={!collapsed}
        className="grid w-full grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-x-3 rounded-(--radius-popover) px-3 py-2 text-left hover:bg-surface-secondary/50"
      >
        <ChevronRight
          size={14}
          aria-hidden="true"
          className={cn(
            "shrink-0 text-label-tertiary transition-transform duration-[var(--duration-fast)] ease-out",
            !collapsed && "rotate-90",
          )}
        />

        <span className="min-w-0 flex items-baseline [&>*+*]:before:content-['·'] [&>*+*]:before:mx-1.5 truncate font-mono type-metadata text-label-tertiary tabular-nums">
          <span className="type-body font-medium! text-label">{folderName(project)}</span>
          <span>{project.displayPath}</span>
          {!hasHistory ? (
            <span>no sessions found</span>
          ) : (
            <>
              <span>{plural(project.sessionCount, "session")}</span>
              {project.lastSessionMs && (
                <span>
                  last{" "}
                  {relativeTime(new Date(project.lastSessionMs).toISOString(), {
                    compact: true,
                    now,
                  })}{" "}
                  ago
                </span>
              )}
            </>
          )}

          {attention > 0 && (
            <span className="shrink-0 text-system-orange">
              {attention} {attention === 1 ? "needs" : "need"} attention
            </span>
          )}

          {!project.folderExists && (
            <span className="shrink-0 text-system-orange">folder not found</span>
          )}
        </span>

        <span className="font-mono text-label justify-self-end">
          {live.length} {live.length === 1 ? "memory" : "memories"} [
          {formatSize(live.reduce((total, entry) => total + entry.sizeBytes, 0))}]
        </span>
      </button>

      {!collapsed && (
        <div className="py-2 ps-4">
          <div className="flex flex-wrap items-baseline type-footnote text-label-tertiary [&>*+*]:before:content-['·'] [&>*+*]:before:mx-1.5">
            <span className="flex items-center gap-x-1.5">
              {renderAgentIcon(project.agent, 10)}
              {agentDisplayName(project.agent)}
            </span>

            {project.indexPath ? (
              <>
                <span>MEMORY.md</span>
                <span>
                  {entries(live.filter((entry) => entry.inIndex).length + dangling.length)}
                </span>

                <RevealTextButton onReveal={() => onReveal(project.indexPath!)} />
              </>
            ) : (
              <span className="type-footnote text-system-orange">
                No MEMORY.md index · Claude cannot find these memories.
              </span>
            )}

            {indexBackupWritten.has(project.slug) && (
              <span>Backup written to MEMORY.md.antiburn-bak</span>
            )}

            {!writesSupported && <span>Editing memories is not supported on Windows yet.</span>}
          </div>

          {dangling.length > 0 && (
            <div>
              <p className="type-footnote text-system-orange">
                {dangling.length === 1
                  ? "1 entry points to a missing file"
                  : `${dangling.length} entries point to missing files`}
              </p>
              <ul className="font-mono type-footnote text-label-secondary">
                {dangling.map((entry) => {
                  const error = rowErrors.get(indexLineKey(project.slug, entry.lineNumber))
                  return (
                    <li key={`${entry.lineNumber}:${entry.target}`}>
                      <span className="flex items-baseline gap-x-3">
                        <span>
                          {entry.title} → {entry.target}
                        </span>
                        {writesSupported && (
                          <button
                            type="button"
                            onClick={() => onRemoveLine(entry)}
                            className={DESTRUCTIVE_TEXT_BUTTON}
                          >
                            Remove line
                          </button>
                        )}
                      </span>
                      {error && (
                        <RowErrorNotice
                          error={error}
                          failedText="Could not remove this line."
                          onReload={onReload}
                        />
                      )}
                    </li>
                  )
                })}
              </ul>
            </div>
          )}

          <div
            className={cn(
              "py-2 grid",
              hasHistory
                ? "grid-cols-[auto_minmax(0,1fr)_auto_auto_auto_auto]"
                : "grid-cols-[auto_minmax(0,1fr)_auto_auto]",
            )}
          >
            {sortMemories(project.memories).map((entry) => (
              <MemoryRow
                key={entry.path}
                entry={entry}
                expanded={expandedMemories.has(entry.path)}
                now={now}
                archived={archived.get(entry.path)}
                error={rowErrors.get(entry.path)}
                writesSupported={writesSupported}
                showFacts={hasHistory}
                onToggle={() => onToggleMemory(entry.path)}
                onReveal={() => onReveal(entry.path)}
                onDelete={() => onDelete(entry)}
                onUndo={() => onUndo(entry)}
                onReload={onReload}
                focusRequested={focusRequest?.path === entry.path}
                onFocused={() => focusRequest && onFocusHandled(focusRequest.revision)}
              />
            ))}
          </div>
        </div>
      )}
    </section>
  )
}
