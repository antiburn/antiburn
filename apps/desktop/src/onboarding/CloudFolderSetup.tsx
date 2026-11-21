import { Folder } from "lucide-react"
import { useId } from "react"
import { ToggleSwitch } from "../components/ui/ToggleSwitch"
import { cloudDisplayPath } from "../lib/cloud"
import "./onboarding.css"
import "./cloud.css"

interface FolderDestination {
  id: string
  name: string
  kind: "personal" | "team"
}

interface LocalFolder {
  id: string
  path: string
  name?: string
  sessionCount?: number | null
  worktreeCount?: number
  captureSupported?: boolean
}

/** The caller owns the draft, discovery, and all consent and upload operations. */
export interface CloudFolderSetupProps {
  /** Current verified destinations. IDs must be unique and nonempty. */
  destinations: readonly FolderDestination[]
  /** Current local folder groups. Counts describe discovery, not Cloud uploads. */
  folders: readonly LocalFolder[]
  /** Omitted means untouched; null means explicitly off. Keep removed IDs until edited. */
  choices: Readonly<Partial<Record<string, string | null>>>
  homeDirectory?: string | null | undefined
  loading?: boolean
  busy?: boolean
  /** Set this when a service or discovery error prevents a safe start. */
  startBlocked?: boolean
  message?: string | null
  onChoiceChange: (folderId: string, destinationId: string | null) => void
  /** Revalidate these visible assignments against current discovery and account state. */
  onStart: (assignments: readonly { folderId: string; destinationId: string }[]) => void
  /** Leave setup without saving the draft or enabling uploads. */
  onFinish: () => void
}

const countLabel = (count: number, label: string) =>
  `${count.toLocaleString()} ${label}${count === 1 ? "" : "s"}`

const focusHeading = (heading: HTMLHeadingElement | null) => heading?.focus()

