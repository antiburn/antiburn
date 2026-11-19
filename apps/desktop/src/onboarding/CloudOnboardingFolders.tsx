import { FileText, Folder, GitBranch } from "lucide-react"
import { ToggleSwitch } from "../components/ui/ToggleSwitch"
import {
  cloudDisplayPath,
  type CloudProjectChoices,
  type CloudProjectGroup,
  type CloudWorkspace,
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

/** Choose this folder's destination. The caller owns approval and content consent. */
export function CloudFolderSyncControl({
  label,
  descriptionId,
  selected,
  disabled,
  workspaces,
  destinationId,
  selectedWorkspace,
  onWorkspaceSelect,
  onSelect,
}: {
  label: string
  descriptionId: string
  selected: boolean
  disabled: boolean
  workspaces?: readonly CloudWorkspace[] | undefined
  destinationId?: string | undefined
  selectedWorkspace?: string | undefined
  onWorkspaceSelect?: ((id: string) => void) | undefined
  onSelect: (selected: boolean) => void
}) {
  const unavailableChoice =
    selectedWorkspace && !workspaces?.some((workspace) => workspace.id === selectedWorkspace)
  if (destinationId && ((workspaces?.length ?? 0) > 1 || unavailableChoice)) {
    return (
      <select
        aria-label={label}
        aria-describedby={descriptionId}
        className="rounded-control bg-surface-card px-2 py-1 type-footnote text-label"
        value={selected ? (selectedWorkspace ?? destinationId) : ""}
        disabled={disabled}
        onChange={(event) => {
          const target = event.currentTarget.value
          if (target === "" || target === destinationId) onSelect(target === destinationId)
          else if (workspaces?.some((workspace) => workspace.id === target))
            onWorkspaceSelect?.(target)
        }}
      >
        <option value="">Don't sync</option>
        {unavailableChoice && (
          <option value={selectedWorkspace} disabled>
            Selected workspace unavailable
          </option>
        )}
        {workspaces?.map((workspace) => (
          <option
            key={workspace.id}
            value={workspace.id}
            disabled={workspace.id !== destinationId && !onWorkspaceSelect}
          >
            {workspace.name}
            {workspace.id !== destinationId && !onWorkspaceSelect
              ? " (uploads unavailable)"
              : ""}
          </option>
        ))}
      </select>
    )
  }
  return (
    <ToggleSwitch
      aria-label={label}
      aria-describedby={descriptionId}
      checked={selected}
      disabled={disabled}
      onCheckedChange={onSelect}
    />
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
  workspaces,
  destinationId,
  targets,
  onWorkspaceSelect,
  onSelect,
}: {
  groups: readonly CloudProjectGroup[]
  choices: CloudProjectChoices
  disabled?: boolean
  homeDirectory?: string | null | undefined
  workspaces?: readonly CloudWorkspace[] | undefined
  destinationId?: string | undefined
  targets?: Readonly<Record<string, string>> | undefined
  onWorkspaceSelect?: ((project: string, workspace: string) => void) | undefined
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
            <CloudFolderSyncControl
              label={`Sync ${group.name}: ${group.path}`}
              descriptionId={`cloud-folder-${encodeURIComponent(group.id)}`}
              selected={choices[group.id] ?? true}
              disabled={disabled}
              workspaces={workspaces}
              destinationId={destinationId}
              selectedWorkspace={targets?.[group.id]}
              onWorkspaceSelect={onWorkspaceSelect && ((id) => onWorkspaceSelect(group.id, id))}
              onSelect={(selected) => onSelect(group.id, selected)}
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
