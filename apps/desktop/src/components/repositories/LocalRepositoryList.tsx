import { FolderSearch, Lock, RefreshCw, Search } from "lucide-react"
import type { ReactNode } from "react"

import type { LocalRepositoryItem, LocalRepositoryStatus } from "../../lib/types/repository"
import { Tooltip } from "../presentation/Tooltip"
import { WslOriginBadge } from "../presentation/WslOriginBadge"
import { PushButton } from "../ui/PushButton"
import { ScrollPane } from "../ui/ScrollPane"
import { Skeleton } from "../ui/Skeleton"
import { ToggleSwitch } from "../ui/ToggleSwitch"

interface RepositoryUndo {
  /** What was just changed, phrased as a completed action. */
  label: string
  onUndo: () => void
}

export interface LocalRepositoryListProps {
  repositories: LocalRepositoryItem[]
  /** Whether a scan is in flight. Shows placeholder rows on the first one. */
  loading?: boolean
  /** Include or ignore one repository. Omitted makes the list read-only. */
  onToggleRepository?: (item: LocalRepositoryItem, enabled: boolean) => void
  /** Re-scan for repositories. Omitted hides the control. */
  onRefresh?: () => void
  /** Point the app at a repository it could not find. Omitted hides the control. */
  onLocate?: (item: LocalRepositoryItem) => void
  /** Ask the operating system for access to a blocked repository. */
  onGrantAccess?: (item: LocalRepositoryItem) => void
  /** The last reversible change, shown as a transient bar under the header. */
  undo?: RepositoryUndo | null
  emptyTitle?: string
  emptyDescription?: string
  /** Glyph for the WSL-origin badge. */
  wslIcon?: ReactNode
}

const STATUS_NOTE: Record<LocalRepositoryStatus, string | null> = {
  accessible: null,
  permission_denied: "Blocked by the system",
  not_cloned: "Not on this machine",
  disabled: null,
}

/** How many placeholder rows the first scan shows. */
const PLACEHOLDER_ROWS = 3

function RepositoryRow({
  item,
  onToggleRepository,
  onLocate,
  onGrantAccess,
  wslIcon,
}: {
  item: LocalRepositoryItem
  onToggleRepository?: (item: LocalRepositoryItem, enabled: boolean) => void
  onLocate?: (item: LocalRepositoryItem) => void
  onGrantAccess?: (item: LocalRepositoryItem) => void
  wslIcon?: ReactNode
}) {
  const blocked = item.status === "permission_denied"
  const missing = item.status === "not_cloned"
  const path = item.repoRoot ?? item.suspectedPath ?? null
  const note = STATUS_NOTE[item.status]

  const facts: string[] = []
  if (item.worktreeCount) {
    facts.push(`${item.worktreeCount} worktree${item.worktreeCount === 1 ? "" : "s"}`)
  }
  if (item.sessionCount) {
    facts.push(`${item.sessionCount} session${item.sessionCount === 1 ? "" : "s"}`)
  }

  return (
    <div className="flex items-start gap-3 rounded-md py-2 transition-colors duration-[var(--duration-fast)] ease-out hover:bg-surface-hover">
      <div className="min-w-0 flex-1 space-y-0.5">
        <div className="flex min-w-0 items-center gap-1.5">
          <span className="truncate type-callout text-label">{item.fullName}</span>
          <WslOriginBadge distro={item.wslDistro} {...(wslIcon ? { icon: wslIcon } : {})} />
          {blocked && (
            <Tooltip label="The system is blocking access to this folder">
              <span
                className="inline-flex shrink-0 text-system-orange"
                aria-label="Access blocked"
              >
                <Lock size={12} aria-hidden="true" />
              </span>
            </Tooltip>
          )}
        </div>

        {/* The path is the disambiguator: two clones of one repository differ
            by nothing else. It leads with the end of the path, which is the
            part that differs, by eliding from the left. */}
        {path && (
          <p
            dir="rtl"
            title={path}
            className="truncate text-left type-footnote text-label-secondary"
          >
            <bdi>{path}</bdi>
          </p>
        )}

        {(note || facts.length > 0) && (
          <p className="type-caption text-label-tertiary">
            {[note, ...facts].filter(Boolean).join(" · ")}
          </p>
        )}
      </div>

      <div className="flex shrink-0 items-center gap-1.5">
        {blocked && onGrantAccess && (
          <PushButton onClick={() => onGrantAccess(item)}>Grant access</PushButton>
        )}
        {missing && onLocate && <PushButton onClick={() => onLocate(item)}>Locate…</PushButton>}
        {/* A repository that is not on this machine has nothing to include,
            so it gets no switch at all. Everything else shows one, disabled
            when the host cannot act on it: the switch states a fact about the
            repository, and hiding it would hide the fact along with the
            control. */}
        {!missing && (
          <ToggleSwitch
            checked={item.enabled}
            onCheckedChange={(next) => onToggleRepository?.(item, next)}
            disabled={!onToggleRepository}
            aria-label={`Include ${item.fullName}`}
          />
        )}
      </div>
    </div>
  )
}

