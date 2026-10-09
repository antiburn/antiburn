import { noteInteraction, revealSource, type Interaction } from "../../../lib/ipc"
import {
  archiveAgentMemory,
  listAgentMemories,
  removeAgentMemoryIndexLine,
  restoreAgentMemory,
  type AgentMemoriesReport,
  type DanglingIndexEntry,
  type MemoryEditOutcome,
  type MemoryEntry,
  type MemoryProject,
} from "../../../lib/memoriesIpc"
import { SurfaceExposureTracker } from "../../../lib/surfaceExposure"
import { readCollapsedProjects, writeCollapsedProjects } from "./memoriesViewPrefs"

export interface MemoriesSnapshot {
  /** The last loaded report, or null until the first load ends. */
  report: AgentMemoriesReport | null
  error: boolean
  loading: boolean
  collapsedProjects: ReadonlySet<string>
  expandedMemories: ReadonlySet<string>
  /** Memories deleted in this visit, by path. They stay in the stale report. */
  archived: ReadonlyMap<string, ArchivedMemory>
  /** The last failed action per memory path, or per `indexLineKey`. */
  rowErrors: ReadonlyMap<string, RowError>
  /** Index lines removed in this visit, by `indexLineKey`. */
  removedIndexLines: ReadonlySet<string>
  /** Projects whose MEMORY.md has a backup written by antiburn. */
  indexBackupWritten: ReadonlySet<string>
  /** A request to scroll to and focus one memory row. The row clears it. */
  focusRequest: FocusRequest | null
}

interface FocusRequest {
  path: string
  /** Grows with each request, so a repeat request for one path still acts. */
  revision: number
}

export interface ArchivedMemory {
  slug: string
  archiveId: string
  indexLineRemoved: boolean
}

export type RowError = "changedOnDisk" | "failed"

/** The key of one index line in the removed set and in the row errors. */
export function indexLineKey(slug: string, lineNumber: number): string {
  return `${slug}:${lineNumber}`
}

export interface MemoriesAdapter {
  listMemories(): Promise<AgentMemoriesReport>
  reveal(path: string): Promise<void>
  archive(
    slug: string,
    fileName: string,
    expectedSizeBytes: number,
    expectedModifiedMs: number | null,
  ): Promise<MemoryEditOutcome>
  restore(slug: string, archiveId: string): Promise<MemoryEditOutcome>
  removeIndexLine(slug: string, lineNumber: number, target: string): Promise<MemoryEditOutcome>
  noteInteraction(interaction: Interaction): void
  now(): number
}

type MemoryActionKind = Extract<Interaction, { kind: "memoryAction" }>["action"]

const productionAdapter: MemoriesAdapter = {
  listMemories: () => listAgentMemories(),
  reveal: (path) => revealSource(path),
  archive: archiveAgentMemory,
  restore: restoreAgentMemory,
  removeIndexLine: removeAgentMemoryIndexLine,
  noteInteraction,
  now: () => Date.now(),
}

/** Compare two nullable times, oldest first, with a missing time last. */
function compareNullableAscending(left: number | null, right: number | null): number {
  if (left == null && right == null) return 0
  if (left == null) return 1
  if (right == null) return -1
  return left - right
}

/** Order memories by the least recently referenced first, then the least
 *  recently written, then by title. A missing time sorts last. */
export function sortMemories(entries: readonly MemoryEntry[]): MemoryEntry[] {
  return [...entries].sort(
    (left, right) =>
      compareNullableAscending(left.facts.lastReferencedMs, right.facts.lastReferencedMs) ||
      compareNullableAscending(left.facts.lastWrittenMs, right.facts.lastWrittenMs) ||
      left.title.localeCompare(right.title),
  )
}

/** Toggle one member of a set without changing the set it received. */
function toggled(set: ReadonlySet<string>, member: string): Set<string> {
  const next = new Set(set)
  if (!next.delete(member)) next.add(member)
  return next
}

/**
 * Own the Memories section's read: every project's memories. Loads when a
 * viewer becomes active, and again each time the view is left and entered.
 * The previous report stays on screen during a reload. This screen does not
 * live-update.
 */
export class MemoriesSession {
  private readonly adapter: MemoriesAdapter
  private snapshot: MemoriesSnapshot
  private readonly listeners = new Set<() => void>()
  private readonly activeListeners = new Set<() => void>()
  private loadVersion = 0
  private focusRevision = 0
  private active = false
  /** Counts activity changes. An edit that settles later must not land. */
  private activity = 0
  private readonly exposure = new SurfaceExposureTracker()

