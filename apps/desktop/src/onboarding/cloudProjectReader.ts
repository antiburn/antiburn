import type { CloudPort, CloudProjectCursor, CloudWorkspace } from "../lib/cloud"
import { traceEvent } from "../lib/perfTrace"
import {
  mergeProjectPage,
  nextProjectCursor,
  projectPagesByPort,
  type ProjectPages,
} from "./cloudProjectPages"
import {
  cloudCaptureUnsupported,
  cloudFoldersLoading,
  readCloudProjectScope,
  type ProjectScopeObserver,
} from "./cloudProjectScope"
import {
  cloudReadNeedsVerifiedScope,
  cloudReadScopeMessage,
  retryCloudRead,
} from "./cloudReadRetry"
import {
  cloudProjectSourceKey,
  projectBinding,
  projectViewGroups,
  type CloudViewSnapshot,
} from "./cloudSessionState"

const folderForegroundTimeout = 7_000
const folderReadTimeout = 30_000
const writeWaitTimeout = 1_000
const folderReadError = "Could not check session folders."
const scopeReadError = "Could not check Cloud setup or workspaces."

function waitForRequest<T>(request: Promise<T>, signal: AbortSignal): Promise<T> {
  return new Promise((resolve, reject) => {
    const abort = () => reject(signal.reason)
    signal.addEventListener("abort", abort, { once: true })
    request.then(resolve, reject).then(() => signal.removeEventListener("abort", abort))
    if (signal.aborted) abort()
  })
}

interface ProjectReaderObserver extends ProjectScopeObserver {
  readonly automaticPaging: boolean
  isActive: () => boolean
  isVisible: () => boolean
  getPages: () => ProjectPages | undefined
  needsChoiceReview: () => boolean
  publishProjects: (projects: NonNullable<CloudViewSnapshot["projects"]>) => void
  publishGroups: (
    pages: ProjectPages,
    workspaces: readonly CloudWorkspace[] | undefined,
    state?: "provisional" | "refreshing" | "paging",
  ) => void
}

/** Keep native requests separate across workers. An abort stops waiting, not IPC work. */
class CloudReadLane {
  pending: Promise<void> | undefined

  async request<T>(
    read: () => Promise<T>,
    signal: AbortSignal,
    admissionSignal = signal,
  ): Promise<T> {
    while (this.pending) await waitForRequest(this.pending, admissionSignal)
    admissionSignal.throwIfAborted()
    const result = read()
    const release = () => {
      if (this.pending === pending) this.pending = undefined
    }
    const pending = result.then(release, release)
    this.pending = pending
    return waitForRequest(result, signal)
  }
}

/** Own one session's folder reads. The session still owns choices and consent. */
export class CloudProjectReader {
  private read: AbortController | undefined
  private generation = 0
  private pagesRead: Promise<void> | undefined
  private scopeRead: Promise<void> | undefined
  private failedPendingPage: { request: Promise<void>; recheckQueued: boolean } | undefined
  private pendingPageRelease: Promise<void> | undefined
  private readonly accountLane = new CloudReadLane()
  private readonly folderLane = new CloudReadLane()
  private pageScope: string | undefined
  private readonly port: CloudPort
  private readonly observer: ProjectReaderObserver
  private automaticCursor: CloudProjectCursor | null | undefined
  private interruptedWriteScope: string | undefined

  constructor(port: CloudPort, observer: ProjectReaderObserver) {
    this.port = port
    this.observer = observer
  }

  /** Report reads that can still publish results. */
  get hasRead() {
    return !!this.read && !this.read.signal.aborted
  }

  /** Only rows loaded for the current account, workspace, and sources can be approved. */
  get canUseChoices() {
    const { cloud, projects } = this.observer.getSnapshot()
    const binding = projectBinding(cloud, false)
    return (
      !!binding &&
      !!projects?.loaded &&
      !projects.loading &&
      !projects.scopeInvalid &&
      !projects.error &&
      this.pageScope === JSON.stringify([binding, cloudProjectSourceKey(cloud)])
    )
  }

  /** Follow fresh replies even when cached pages already complete their cursor chain. */
  private get nextCursor() {
    return this.observer.automaticPaging && this.automaticCursor !== undefined
      ? this.automaticCursor
      : nextProjectCursor(this.observer.getPages())
  }

  /** Keep counts incomplete until Settings has checked every page in this refresh. */
  private get hasUnreadAutomaticPages() {
    return this.observer.automaticPaging && this.nextCursor !== undefined
  }

  /** Resume unread Settings pages without replacing the first page on return. */
  get hasPendingAutomaticPages() {
    return (
      this.hasUnreadAutomaticPages &&
      this.canUseChoices &&
      !this.observer.getSnapshot().projects?.refreshError
    )
  }

