import { relativeTime } from "../../../lib/presentation/relativeTime"
import type { SessionMemoriesPayload, SessionMemoryTouch } from "../../../lib/memoriesIpc"

export interface SessionMemoriesProps {
  sessionMemories: SessionMemoriesPayload | null
  /** Open one memory in the Memories view. Omitted where there is no such view. */
  onOpenMemory?: ((target: { slug: string; path: string }) => void) | undefined
}

function actionLabel(entry: SessionMemoryTouch): string {
  const word = entry.action === "written" ? "written" : "read"
  return entry.count > 1 ? `${word} ×${entry.count}` : word
}

/** The memories one session read or wrote, one row per file and action. */
export function SessionMemories({ sessionMemories, onOpenMemory }: SessionMemoriesProps) {
  if (!sessionMemories || sessionMemories.entries.length === 0) return null
  return (
    <section className="shrink-0">
      <h4 className="mb-1.5 type-caption text-label-tertiary">Memories touched</h4>
      <ul className="grid grid-cols-[minmax(0,1fr)_auto_auto_auto] gap-x-4 gap-y-1">
        {sessionMemories.entries.map((entry) => (
          <li
            key={`${entry.path}:${entry.action}`}
            className="col-span-full grid grid-cols-subgrid items-baseline"
          >
            <span className="flex min-w-0 flex-col">
              <span className="truncate type-body text-label">{entry.title}</span>
              <span className="truncate font-mono type-footnote text-label-tertiary">
                {entry.fileName}
              </span>
            </span>
            <span
              className="type-footnote text-label-secondary"
              title="Counts Read calls and shell commands that name the file"
            >
              {actionLabel(entry)}
            </span>
            <span className="font-mono type-footnote tabular-nums text-label-secondary">
              {entry.lastMs === null
                ? "—"
                : relativeTime(new Date(entry.lastMs).toISOString(), { compact: true })}
            </span>
            <span className="type-footnote">
              {!entry.exists ? (
                <span className="text-label-tertiary">deleted</span>
              ) : onOpenMemory ? (
                <button
                  type="button"
                  className="text-accent hover:underline"
                  onClick={() => onOpenMemory({ slug: entry.slug, path: entry.path })}
                >
                  Show in Memories
                </button>
              ) : null}
            </span>
          </li>
        ))}
      </ul>
    </section>
  )
}
