import { FileText, Folder, GitBranch } from "lucide-react"
import { ToggleSwitch } from "../components/ui/ToggleSwitch"
import {
  cloudDisplayPath,
  type CloudProjectChoices,
  type CloudProjectGroup,
} from "../lib/cloud"

/** Show the main path and counts. Keep raw paths as selection authority. */
export function CloudProjectFolderSummary({
  group,
  homeDirectory,
  id,
}: {
  group: CloudProjectGroup
  homeDirectory?: string | null | undefined
  /** Connect the row's selection control to its path and counts. */
  id?: string
}) {
  const sessionLabel =
    group.sessionCount == null
      ? "Count unavailable"
      : `${group.sessionCount} ${group.sessionCount === 1 ? "session" : "sessions"}`
  const worktreeLabel =
    group.worktreeCount > 0
      ? `${group.worktreeCount} ${group.worktreeCount === 1 ? "worktree" : "worktrees"}`
      : null
  return (
    <div
      id={id}
      className="flex min-w-0 items-center gap-2 type-callout text-label-secondary"
      aria-description={[group.path, sessionLabel, worktreeLabel].filter(Boolean).join(" · ")}
    >
      <span className="min-w-0 truncate" title={group.path}>
        {cloudDisplayPath(group.path, homeDirectory)}
      </span>
      <span aria-hidden>·</span>
      <span className="cloud-folder-count inline-flex shrink-0 items-center gap-1">
        <FileText size={12} aria-hidden />
        <span className="whitespace-nowrap">{sessionLabel}</span>
      </span>
      {worktreeLabel && (
        <>
          <span aria-hidden>·</span>
          <span className="inline-flex shrink-0 items-center gap-1 whitespace-nowrap">
            <GitBranch size={12} aria-hidden />
            <span className="whitespace-nowrap">{worktreeLabel}</span>
          </span>
        </>
      )}
    </div>
  )
}

/** Show one local choice per project. Only Start saves the current selection.
 * Pass an empty list only after a complete, successful discovery.
 */
export function CloudProjectFolders({
  groups,
  choices,
  disabled = false,
  homeDirectory,
  onSelect,
}: {
  groups: readonly CloudProjectGroup[]
  choices: CloudProjectChoices
  disabled?: boolean
  homeDirectory?: string | null | undefined
  onSelect: (projectId: string, selected: boolean) => void
}) {
  return (
    <div
      className="cloud-folder-list min-h-0! max-h-none! flex-1"
      aria-label="Cloud project folders"
    >
      {groups.length ? (
        groups.map((group) => (
          <div key={group.id} className="cloud-folder-row items-start!">
            <Folder size={16} className="mt-0.5 shrink-0" aria-hidden />
            <div className="min-w-0 flex-1">
              <strong className="block truncate type-title-3 font-medium!">{group.name}</strong>
              <CloudProjectFolderSummary
                id={`cloud-folder-${encodeURIComponent(group.id)}`}
                group={group}
                homeDirectory={homeDirectory}
              />
            </div>
            <ToggleSwitch
              aria-label={`Sync ${group.name}: ${group.path}`}
              aria-describedby={`cloud-folder-${encodeURIComponent(group.id)}`}
              checked={choices[group.id] ?? true}
              disabled={disabled}
              onCheckedChange={(selected) => onSelect(group.id, selected)}
            />
          </div>
        ))
      ) : (
        <p className="cloud-empty type-body text-onboarding-muted">
          No project folders found in your available sessions.
        </p>
      )}
    </div>
  )
}
