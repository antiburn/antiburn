import { ChevronDown, FileText, Folder, GitBranch } from "lucide-react"
import { ToggleSwitch } from "../components/ui/ToggleSwitch"
import {
  cloudDisplayPath,
  type CloudProjectChoices,
  type CloudProjectGroup,
  type CloudWorkspace,
} from "../lib/cloud"

const chooseWorkspace = ":choose-workspace"

/** Label a scan count without implying that the scan covered the whole folder. */
export function folderSessionsFoundLabel(count: number) {
  return `${count} ${count === 1 ? "session" : "sessions"} found`
}

/** Show the main path and counts. Keep raw paths as selection authority. */
export function CloudProjectFolderSummary({
  group,
  homeDirectory,
  id,
  stackPath = false,
  syncFound,
}: {
  group: CloudProjectGroup
  homeDirectory?: string | null | undefined
  /** Connect the row's selection control to its path and counts. */
  id?: string
  /** Give the full path a wrapping line above the counts. */
  stackPath?: boolean
  /** Use a verified sync scan count instead of the local index count. */
  syncFound?: number | undefined
}) {
  const sessionLabel = group.discoveryStale
    ? "Folder details need refresh"
    : syncFound !== undefined
      ? folderSessionsFoundLabel(syncFound)
      : group.sessionCount == null
        ? "Calculating count.."
        : `${group.sessionCount} ${group.sessionCount === 1 ? "session" : "sessions"}`
  const worktreeLabel =
    !group.discoveryStale && group.groupingComplete !== false && group.worktreeCount > 0
      ? `${group.worktreeCount} ${group.worktreeCount === 1 ? "worktree" : "worktrees"}`
      : null
  return (
    <div
      id={id}
      className={`flex min-w-0 items-center type-callout text-label-secondary${stackPath ? " flex-wrap gap-x-2 gap-y-1" : " gap-2"}`}
      aria-description={[group.path, sessionLabel, worktreeLabel].filter(Boolean).join(" · ")}
    >
      <span
        className={stackPath ? "min-w-0 basis-full wrap-anywhere" : "min-w-0 truncate"}
        title={group.path}
      >
        {cloudDisplayPath(group.path, homeDirectory)}
      </span>
      {!stackPath && <span aria-hidden>·</span>}
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
  busy = false,
  workspaces,
  destinationId,
  selectedWorkspace,
  typedWorkspaceLabels = false,
  wrapWorkspaceLabel = false,
  needsWorkspaceChoice = false,
  keepWorkspacePicker = false,
  onSelect,
}: {
  label: string
  descriptionId: string
  selected: boolean
  disabled: boolean
  /** Disable changes during sync. A switch can keep its current focus. */
  busy?: boolean
  workspaces?: readonly CloudWorkspace[] | undefined
  destinationId?: string | undefined
  selectedWorkspace?: string | undefined
  /** Show the workspace type and limit dropdown width without changing selection behavior. */
  typedWorkspaceLabels?: boolean
  /** Wrap the selected label while keeping the native select as the only control. */
  wrapWorkspaceLabel?: boolean
  /** Ask for a destination when a checked draft has no workspace binding. */
  needsWorkspaceChoice?: boolean
  /** Keep a staged change editable when a workspace disappears from the list. */
  keepWorkspacePicker?: boolean
  onSelect: (selected: boolean, destination?: string) => void
}) {
  const unavailableChoice =
    selectedWorkspace && !workspaces?.some((workspace) => workspace.id === selectedWorkspace)
  if (
    destinationId &&
    (keepWorkspacePicker || (workspaces?.length ?? 0) > 1 || unavailableChoice)
  ) {
    const value = needsWorkspaceChoice
      ? chooseWorkspace
      : selected
        ? (selectedWorkspace ?? destinationId)
        : ""
    const workspaceLabel = (workspace: CloudWorkspace) =>
      typedWorkspaceLabels ? cloudWorkspaceLabel(workspace) : workspace.name
    const options = [
      { id: "", label: "Don't sync" },
      ...(needsWorkspaceChoice
        ? [{ id: chooseWorkspace, label: "Choose a workspace", disabled: true }]
        : []),
      ...(unavailableChoice
        ? [{ id: selectedWorkspace, label: "Selected workspace unavailable", disabled: true }]
        : []),
      ...(workspaces?.map((workspace) => ({
        id: workspace.id,
        label: workspaceLabel(workspace),
      })) ?? []),
    ]
    const picker = (
      <select
        aria-label={label}
        aria-describedby={descriptionId}
        className={
          wrapWorkspaceLabel
            ? "absolute inset-0 h-full w-full cursor-pointer opacity-0 disabled:cursor-default"
            : `rounded-control bg-surface-card px-2 py-1 type-footnote text-label${typedWorkspaceLabels ? " min-h-8 min-w-0 max-w-[min(100%,12rem)] truncate" : ""}`
        }
        value={value}
        disabled={disabled || busy}
        onChange={(event) => {
          const target = event.currentTarget.value
          if (target === "") onSelect(false)
          else if (workspaces?.some((workspace) => workspace.id === target))
            onSelect(true, target)
        }}
      >
        {options.map((option) => (
          <option key={option.id} value={option.id} disabled={"disabled" in option}>
            {option.label}
          </option>
        ))}
      </select>
    )
    if (!wrapWorkspaceLabel) return picker
    return (
      <div
        className={`relative flex min-h-8 min-w-0 max-w-full items-center gap-2 rounded-control bg-surface-card px-2 py-1 type-footnote text-label has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-accent-fill ${disabled || busy ? "opacity-50" : ""}`}
      >
        <span className="min-w-0 wrap-anywhere" aria-hidden="true">
          {options.find(({ id }) => id === value)?.label}
        </span>
        <ChevronDown size={13} className="shrink-0" aria-hidden="true" />
        {picker}
      </div>
    )
  }
  return (
    <ToggleSwitch
      aria-label={label}
      aria-describedby={descriptionId}
      checked={selected}
      disabled={disabled}
      aria-disabled={busy}
      onCheckedChange={onSelect}
    />
  )
}