  /** Keep indexed counts stable until the fresh page chain finishes. Provisional rows grant no consent. */
  viewProjects(pages: ProjectPages, provisional: boolean) {
    let groups = projectViewGroups(pages.groups, pages.sources)
    if (this.hasUnreadAutomaticPages) {
      const counts = new Map(
        this.observer
          .getSnapshot()
          .projects?.groups.map((group) => [group.id, group.sessionCount]),
      )
      groups = groups.map((group) =>
        group.discoveryStale ? group : { ...group, sessionCount: counts.get(group.id) ?? null },
      )
    }
    const hasMore =
      !provisional && (this.hasUnreadAutomaticPages || nextProjectCursor(pages) !== undefined)
    return { groups, hasMore }
  }

  /** Cancel retries without publishing state during account or source changes. */
  abort() {
    this.generation++
    this.failedPendingPage = undefined
    this.read?.abort()
  }

  /** Drop the former account's cached pages, including for other surfaces. */
  forgetCachedPages() {
    projectPagesByPort.delete(this.port)
  }

  /** Keep displayed pages when a visibility change or sign-out interrupts a read. */
  suspend() {
    this.abort()
    const projects = this.observer.getSnapshot().projects
    if (projects?.paging || projects?.loading || projects?.retrying) {
      const hasMore = projects.paging ? this.nextCursor !== undefined : projects.hasMore
      this.observer.publishProjects({
        ...projects,
        hasMore,
        loaded: projects.loaded && !projects.loading,
        loading: false,
        retrying: false,
        retryStage: undefined,
        paging: this.pendingPaging(hasMore),
      })
    }
  }

  /** Stop discovery before a write. Desktop still controls admission if a read does not finish. */
  finishRead() {
    const scope = this.hasPendingAutomaticPages ? this.pageScope : undefined
    this.suspend()
    this.interruptedWriteScope = scope
    const pending = this.accountLane.pending
    if (!pending) return
    const controller = new AbortController()
    const timeout = setTimeout(() => controller.abort(), writeWaitTimeout)
    return waitForRequest(pending, controller.signal)
      .catch(() => {
        // The explicit write can report desktop admission without waiting on this read.
      })
      .finally(() => clearTimeout(timeout))
  }

  /** Resume paging that a write paused in the same verified scope. Call after busy clears. */
  resumeAfterWrite() {
    const scope = this.interruptedWriteScope
    this.interruptedWriteScope = undefined
    if (
      scope === undefined ||
      scope !== this.pageScope ||
      !this.observer.isActive() ||
      !this.observer.isVisible() ||
      this.observer.getSnapshot().busy ||
      this.hasRead ||
      !this.hasPendingAutomaticPages
    )
      return
    return this.load(true, true)
  }

  /** Account and scope reads use the same native worker. */
  request<T>(read: () => Promise<T>, signal: AbortSignal): Promise<T> {
    return this.accountLane.request(read, signal)
  }

  /** Retry the folder pane, or share the current scope read. */
  retryScope() {
    if (this.hasRead && this.scopeRead) return this.scopeRead
    if (this.hasRead) this.suspend()
    return this.load(false, true)
  }

  /** Keep one focus-triggered recheck until the failed page releases the worker. */
  queueFailedPageRecheck() {
    const failed = this.failedPendingPage
    if (!failed || failed.request !== this.folderLane.pending || !this.canUseChoices)
      return false
    if (!failed.recheckQueued) {
      failed.recheckQueued = true
      void Promise.all([failed.request, this.pagesRead]).then(() => {
        if (this.failedPendingPage !== failed) return
        this.failedPendingPage = undefined
        const { busy, projects } = this.observer.getSnapshot()
        if (
          this.observer.isActive() &&
          this.observer.isVisible() &&
          !busy &&
          !this.hasRead &&
          this.canUseChoices &&
          projects?.hasMore &&
          projects.refreshError
        )
          return this.load(false, true)
      })
    }
    return true
  }

  /** Read bounded pages. Settings follows the remaining cursor chain. */
  load(more: boolean, retryInBackground: boolean) {
    if (this.hasRead) return Promise.resolve()
    this.failedPendingPage = undefined
    if (!more) this.automaticCursor = undefined
    const read = this.readProjectPages(more, retryInBackground, ++this.generation)
    this.pagesRead = read
    if (!more) this.scopeRead = read
    const clear = () => {
      if (this.pagesRead === read) this.pagesRead = undefined
      if (this.scopeRead === read) this.scopeRead = undefined
    }
    void read.then(clear, clear)
    return read
  }