  constructor(adapter: MemoriesAdapter = productionAdapter) {
    this.adapter = adapter
    this.snapshot = {
      report: null,
      error: false,
      loading: false,
      collapsedProjects: new Set(readCollapsedProjects()),
      expandedMemories: new Set(),
      archived: new Map(),
      rowErrors: new Map(),
      removedIndexLines: new Set(),
      indexBackupWritten: new Set(),
      focusRequest: null,
    }
  }

  getSnapshot = (): MemoriesSnapshot => this.snapshot
  subscribe = (listener: () => void): (() => void) => this.attach(listener, true)
  subscribeInactive = (listener: () => void): (() => void) => this.attach(listener, false)

  private update(patch: Partial<MemoriesSnapshot>): void {
    this.snapshot = { ...this.snapshot, ...patch }
    this.syncExposure()
    for (const listener of this.listeners) listener()
  }

  private attach(listener: () => void, active: boolean): () => void {
    this.listeners.add(listener)
    if (active) this.activeListeners.add(listener)
    this.syncActive()
    return () => {
      this.listeners.delete(listener)
      this.activeListeners.delete(listener)
      this.syncActive()
    }
  }

  private syncActive(): void {
    const active = this.activeListeners.size > 0
    if (active === this.active) return
    this.active = active
    this.activity += 1
    // A load that started in an earlier activity period must not land.
    this.loadVersion += 1
    if (!active) {
      this.exposure.conceal("memories")
      // The next load reflects the disk, so this visit's edits are dropped.
      this.snapshot = {
        ...this.snapshot,
        loading: false,
        archived: new Map(),
        rowErrors: new Map(),
        removedIndexLines: new Set(),
        focusRequest: null,
      }
      return
    }
    this.syncExposure()
    void this.load()
  }

  /** Read the report again. Exposed for a Retry button. */
  refresh = (): void => {
    if (this.active) void this.load()
  }

  private async load(): Promise<void> {
    const version = ++this.loadVersion
    this.update({ loading: true, error: false })
    try {
      const report = await this.adapter.listMemories()
      if (version !== this.loadVersion) return
      // Line numbers and paths in the new report are fresh. The old edits
      // and errors no longer match them.
      this.update({
        report,
        loading: false,
        rowErrors: new Map(),
        removedIndexLines: new Set(),
      })
    } catch {
      if (version !== this.loadVersion) return
      this.update({ error: true, loading: false })
    }
  }

  toggleProject = (slug: string): void => {
    const collapsedProjects = toggled(this.snapshot.collapsedProjects, slug)
    writeCollapsedProjects(collapsedProjects)
    this.update({ collapsedProjects })
  }

  toggleMemory = (path: string): void => {
    this.update({ expandedMemories: toggled(this.snapshot.expandedMemories, path) })
  }

  /** Open one memory: show its project, expand the row, and ask the row to take focus. */
  focus = (slug: string, path: string): void => {
    let collapsedProjects = this.snapshot.collapsedProjects
    if (collapsedProjects.has(slug)) {
      collapsedProjects = toggled(collapsedProjects, slug)
      writeCollapsedProjects(collapsedProjects)
    }
    this.update({
      collapsedProjects,
      expandedMemories: new Set(this.snapshot.expandedMemories).add(path),
      focusRequest: { path, revision: ++this.focusRevision },
    })
  }

  /** The row took focus. Clear the request if no newer one replaced it. */
  focusHandled = (revision: number): void => {
    if (this.snapshot.focusRequest?.revision !== revision) return
    this.update({ focusRequest: null })
  }

  now = (): number => this.adapter.now()

  reveal = (path: string): Promise<void> =>
    this.adapter.reveal(path).then(
      () => this.noteAction("reveal", "succeeded"),
      () => this.noteAction("reveal", "failed"),
    )

  private noteAction(
    action: MemoryActionKind,
    outcome: Extract<Interaction, { kind: "memoryAction" }>["outcome"],
  ): void {
    this.adapter.noteInteraction({ kind: "memoryAction", action, outcome })
  }

  /** The analytics outcome for an edit that did not succeed. */
  private failureOutcome(
    outcome: MemoryEditOutcome | null,
  ): "failed" | "changed_on_disk" | "unsupported" {
    if (outcome?.outcome === "changedOnDisk" || outcome?.outcome === "missing") {
      return "changed_on_disk"
    }
    if (outcome?.outcome === "unavailable" && outcome.reason === "automaticapplyunsupported") {
      return "unsupported"
    }
    return "failed"
  }

