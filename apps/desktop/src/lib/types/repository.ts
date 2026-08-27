/**
 * Public presentation types for local repositories.
 *
 * Mirrors the JSON shape of `antiburn_local::repositories::{
 * RepositoryDescriptor, RepoAccessStatus, LocalRepoStatus}` as it crosses the
 * IPC boundary. The engine serializes with `#[serde(rename_all = "camelCase")]`
 * for records and `snake_case` for the status enum, and the shapes below keep
 * that spelling.
 *
 * Nothing here carries a hosting-provider, account, or eligibility field: the
 * engine's descriptor is a plain name/URL record the embedding application
 * fills in from whatever source it has.
 */

/**
 * A repository the caller already knows about, matched against what discovery
 * finds on disk. Mirrors engine `RepositoryDescriptor`.
 */
export interface LocalRepositoryDescriptor {
  /** Repository name without its owner (for example `widgets`). */
  name: string
  /** Owner-qualified repository name (for example `avery/widgets`). */
  fullName: string
  /** HTTPS clone URL, used to match a local clone by its git remote. */
  cloneUrl: string
  /** SSH clone URL, used to match a local clone by its git remote. */
  sshUrl: string
  /**
   * Whether the caller has selected this repository, as a tri-state: `true`
   * selected, `false` explicitly excluded, and `null`/absent "no opinion".
   * Absent is treated as selected — see {@link isRepositorySelected}.
   */
  enabled?: boolean | null
}

/**
 * Collapse a descriptor's tri-state selection with the opt-out default, so a
 * source that predates the notion of selection does not disable everything.
 * Mirrors engine `RepositoryDescriptor::is_enabled`.
 */
export function isRepositorySelected(descriptor: LocalRepositoryDescriptor): boolean {
  return descriptor.enabled ?? true
}

/** Access status for a single repository. Mirrors engine `RepoAccessStatus`. */
export type LocalRepositoryStatus =
  /** Found on disk and readable. */
  | "accessible"
  /** Found on disk, but the operating system blocked reading it. */
  | "permission_denied"
  /** Known to the caller, but not present on this machine. */
  | "not_cloned"
  /** Present, but excluded by the user. */
  | "disabled"

/**
 * Status of a single repository on the local machine. Mirrors engine
 * `LocalRepoStatus`, with the row identity the list needs to key on added.
 */
export interface LocalRepositoryItem {
  /**
   * Stable list identity. Build it with `localRepositoryKey` so two clones of
   * the same name in different environments cannot collide.
   */
  key: string
  /** Repository name without its owner. */
  repoName: string
  /** Owner-qualified repository name. */
  fullName: string
  status: LocalRepositoryStatus
  /** Canonical main repository root, when it is readable. */
  repoRoot?: string | null
  /** Where the repository appears to live, when present but blocked. */
  suspectedPath?: string | null
  /** Linked worktrees attached to the main root. */
  worktreeCount?: number
  /** Recent AI coding sessions whose working directory resolved here. */
  sessionCount?: number
  /** WSL distribution the repository lives in; absent for native clones. */
  wslDistro?: string | null
  /** Whether the user has this repository included. Selection is opt-out. */
  enabled: boolean
}

/**
 * A protected folder the last pass declined to read, and how many working
 * directories wait behind it.
 *
 * One entry per folder rather than per path: the operating system grants access
 * at that granularity, so it is the only granularity worth asking about.
 */
export type DeferredPermissionDir = {
  /** The folder's name, for example `Documents`. */
  dir: string
  /** How many known working directories sit inside it. */
  pathCount: number
}

/** What the last pass could and could not read. */
export type FolderPermissions = {
  /** Folders the last pass declined to read, in the order it met them. */
  deferred: DeferredPermissionDir[]
  /** Folder names the reader has already granted. */
  granted: string[]
  /**
   * Whether this platform guards folders behind consent at all. False
   * everywhere but macOS, where the whole surface is hidden.
   */
  supported: boolean
}

/**
 * How the operating system answered a request for a folder.
 *
 * `recordedDenial` is the case worth handling separately: the system refused
 * from a decision it already held, without showing a dialog, so asking again
 * does nothing and the reader has to change it in system settings.
 */
export type FolderAccessOutcome = {
  outcome: "granted" | "denied" | "recorded-denial"
  /** How long the system took to answer, in milliseconds. */
  elapsedMs: number
}

/** One recorded attempt to read a protected folder. */
export type ProbeRecord = {
  target: string
  outcome: string
  elapsedMs: number
}