  /** Follow Settings cursors in order. Each page keeps its own timeout and retry limit. */
  private async readProjectPages(
    more: boolean,
    retryInBackground: boolean,
    generation: number,
  ) {
    let loaded = await this.readProjects(more, retryInBackground)
    while (
      loaded &&
      generation === this.generation &&
      this.hasPendingAutomaticPages &&
      this.observer.isActive() &&
      this.observer.isVisible()
    ) {
      if (loaded.retrySamePage) {
        traceEvent("cloud.folder_page_deferred")
        const projects = this.observer.getSnapshot().projects!
        this.observer.publishProjects({ ...projects, refreshError: folderReadError })
        return
      }
      if (this.observer.getSnapshot().busy) {
        this.interruptedWriteScope = this.pageScope
        return
      }
      loaded = await this.readProjects(true, retryInBackground)
    }
  }

  /** Keep pagination disabled until a dispatched folder request releases the worker. */
  private pendingPaging(hasMore: boolean) {
    const pending = hasMore ? this.folderLane.pending : undefined
    if (!pending) return undefined
    if (this.pendingPageRelease !== pending) {
      this.pendingPageRelease = pending
      void pending.then(() => {
        if (this.pendingPageRelease !== pending) return
        this.pendingPageRelease = undefined
        const projects = this.observer.getSnapshot().projects
        if (this.observer.isActive() && !this.hasRead && projects?.paging === "pending") {
          this.observer.publishProjects({ ...projects, paging: undefined })
          if (
            this.observer.isVisible() &&
            !this.observer.getSnapshot().busy &&
            this.hasPendingAutomaticPages
          )
            void this.load(true, true)
        }
      })
    }
    return "pending" as const
  }