/**
 * The repositories this machine knows about, and which of them the app watches.
 *
 * Inclusion is opt-out: a repository found on disk is watched unless the reader
 * turns it off, so a fresh install is useful without a setup pass. The three
 * states that are not simply "found" each carry their own recovery — a blocked
 * folder can be granted access, a repository that is not here can be located —
 * rather than being reported as a failure with nothing to do about it.
 */
export function LocalRepositoryList({
  repositories,
  loading = false,
  onToggleRepository,
  onRefresh,
  onLocate,
  onGrantAccess,
  undo,
  emptyTitle = "No repositories found",
  emptyDescription = "Repositories appear here once a coding session runs in one, or after a scan finds one on this machine.",
  wslIcon,
}: LocalRepositoryListProps) {
  const showPlaceholders = loading && repositories.length === 0

  return (
    <div className="flex h-full flex-col">
      {onRefresh && (
        <div className="flex h-6 items-center gap-3 px-4 pt-3 pb-2">
          <button
            type="button"
            onClick={onRefresh}
            disabled={loading}
            aria-label="Rescan for repositories"
            className="-mr-1.5 ml-auto inline-flex h-6 shrink-0 items-center gap-1 rounded-control px-1.5 type-footnote text-label-secondary hover:bg-surface-hover disabled:opacity-50"
          >
            <RefreshCw
              size={12}
              strokeWidth={2}
              aria-hidden="true"
              className={loading ? "animate-spin" : undefined}
            />
            <span>Rescan</span>
          </button>
        </div>
      )}

      {undo && (
        <div className="mx-4 mb-2 flex items-center gap-2 rounded-control bg-surface-secondary px-2 py-1.5">
          <span className="min-w-0 flex-1 truncate type-footnote text-label-secondary">
            {undo.label}
          </span>
          <button
            type="button"
            onClick={undo.onUndo}
            className="shrink-0 type-footnote font-medium text-accent hover:underline"
          >
            Undo
          </button>
        </div>
      )}

      <ScrollPane>
        {showPlaceholders ? (
          <div aria-hidden data-testid="repository-list-placeholder" className="space-y-1 pb-3">
            {Array.from({ length: PLACEHOLDER_ROWS }, (_, i) => (
              <div key={i} className="flex items-start gap-3 px-2 py-2">
                <div className="min-w-0 flex-1 space-y-1.5">
                  <Skeleton className="h-3.5 w-40" />
                  <Skeleton className="h-3 w-56" />
                </div>
                <Skeleton className="h-[15px] w-[26px] shrink-0 rounded-full" />
              </div>
            ))}
          </div>
        ) : repositories.length === 0 ? (
          <div className="flex h-full items-center justify-center px-6 text-center">
            <div className="flex max-w-[260px] flex-col items-center">
              <div className="mb-3 flex h-10 w-10 items-center justify-center rounded-full bg-surface-secondary text-label-tertiary">
                <FolderSearch size={20} strokeWidth={1.75} aria-hidden="true" />
              </div>
              <p className="type-callout font-medium text-label-secondary">{emptyTitle}</p>
              <p className="mt-1.5 type-footnote text-label-tertiary">{emptyDescription}</p>
              {onRefresh && (
                <PushButton className="mt-3 gap-1.5" onClick={onRefresh} disabled={loading}>
                  <Search size={12} aria-hidden="true" />
                  Scan again
                </PushButton>
              )}
            </div>
          </div>
        ) : (
          <div className="pb-3">
            {repositories.map((item) => (
              <RepositoryRow
                key={item.key}
                item={item}
                {...(onToggleRepository ? { onToggleRepository } : {})}
                {...(onLocate ? { onLocate } : {})}
                {...(onGrantAccess ? { onGrantAccess } : {})}
                {...(wslIcon ? { wslIcon } : {})}
              />
            ))}
          </div>
        )}
      </ScrollPane>
    </div>
  )
}