  private setRowError(key: string, error: RowError): void {
    this.update({ rowErrors: new Map(this.snapshot.rowErrors).set(key, error) })
  }

  private clearRowError(key: string): void {
    if (!this.snapshot.rowErrors.has(key)) return
    const rowErrors = new Map(this.snapshot.rowErrors)
    rowErrors.delete(key)
    this.update({ rowErrors })
  }

  /**
   * Run one edit. The edit's interaction goes out exactly once, after the
   * edit settles. The state only changes when the visit is still the same.
   */
  private async runEdit(
    action: MemoryActionKind,
    key: string,
    edit: () => Promise<MemoryEditOutcome>,
    succeeded: (outcome: MemoryEditOutcome) => boolean,
    apply: (outcome: MemoryEditOutcome) => void,
  ): Promise<void> {
    const activity = this.activity
    this.clearRowError(key)
    const outcome = await edit().catch(() => null)
    const sameVisit = activity === this.activity
    if (outcome && succeeded(outcome)) {
      if (sameVisit) apply(outcome)
      this.noteAction(action, "succeeded")
      return
    }
    const kind = this.failureOutcome(outcome)
    this.noteAction(action, kind)
    if (sameVisit)
      this.setRowError(key, kind === "changed_on_disk" ? "changedOnDisk" : "failed")
  }

  /** Delete a memory by moving it into antiburn's archive. */
  archive = (project: MemoryProject, memory: MemoryEntry): Promise<void> =>
    this.runEdit(
      "archive",
      memory.path,
      () =>
        this.adapter.archive(
          project.slug,
          memory.fileName,
          memory.sizeBytes,
          memory.modifiedMs,
        ),
      (outcome) => outcome.outcome === "archived",
      (outcome) => {
        if (outcome.outcome !== "archived") return
        const archived = new Map(this.snapshot.archived)
        archived.set(memory.path, {
          slug: project.slug,
          archiveId: outcome.archiveId,
          indexLineRemoved: outcome.indexLineRemoved,
        })
        const indexBackupWritten = outcome.indexLineRemoved
          ? new Set(this.snapshot.indexBackupWritten).add(project.slug)
          : this.snapshot.indexBackupWritten
        this.update({ archived, indexBackupWritten })
      },
    )

  /** Put a deleted memory back from the archive. */
  undo = (memory: MemoryEntry): Promise<void> => {
    const archived = this.snapshot.archived.get(memory.path)
    if (!archived) return Promise.resolve()
    return this.runEdit(
      "restore",
      memory.path,
      () => this.adapter.restore(archived.slug, archived.archiveId),
      // The file is already back when the restore target exists.
      (outcome) => outcome.outcome === "restored" || outcome.outcome === "alreadyExists",
      () => {
        // The report still holds the row and its facts, so no reload is needed.
        const next = new Map(this.snapshot.archived)
        next.delete(memory.path)
        this.update({ archived: next })
      },
    )
  }

  /** Remove a dangling line from a project's MEMORY.md. */
  removeIndexLine = (project: MemoryProject, entry: DanglingIndexEntry): Promise<void> => {
    const key = indexLineKey(project.slug, entry.lineNumber)
    // The report lists the line numbers of its own read. Lines removed since
    // then moved the later lines up.
    let lineNumber = entry.lineNumber
    for (const removed of this.snapshot.removedIndexLines) {
      const split = removed.lastIndexOf(":")
      if (
        removed.slice(0, split) === project.slug &&
        Number(removed.slice(split + 1)) < entry.lineNumber
      ) {
        lineNumber -= 1
      }
    }
    return this.runEdit(
      "remove_index_line",
      key,
      () => this.adapter.removeIndexLine(project.slug, lineNumber, entry.target),
      (outcome) => outcome.outcome === "indexLineRemoved",
      () => {
        this.update({
          removedIndexLines: new Set(this.snapshot.removedIndexLines).add(key),
          indexBackupWritten: new Set(this.snapshot.indexBackupWritten).add(project.slug),
        })
      },
    )
  }

  private syncExposure(): void {
    if (!this.active) return
    const generation = this.exposure.expose({ surface: "memories", origin: "user" })
    const state = this.memoriesState()
    if (state) this.exposure.observe(state, generation)
  }

  private memoriesState(): "ready" | "empty" | "error" | null {
    const { error, report } = this.snapshot
    if (error) return "error"
    if (!report) return null
    return report.projects.length === 0 ? "empty" : "ready"
  }

  dispose = (): void => {
    this.loadVersion += 1
    this.active = false
    this.activeListeners.clear()
    this.listeners.clear()
    this.exposure.conceal("memories")
  }
}
