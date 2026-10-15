import type { CalledToolPayload } from "../../../lib/sessionIpc"

export interface CalledToolsProps {
  /** The tools and skills the session used, most-used first. */
  tools: CalledToolPayload[]
}

/**
 * The Tools tab's fallback reading for a session with no sized startup
 * context: the tools the session called and the skills it used, and how often.
 *
 * It names its own limit. Without a sized startup context there is no list of
 * what the session loaded, so nothing here can price a tool that sat in every
 * request and was never called.
 *
 * Skill uses are not tool calls. A session can load a skill through a skill
 * tool or through a read of the skill file, so skills get their own list.
 */
export function CalledTools({ tools }: CalledToolsProps) {
  const toolCalls = tools.filter((tool) => tool.kind === "tool")
  const skillUses = tools.filter((tool) => tool.kind === "skill")

  return (
    <div className="flex flex-col gap-y-3">
      <div className="flex flex-col gap-y-1">
        <h3 className="type-headline text-label">Tools and skills used</h3>
        <p className="type-callout text-label-tertiary">
          This session has no startup context with sizes, so the cost of items that were loaded
          but never called is unavailable. These are the tools the session called and the skills
          it used. A read that loads a skill counts as a skill use, not as a tool call.
        </p>
      </div>
      {toolCalls.length > 0 && (
        <UsageList label="Tool" countLabel="Calls" entries={toolCalls} />
      )}
      {skillUses.length > 0 && (
        <UsageList label="Skill" countLabel="Uses" entries={skillUses} />
      )}
    </div>
  )
}

function UsageList({
  label,
  countLabel,
  entries,
}: {
  label: string
  countLabel: string
  entries: CalledToolPayload[]
}) {
  return (
    <div className="grid min-w-0 grid-cols-[1fr_auto] gap-x-6 gap-y-1 rounded-control bg-surface-card/50 px-3 py-2">
      <span className="type-caption text-label-secondary">{label}</span>
      <span className="type-caption text-right text-label-secondary">{countLabel}</span>
      {entries.map((entry) => (
        <div
          key={entry.name}
          className="col-span-full -mx-1 grid grid-cols-subgrid rounded-control px-1 py-0.5 type-callout transition-colors duration-[var(--duration-fast)] ease-out hover:bg-surface-hover"
        >
          <span className="min-w-0 truncate text-label">{entry.name}</span>
          <span className="text-right tabular-nums text-label-secondary">{entry.calls}</span>
        </div>
      ))}
    </div>
  )
}
