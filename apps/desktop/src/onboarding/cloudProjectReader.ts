import type { CloudPort, CloudWorkspace } from "../lib/cloud"
import { mergeProjectPage, projectPagesByPort, type ProjectPages } from "./cloudProjectPages"
import {
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
  isActive: () => boolean
  isVisible: () => boolean
  getPages: () => ProjectPages | undefined
  needsChoiceReview: () => boolean
  publishProjects: (projects: NonNullable<CloudViewSnapshot["projects"]>) => void
  publishGroups: (
    pages: ProjectPages,
    workspaces: readonly CloudWorkspace[] | undefined,
    state?: "provisional" | "refreshing",
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
  private scopeRead: Promise<void> | undefined
  private pendingPageRelease: Promise<void> | undefined
  private readonly accountLane = new CloudReadLane()
  private readonly folderLane = new CloudReadLane()
  private pageScope: string | undefined
  private readonly port: CloudPort
  private readonly observer: ProjectReaderObserver

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

  /** Cancel retries without publishing state during account or source changes. */
  abort() {
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
    if (projects?.paging || projects?.loading || projects?.retrying)
      this.observer.publishProjects({
        ...projects,
        loaded: projects.loaded && !projects.loading,
        loading: false,
        retrying: false,
        retryStage: undefined,
        paging: this.pendingPaging(projects.hasMore),
      })
  }

  /** Stop discovery before a write. Desktop still controls admission if a read does not finish. */
  finishRead() {
    this.suspend()
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

  /** Account and scope reads use the same native worker. */
  request<T>(read: () => Promise<T>, signal: AbortSignal): Promise<T> {
    return this.accountLane.request(read, signal)
  }

  /** Retry the folder pane without a global account reload or duplicate scope request. */
  retryScope() {
    if (this.hasRead && this.scopeRead) return this.scopeRead
    if (this.hasRead) this.suspend()
    return this.load(false, true)
  }

  /** Read one bounded page. A failed page never becomes an empty success. */
  load(more: boolean, retryInBackground: boolean) {
    if (this.hasRead) return Promise.resolve()
    const read = this.readProjects(more, retryInBackground)
    if (!more) {
      this.scopeRead = read
      const clear = () => {
        if (this.scopeRead === read) this.scopeRead = undefined
      }
      void read.then(clear, clear)
    }
    return read
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
        if (this.observer.isActive() && !this.hasRead && projects?.paging === "pending")
          this.observer.publishProjects({ ...projects, paging: undefined })
      })
    }
    return "pending" as const
  }

  private async readProjects(more: boolean, retryInBackground: boolean) {
    const { observer } = this
    const port = this.port.ongoing
    const binding = projectBinding(observer.getSnapshot().cloud, !more)
    const existing = observer.getSnapshot().projects
    if (
      !observer.isActive() ||
      !observer.isVisible() ||
      (more && (!observer.getPages()?.next || this.folderLane.pending || !this.canUseChoices))
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
    const cursor = more ? (previous?.next ?? null) : null
    let latest: ProjectPages | undefined
    let remainingEmptyRechecks = 1
    const foreground = setTimeout(() => {
      retryDeadline.abort(timeoutError)
      if (!current()) return
      const projects = observer.getSnapshot().projects!
      const keepChoices = this.canUseChoices
      const message = scopeVerified ? folderReadError : scopeReadError
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
        observer.publishGroups(
          pages,
          workspaces,
          settled || foregroundExpired ? undefined : "refreshing",
        )
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
      const fallback = latest ?? (!existing?.scopeInvalid ? previous : undefined)
      latest =
        !more &&
        page.partial &&
        !page.groups.length &&
        !page.nextCursor &&
        !fallback?.next &&
        fallback?.groups.length
          ? { ...fallback, partial: true }
          : mergeProjectPage(previous, cursor, page)
      if (!more && current() && latest.groups.length) publish(latest)
      return page
    }
    const readPage = () =>
      (retrySignal.aborted
        ? requestPage(signal)
        : retryCloudRead(
            () => requestPage(retrySignal),
            retrySignal,
            (page) =>
              !retrySignal.aborted &&
              page.partial &&
              !page.nextCursor &&
              !latest?.groups.length &&
              remainingEmptyRechecks-- > 0,
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
        if (verified === null) {
          const projects = observer.getSnapshot().projects!
          observer.publishProjects({
            ...projects,
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
      if (current() && latest) publish(latest, true)
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
        const message = scopeVerified ? folderReadError : scopeReadError
        observer.publishProjects({
          ...projects,
          loading: false,
          retrying: false,
          retryStage: undefined,
          paging: this.pendingPaging(projects.hasMore),
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
