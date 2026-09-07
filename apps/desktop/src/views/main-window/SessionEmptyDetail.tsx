import { MessagesSquare } from "lucide-react"
import { isMacOS } from "../../lib/platform"

export function SessionEmptyDetail() {
  return (
    <div className="relative flex min-h-0 flex-1 items-center justify-center overflow-auto p-6 text-center">
      {isMacOS() && (
        <div className="main-window-empty-titlebar" data-tauri-drag-region aria-hidden="true" />
      )}
      <div className="flex max-w-xs flex-col items-center">
        <div
          className="mb-4 flex h-12 w-12 shrink-0 items-center justify-center rounded-full bg-surface-secondary text-label-tertiary"
          aria-hidden="true"
        >
          <MessagesSquare size={24} strokeWidth={1.5} />
        </div>
        <h3 className="type-title-2 text-balance text-label">No session selected</h3>
        <p className="mt-2 type-body text-pretty text-label-secondary">
          Choose a session from the list to explore its activity, cost, and burn checks.
        </p>
      </div>
    </div>
  )
}