  /** Verify the first page's scope. Later pages keep that scope until an event cancels it. */
  private async readProjects(more: boolean, retryInBackground: boolean) {
    const { observer } = this
    const port = this.port.ongoing
    const binding = projectBinding(observer.getSnapshot().cloud, !more)
    const existing = observer.getSnapshot().projects
    const next = this.nextCursor
    if (
      !observer.isActive() ||
      !observer.isVisible() ||
      (more && (next === undefined || this.folderLane.pending || !this.canUseChoices))
    )
      return
    if (!port || !binding) return
    const usableChoices = this.canUseChoices
    const read = (this.read = new AbortController())
    const { signal } = read
    const timeoutError = new Error(folderReadError)
    const timeout = setTimeout(() => read.abort(timeoutError), folderReadTimeout)
    const retryDeadline = new AbortController()
    signal.addEventListener("abort", () => retryDeadline.abort(signal.reason), { once: true })
    const retrySignal = retryDeadline.signal
    const scopeKey = JSON.stringify([
      binding,
      cloudProjectSourceKey(observer.getSnapshot().cloud),
    ])
    const current = () => observer.isActive() && this.read === read && !signal.aborted
    observer.publishProjects(
      more && existing
        ? { ...existing, paging: "loading", refreshError: undefined }
        : usableChoices && existing
          ? {
              ...existing,
              retrying: true,
              retryStage: undefined,
              paging: existing.hasMore ? "pending" : undefined,
            }
          : {
              ...cloudFoldersLoading(existing, observer.needsChoiceReview()),
              scopeInvalid: !!existing?.scopeInvalid,
              error: existing?.scopeInvalid ? existing.error : null,
              refreshError: undefined,
            },
    )
    const onExtendedRetry = retryInBackground
      ? (stage: "setup" | "workspaces" | "folders") => {
          const projects = observer.getSnapshot().projects
          if (current() && !retryDeadline.signal.aborted && projects)
            observer.publishProjects(
              more
                ? { ...projects, paging: "retrying" }
                : { ...projects, retrying: true, retryStage: stage },
            )
        }
      : undefined
    let scopeVerified = more
    let workspaces = existing?.workspaces
    const cached = projectPagesByPort.get(this.port)
    const previous =
      this.pageScope === scopeKey
        ? observer.getPages()
        : cached?.binding === scopeKey
          ? cached.pages
          : undefined
    const cursor = more ? (next ?? null) : null
    let latest: ProjectPages | undefined
    let retrySamePage = false
    let remainingEmptyRechecks = 1
    let remainingPageContinuations = 1
    const foreground = setTimeout(() => {
      retryDeadline.abort(timeoutError)
      if (!current()) return
      const projects = observer.getSnapshot().projects!
      const keepChoices = this.canUseChoices
      const message = scopeVerified ? folderReadError : scopeReadError
      const pendingPage = this.folderLane.pending
      if (more && observer.automaticPaging && keepChoices && pendingPage)
        this.failedPendingPage = { request: pendingPage, recheckQueued: false }
      observer.publishProjects({
        ...projects,
        loading: false,
        retrying: false,
        retryStage: undefined,
        paging: projects.hasMore ? "pending" : undefined,
        error: keepChoices ? null : (projects.error ?? message),
        refreshError: keepChoices ? message : undefined,
      })
    }, folderForegroundTimeout)
    const publish = (pages: ProjectPages, settled = false) => {
      const foregroundExpired = retryDeadline.signal.aborted
      if (scopeVerified) {
        projectPagesByPort.set(this.port, { binding: scopeKey, pages })
        this.pageScope = scopeKey
        const state = settled || foregroundExpired ? undefined : more ? "paging" : "refreshing"
        observer.publishGroups(pages, workspaces, state)
      } else if (!usableChoices && !foregroundExpired) {
        this.pageScope = undefined
        observer.publishGroups(pages, undefined, "provisional")
      }
    }
    const requestPage = async (admissionSignal: AbortSignal) => {
      const page = await this.folderLane.request(
        () => port.groups(binding, cursor),
        signal,
        admissionSignal,
      )
      signal.throwIfAborted()
      retrySamePage = !!page.retrySamePage
      const fallback = latest ?? (!existing?.scopeInvalid ? previous : undefined)
      latest =
        !more &&
        page.partial &&
        !page.groups.length &&
        !page.nextCursor &&
        !fallback?.next &&
        fallback?.groups.length
          ? { ...fallback, partial: true, deferred: page.retrySamePage ? null : undefined }
          : mergeProjectPage(previous, cursor, page)
      const continuing =
        page.retrySamePage && remainingPageContinuations > 0 && !retrySignal.aborted
      if (this.observer.automaticPaging && current())
        this.automaticCursor = page.retrySamePage ? cursor : (page.nextCursor ?? undefined)
      if (current() && (!more ? latest.groups.length : continuing)) publish(latest)
      return page
    }
    const readPage = () =>
      (retrySignal.aborted
        ? requestPage(signal)
        : retryCloudRead(
            () => requestPage(retrySignal),
            retrySignal,
            (page) => {
              if (retrySignal.aborted) return false
              if (page.retrySamePage) return remainingPageContinuations-- > 0
              return (
                page.partial &&
                !page.nextCursor &&
                !latest?.groups.length &&
                remainingEmptyRechecks-- > 0
              )
            },
            onExtendedRetry && (() => onExtendedRetry("folders")),
          )
      ).then(
        () => ({ ok: true as const }),
        (error: unknown) => ({ ok: false as const, error }),
      )
    try {
      // Folder discovery uses a separate local worker. It does not need the workspace list.
      const speculative = !more && !observer.getSnapshot().cloud.setupError
      let page = more || speculative ? readPage() : undefined
      if (!more) {
        const verified = await readCloudProjectScope(
          this.port,
          port,
          binding,
          signal,
          observer,
          (request) => this.request(request, signal),
          onExtendedRetry,
        )
        if (!current()) return
        const setupError = observer.getSnapshot().cloud.setupError
        if (verified === null || setupError) {
          const projects = observer.getSnapshot().projects!
          observer.publishProjects({
            ...projects,
            ...(setupError === cloudCaptureUnsupported && verified
              ? { workspaces: verified, scopeInvalid: false, error: null }
              : {}),
            loading: false,
            retrying: false,
            retryStage: undefined,
            paging: this.pendingPaging(projects.hasMore),
          })
          return
        }
        scopeVerified = true
        workspaces = verified
        const display = latest ?? (!existing?.scopeInvalid ? previous : undefined)
        if (display?.groups.length) publish(display)
      }
      page ??= readPage()
      const result = await page
      if (!result.ok) {
        if (!speculative || !cloudReadNeedsVerifiedScope(result.error) || !current())
          throw result.error
        await requestPage(signal)
      }
      if (current() && latest) {
        publish(latest, true)
        return { retrySamePage }
      }
    } catch (error) {
      const scopeMessage = cloudReadScopeMessage(error)
      if (
        observer.isActive() &&
        this.read === read &&
        (!signal.aborted || signal.reason === timeoutError)
      ) {
        const projects = observer.getSnapshot().projects!
        const scopeInvalid = projects.scopeInvalid || !!scopeMessage
        const keepChoices = this.canUseChoices && !scopeInvalid
        if (more && observer.automaticPaging && keepChoices)
          traceEvent("cloud.folder_page_failed", { timedOut: signal.reason === timeoutError })
        const message = scopeVerified ? folderReadError : scopeReadError
        const hasMore = more ? this.nextCursor !== undefined : projects.hasMore
        observer.publishProjects({
          ...projects,
          hasMore,
          loading: false,
          retrying: false,
          retryStage: undefined,
          paging: this.pendingPaging(hasMore),
          scopeInvalid,
          error: scopeMessage ?? (keepChoices ? null : message),
          refreshError: keepChoices ? message : undefined,
        })
      }
    } finally {
      clearTimeout(foreground)
      clearTimeout(timeout)
      read.abort()
      if (this.read === read) this.read = undefined
    }
  }
}