/** Distinguish workspace types, even when their names match. */
export function cloudWorkspaceLabel({ kind, name }: Pick<CloudWorkspace, "kind" | "name">) {
  if (kind === "team") return `Team workspace: ${name}`
  return name.trim().toLowerCase() === "personal"
    ? "Personal workspace"
    : `Personal workspace: ${name}`
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
  savedDestinations,
  id,
  onSelect,
}: {
  groups: readonly CloudProjectGroup[]
  choices: CloudProjectChoices
  disabled?: boolean
  homeDirectory?: string | null | undefined
  workspaces?: readonly CloudWorkspace[] | undefined
  destinationId?: string | undefined
  targets?: Readonly<Record<string, string>> | undefined
  /** Name the saved destination for folders outside the onboarding batch. */
  savedDestinations?: ReadonlyMap<string, string>
  /** Focus this list when the last page removes its Load more button. */
  id?: string
  onSelect: (projectId: string, selected: boolean, destination?: string) => void
}) {
  return (
    <div
      id={id}
      tabIndex={id ? -1 : undefined}
      className="cloud-folder-list min-h-0! max-h-none! flex-1"
      role="group"
      aria-label="Cloud project folders"
    >
      {groups.length ? (
        groups.map((group) => {
          const descriptionId = `cloud-folder-${encodeURIComponent(group.id)}`
          const target = targets?.[group.id]
          const unavailable =
            !!target && !workspaces?.some((workspace) => workspace.id === target)
          const showPicker = !!destinationId && ((workspaces?.length ?? 0) > 1 || unavailable)
          const selected = choices[group.id] ?? !showPicker
          const unverified = group.discoveryStale || group.groupingComplete === false
          const savedDestination = savedDestinations?.get(group.id)
          return (
            <div
              key={group.id}
              className={`cloud-folder-row items-start!${showPicker ? " flex-wrap" : ""}`}
            >
              <Folder size={16} className="mt-0.5 shrink-0" aria-hidden />
              <div className="min-w-0 flex-1">
                <strong className="block truncate type-title-3 font-medium!">
                  {group.name}
                </strong>
                <CloudProjectFolderSummary
                  id={descriptionId}
                  group={group}
                  homeDirectory={homeDirectory}
                />
              </div>
              {savedDestination ? (
                <span className="basis-full type-callout text-onboarding-muted">
                  Sync is already set up for {savedDestination}. Manage it in Settings.
                </span>
              ) : (
                <div
                  className={
                    showPicker
                      ? "flex min-w-0 basis-full items-center gap-2 sm:basis-auto"
                      : undefined
                  }
                >
                  {showPicker && (
                    <span className="type-callout text-onboarding-muted">Sync destination</span>
                  )}
                  <CloudFolderSyncControl
                    label={`${showPicker ? "Sync destination for" : "Sync"} ${group.name}: ${group.path}`}
                    descriptionId={descriptionId}
                    selected={
                      !unverified &&
                      selected &&
                      (!showPicker || group.captureSupported === true)
                    }
                    disabled={disabled || unverified || (showPicker && !group.captureSupported)}
                    workspaces={workspaces}
                    destinationId={destinationId}
                    selectedWorkspace={target}
                    typedWorkspaceLabels
                    needsWorkspaceChoice={
                      showPicker && selected && group.captureSupported === true && !target
                    }
                    onSelect={(enabled, workspace) => onSelect(group.id, enabled, workspace)}
                  />
                </div>
              )}
            </div>
          )
        })
      ) : (
        <p className="cloud-empty type-body text-onboarding-muted">
          No project folders found in your available sessions.
        </p>
      )}
    </div>
  )
}
