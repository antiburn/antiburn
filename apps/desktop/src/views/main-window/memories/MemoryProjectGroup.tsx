import { ChevronRight } from "lucide-react"

import { CountPill } from "../../../components/ui/CountPill"
import { cn } from "../../../lib/cn"
import type { DanglingIndexEntry, MemoryEntry, MemoryProject } from "../../../lib/memoriesIpc"
import { relativeTime } from "../../../lib/presentation/relativeTime"
import type { MemoriesSnapshot } from "./MemoriesSession"
import { indexLineKey, sortMemories } from "./MemoriesSession"
import { DESTRUCTIVE_TEXT_BUTTON, MemoryRow, RevealButton, RowErrorNotice } from "./MemoryRow"

function plural(count: number, word: string): string {
  return `${count} ${word}${count === 1 ? "" : "s"}`
}

function entries(count: number): string {
  return count === 1 ? "1 entry" : `${count} entries`
}

const COLUMN_LABEL = "type-metadata uppercase tracking-wide text-label-tertiary"

/** One project's memories: a collapsible header, its index line, and the rows. */
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
}: {
  project: MemoryProject
  collapsed: boolean
  state: Pick<
    MemoriesSnapshot,
    "expandedMemories" | "archived" | "rowErrors" | "removedIndexLines" | "indexBackupWritten"
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
}) {
  const { expandedMemories, archived, rowErrors, removedIndexLines, indexBackupWritten } = state
  // Deleted rows stay in the list but no longer count.
  const live = project.memories.filter((entry) => !archived.has(entry.path))
  const dangling = project.dangling.filter(
    (entry) => !removedIndexLines.has(indexLineKey(project.slug, entry.lineNumber)),
  )
  const orphanCount = live.filter((entry) => !entry.inIndex).length
  const attention = dangling.length + orphanCount
  const history =
    project.sessionCount === 0
      ? "no session history"
      : `${plural(project.sessionCount, "session")}${
          project.lastSessionMs == null
            ? ""
            : ` · last ${relativeTime(new Date(project.lastSessionMs).toISOString(), {
                compact: true,
                now,
              })} ago`
        }`
  return (
    <section className="grid gap-y-2">
      <button
        type="button"
        onClick={onToggleProject}
        aria-expanded={!collapsed}
        className="flex w-full items-center justify-between gap-x-3 rounded-control px-2 py-1.5 text-left transition-colors duration-[var(--duration-fast)] ease-out hover:bg-surface-hover"
      >
        <span className="flex min-w-0 items-center gap-x-2">
          <ChevronRight
            size={14}
            aria-hidden="true"
            className={cn(
              "shrink-0 transition-transform duration-[var(--duration-fast)] ease-out",
              !collapsed && "rotate-90",
            )}
          />
          <span className="truncate font-mono type-body-large text-label">
            {project.displayPath}
          </span>
          <CountPill count={live.length} aria-label="Memories" />
          {attention > 0 && (
            <span className="shrink-0 type-footnote text-system-orange">
              {attention} {attention === 1 ? "needs" : "need"} attention
            </span>
          )}
        </span>
        <span className="shrink-0 font-mono type-footnote tabular-nums text-label-tertiary">
          {history}
        </span>
      </button>
      {!collapsed && (
        <div className="grid gap-y-2 pl-6">
          <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
            {project.indexPath ? (
              <>
                <span className="font-mono type-callout text-label">MEMORY.md</span>
                <span className="type-footnote text-label-tertiary">
                  {entries(live.filter((entry) => entry.inIndex).length + dangling.length)}
                </span>
                <RevealButton onReveal={() => onReveal(project.indexPath!)} />
              </>
            ) : (
              <span className="type-footnote text-label-tertiary">
                No MEMORY.md index: Claude cannot find these memories.
              </span>
            )}
            {indexBackupWritten.has(project.slug) && (
              <span className="type-footnote text-label-tertiary">
                Backup written to MEMORY.md.antiburn-bak
              </span>
            )}
            {!writesSupported && (
              <span className="type-footnote text-label-tertiary">
                Editing memories is not supported on Windows yet.
              </span>
            )}
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
          <div className="grid grid-cols-[auto_minmax(0,1fr)_auto_auto_auto_auto] gap-x-4 gap-y-1">
            <div className="col-span-full grid grid-cols-subgrid px-2">
              <span className={cn(COLUMN_LABEL, "col-span-2")}>Memory</span>
              <span className={COLUMN_LABEL}>Kind</span>
              <span className={COLUMN_LABEL}>Last referenced</span>
              <span className={COLUMN_LABEL}>Last written</span>
              <span className={COLUMN_LABEL}>Sessions since written</span>
            </div>
            {sortMemories(project.memories).map((entry) => (
              <MemoryRow
                key={entry.path}
                entry={entry}
                expanded={expandedMemories.has(entry.path)}
                now={now}
                archived={archived.get(entry.path)}
                error={rowErrors.get(entry.path)}
                writesSupported={writesSupported}
                onToggle={() => onToggleMemory(entry.path)}
                onReveal={() => onReveal(entry.path)}
                onDelete={() => onDelete(entry)}
                onUndo={() => onUndo(entry)}
                onReload={onReload}
              />
            ))}
          </div>
        </div>
      )}
    </section>
  )
}
