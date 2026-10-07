import { Brain } from "lucide-react"
import { useSyncExternalStore } from "react"

import { ScrollPane } from "../../../components/ui/ScrollPane"
import { cn } from "../../../lib/cn"
import { detectPlatform } from "../../../lib/platform"
import { relativeTime } from "../../../lib/presentation/relativeTime"
import type { MemoriesSession } from "./MemoriesSession"
import { MemoryProjectGroup } from "./MemoryProjectGroup"

function MemoriesEmptyState() {
  const computer = detectPlatform() === "macos" ? "Mac" : "computer"
  return (
    <div
      role="status"
      className="flex flex-1 flex-col items-center justify-center px-8 py-12 text-center"
    >
      <Brain size={28} aria-hidden className="mb-3 text-label-tertiary" />
      <p className="type-body text-label">No agent memories found</p>
      <p className="mt-1 type-callout text-label-tertiary">
        {`Claude Code stores memories under ~/.claude/projects/<project>/memory. None were found on this ${computer}.`}
      </p>
    </div>
  )
}

/**
 * The main window's Memories section: every project's Claude Code memories
 * with what the stored tool calls say about their use. A memory can be deleted
 * (moved to the archive, with Undo) and a dangling index line removed.
 */
export function MemoriesView({
  active,
  session,
}: {
  active: boolean
  session: MemoriesSession
}) {
  const state = useSyncExternalStore(
    active ? session.subscribe : session.subscribeInactive,
    session.getSnapshot,
    session.getSnapshot,
  )
  const { report } = state
  const now = session.now()
  const memoryCount =
    report?.projects.reduce(
      (sum, p) => sum + p.memories.filter((memory) => !state.archived.has(memory.path)).length,
      0,
    ) ?? 0

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col bg-surface-window">
      <h1 className="sr-only">Memories</h1>
      {state.error ? (
        <div className="flex flex-1 items-center justify-center text-center">
          <div>
            <p role="alert" className="type-body text-label-secondary">
              Memories are unavailable.
            </p>
            <button type="button" onClick={session.refresh} className="ui-push-button mt-3">
              Retry
            </button>
          </div>
        </div>
      ) : !report ? (
        <p role="status" aria-busy="true" className="p-8 type-body text-label-secondary">
          Loading Memories.
        </p>
      ) : report.projects.length === 0 ? (
        <MemoriesEmptyState />
      ) : (
        <ScrollPane className="min-h-0" viewportLabel="Memories">
          <div
            aria-busy={state.loading || undefined}
            className={cn(
              "grid gap-y-4 px-8 pt-6 pb-16 transition-opacity duration-[var(--duration-medium)]",
              state.loading && "opacity-60",
            )}
          >
            <p className="justify-self-end type-footnote text-label-tertiary">
              {`${memoryCount} ${memoryCount === 1 ? "memory" : "memories"} in ${
                report.projects.length
              } ${report.projects.length === 1 ? "project" : "projects"} · updated ${relativeTime(
                new Date(report.generatedAtMs).toISOString(),
                { now },
              )}`}
            </p>
            {report.projects.map((project) => (
              <MemoryProjectGroup
                key={project.slug}
                project={project}
                collapsed={state.collapsedProjects.has(project.slug)}
                state={state}
                writesSupported={report.writesSupported}
                now={now}
                onToggleProject={() => session.toggleProject(project.slug)}
                onToggleMemory={session.toggleMemory}
                onReveal={(path) => void session.reveal(path)}
                onDelete={(entry) => void session.archive(project, entry)}
                onUndo={(entry) => void session.undo(entry)}
                onRemoveLine={(entry) => void session.removeIndexLine(project, entry)}
                onReload={session.refresh}
                onFocusHandled={session.focusHandled}
              />
            ))}
          </div>
        </ScrollPane>
      )}
    </div>
  )
}
