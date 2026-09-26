import type { CalledToolPayload } from "../../../lib/sessionIpc"

export interface CalledToolsProps {
  /** The tools the session called, most-called first. */
  tools: CalledToolPayload[]
}

/**
 * The Tools tab's fallback reading for a session with no sized startup
 * context: the tools the session called, and how often.
 *
 * It names its own limit. Without a sized startup context there is no list of
 * what the session loaded, so nothing here can price a tool that sat in every
 * request and was never called.
 */
export function CalledTools({ tools }: CalledToolsProps) {
  if (tools.length === 0) return null

  return (
    <div className="flex flex-col gap-y-3">
      <div className="flex flex-col gap-y-1">
        <h3 className="type-headline text-label">Tools called</h3>
        <p className="type-callout text-label-tertiary">
          This session has no startup context with sizes, so the cost of items that were loaded
          but never called is unavailable. These are the tools the session did call.
        </p>
      </div>
      <div className="grid min-w-0 grid-cols-[1fr_auto] gap-x-6 gap-y-1 rounded-control bg-surface-card/50 px-3 py-2">
        <span className="type-caption text-label-secondary">Tool</span>
        <span className="type-caption text-right text-label-secondary">Calls</span>
        {tools.map((tool) => (
          <div
            key={tool.name}
            className="col-span-full -mx-1 grid grid-cols-subgrid rounded-control px-1 py-0.5 type-callout transition-colors duration-[var(--duration-fast)] ease-out hover:bg-surface-hover"
          >
            <span className="min-w-0 truncate text-label">{tool.name}</span>
            <span className="text-right tabular-nums text-label-secondary">{tool.calls}</span>
          </div>
        ))}
      </div>
    </div>
  )
}