/** Render and confirm a draft without saving preferences or starting sync. */
export function CloudFolderSetup({
  destinations,
  folders,
  choices,
  homeDirectory,
  loading = false,
  busy = false,
  startBlocked = false,
  message,
  onChoiceChange,
  onStart,
  onFinish,
}: CloudFolderSetupProps) {
  const id = useId()
  const options = [
    ...destinations.filter((item) => item.kind === "personal"),
    ...destinations.filter((item) => item.kind === "team"),
  ]
  const personal = options.length === 1 && options[0]?.kind === "personal" ? options[0] : null
  const destinationIds = new Set(options.map((item) => item.id))
  const rows = folders.map((folder) => {
    const destinationId = Object.hasOwn(choices, folder.id)
      ? (choices[folder.id] ?? null)
      : null
    const removed = destinationId !== null && !destinationIds.has(destinationId)
    const unsupported = folder.captureSupported === false
    return { folder, destinationId, removed, unsupported }
  })
  const unresolved = rows.some(
    ({ destinationId, removed, unsupported }) =>
      destinationId !== null && (removed || unsupported),
  )
  const assignments = rows.flatMap(({ folder, destinationId, removed, unsupported }) =>
    destinationId !== null && !removed && !unsupported
      ? [{ folderId: folder.id, destinationId }]
      : [],
  )
  const canStart = !loading && !busy && !startBlocked && !unresolved && assignments.length > 0

  return (
    <section className="cloud-folder-content min-h-0 flex-1 overflow-hidden">
      <h1 className="type-cloud-folder-title" tabIndex={-1} ref={focusHeading}>
        Choose folders to sync
      </h1>
      <p id={`${id}-consent`} className="type-onboarding-body text-onboarding-muted">
        Start syncing uploads full sessions available now and new sessions from the selected
        folders and their worktrees to the destinations shown.
      </p>
      <p id={`${id}-selection`} className="type-callout text-onboarding-muted" role="status">
        {countLabel(assignments.length, "folder")} selected
      </p>
      {rows.length > 0 ? (
        <ul
          className="cloud-folder-list min-h-0! max-h-none! flex-1 list-none p-0"
          aria-label="Local project folders"
        >
          {rows.map(({ folder, destinationId, removed, unsupported }, index) => {
            const name =
              folder.name ?? (folder.path.split(/[\\/]/).filter(Boolean).at(-1) || folder.path)
            const details = `${id}-folder-${index}`
            const label = `${name}: ${folder.path}`
            const showSelect =
              (!personal || removed || unsupported) &&
              (options.length > 0 || destinationId !== null)
            const canEdit = !loading && !busy && (!unsupported || destinationId !== null)
            return (
              <li key={folder.id} className="cloud-folder-row flex-wrap items-start!">
                <Folder size={16} className="mt-0.5 shrink-0" aria-hidden />
                <div className="min-w-0 flex-1 basis-40">
                  <strong className="block break-words type-title-3">{name}</strong>
                  <div
                    id={details}
                    className="flex min-w-0 flex-wrap items-center gap-x-2 type-callout text-label-secondary"
                  >
                    <span className="max-w-full truncate" title={folder.path}>
                      {cloudDisplayPath(folder.path, homeDirectory)}
                    </span>
                    <span className="whitespace-nowrap">
                      {folder.sessionCount == null
                        ? "Session count unavailable"
                        : countLabel(folder.sessionCount, "session")}
                    </span>
                    {!!folder.worktreeCount && (
                      <span className="whitespace-nowrap">
                        {countLabel(folder.worktreeCount, "worktree")}
                      </span>
                    )}
                    {removed && !loading && (
                      <span className="w-full text-system-red-text">
                        Destination no longer available
                      </span>
                    )}
                    {unsupported && (
                      <span className="w-full">Uploads unavailable for this folder</span>
                    )}
                  </div>
                </div>
                {showSelect ? (
                  <select
                    className="ui-push-button min-h-8 w-40 max-w-full text-ellipsis"
                    aria-label={`Destination for ${label}`}
                    aria-describedby={details}
                    value={destinationId ?? ""}
                    disabled={!canEdit}
                    onChange={(event) => {
                      const next = event.currentTarget.value
                      if (!next || (!unsupported && destinationIds.has(next))) {
                        onChoiceChange(folder.id, next || null)
                      }
                    }}
                  >
                    <option value="">Don’t sync</option>
                    {removed && destinationId !== null && (
                      <option value={destinationId} disabled>
                        {loading ? "Checking destination…" : "Unavailable destination"}
                      </option>
                    )}
                    {options.map((item) => (
                      <option key={item.id} value={item.id} disabled={unsupported}>
                        {item.kind === "personal" ? "Personal" : item.name}
                      </option>
                    ))}
                  </select>
                ) : personal ? (
                  <div className="flex items-center gap-2 type-callout text-label-secondary">
                    <span aria-hidden>Personal</span>
                    <ToggleSwitch
                      aria-label={`Sync ${label} to Personal`}
                      aria-describedby={details}
                      checked={destinationId === personal.id}
                      disabled={!canEdit}
                      onCheckedChange={(checked) =>
                        onChoiceChange(folder.id, checked ? personal.id : null)
                      }
                    />
                  </div>
                ) : null}
              </li>
            )
          })}
        </ul>
      ) : (
        <p className="cloud-folder-list cloud-empty min-h-0! max-h-none! flex-1 type-body text-onboarding-muted">
          {loading
            ? "Loading folders…"
            : message
              ? "No folders loaded."
              : "No local project folders found."}
        </p>
      )}
      {message ? (
        <p className="type-callout text-onboarding-muted" role="status">
          {message}
        </p>
      ) : !loading && unresolved ? (
        <p className="type-callout text-onboarding-muted">
          Update unavailable choices before starting sync.
        </p>
      ) : !loading && options.length === 0 ? (
        <p className="type-callout text-onboarding-muted">
          No Cloud destinations are available.
        </p>
      ) : null}
      <footer className="flex flex-wrap items-center justify-end gap-3">
        <button
          type="button"
          className="onboarding-secondary"
          disabled={busy}
          onClick={onFinish}
        >
          Finish without syncing
        </button>
        <button
          type="button"
          className="onboarding-primary"
          aria-describedby={`${id}-consent ${id}-selection`}
          disabled={!canStart}
          onClick={() => onStart(assignments)}
        >
          Start syncing
        </button>
      </footer>
    </section>
  )
}
