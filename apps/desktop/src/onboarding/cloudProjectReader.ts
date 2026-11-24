import type { CloudPort, CloudWorkspace } from "../lib/cloud"
import { mergeProjectPage, projectPagesByPort, type ProjectPages } from "./cloudProjectPages"
import {
  cloudFoldersLoading,
  readCloudProjectScope,
  type ProjectScopeObserver,
} from "./cloudProjectScope"
import { cloudReadScopeMessage, retryCloudRead } from "./cloudReadRetry"
import {
  cloudProjectSourceKey,
  projectBinding,
  type CloudViewSnapshot,
} from "./cloudSessionState"

interface ProjectReaderObserver extends ProjectScopeObserver {
  isActive: () => boolean
  isVisible: () => boolean
  getPages: () => ProjectPages | undefined
  needsChoiceReview: () => boolean
  publishProjects: (projects: NonNullable<CloudViewSnapshot["projects"]>) => void
  publishGroups: (
    pages: ProjectPages,
    workspaces: readonly CloudWorkspace[] | undefined,
    loading: boolean,
  ) => void
}

/** Own one session's folder reads. The session still owns choices and consent. */
export class CloudProjectReader {
  private read: AbortController | undefined
  private pendingPage: Promise<unknown> | undefined
  private pageScope: string | undefined
  private readonly port: CloudPort
  private readonly observer: ProjectReaderObserver

  constructor(port: CloudPort, observer: ProjectReaderObserver) {
    this.port = port
    this.observer = observer
  }

  /** Include aborted reads until their cleanup finishes. */
  get hasRead() {
    return this.read !== undefined
  }

  /** A dispatched page can still hold desktop admission after a read aborts. */
  get hasPendingPage() {
    return this.pendingPage !== undefined
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
    if (projects?.paging) this.observer.publishProjects({ ...projects, paging: undefined })
    else if (projects?.loading)
      this.observer.publishProjects({ ...projects, loading: false, retrying: true })
  }

  /** Stop page retries and let any dispatched read release desktop admission. */
  async finishPaging() {
    if (this.observer.getSnapshot().projects?.paging) this.suspend()
    await this.pendingPage?.catch(() => undefined)
  }

  /** Read one bounded page. A failed page never becomes an empty success. */
  async load(more: boolean, retryInBackground: boolean) {
    const { observer } = this
    const port = this.port.ongoing
    const binding = projectBinding(observer.getSnapshot().cloud, !more)
    if (
      !observer.isVisible() ||
      (more &&
        (!observer.getPages()?.next ||
          this.pendingPage ||
          observer.getSnapshot().projects?.scopeInvalid))
    )
      return
    if (!port || !binding || (this.read && !this.read.signal.aborted)) return
    const read = (this.read = new AbortController())
    const { signal } = read
    const scopeKey = JSON.stringify([
      binding,
      cloudProjectSourceKey(observer.getSnapshot().cloud),
    ])
    const current = () => observer.isActive() && !signal.aborted
    const existing = observer.getSnapshot().projects
    observer.publishProjects(
      more && existing
        ? { ...existing, paging: "loading" }
        : cloudFoldersLoading(existing, observer.needsChoiceReview()),
    )
    const onExtendedRetry = retryInBackground
      ? (stage: "setup" | "workspaces" | "folders") => {
          const projects = observer.getSnapshot().projects
          if (current() && projects)
            observer.publishProjects(
              more
                ? { ...projects, paging: "retrying" }
                : { ...projects, retrying: true, retryStage: stage },
            )
        }
      : undefined
    try {
      let workspaces = existing?.workspaces
      const cached = projectPagesByPort.get(this.port)
      const previous =
        this.pageScope === scopeKey
          ? observer.getPages()
          : cached?.binding === scopeKey
            ? cached.pages
            : undefined
      if (!more) {
        const verified = await readCloudProjectScope(
          this.port,
          port,
          binding,
          signal,
          observer,
          onExtendedRetry,
        )
        if (!current()) return
        if (verified === null) {
          observer.publishProjects({ ...observer.getSnapshot().projects!, loading: false })
          return
        }
        workspaces = verified
        if (!existing?.loaded && previous) observer.publishGroups(previous, workspaces, true)
      }
      const cursor = more ? (observer.getPages()?.next ?? null) : null
      // Keep the last usable result if a retry cannot inspect any folders.
      let remainingPartialRetries = 2
      let usable: ProjectPages | undefined
      const { pages } = await retryCloudRead(
        async () => {
          const request = port.groups(binding, cursor)
          if (more) this.pendingPage = request
          try {
            const page = await request
            const pages = mergeProjectPage(previous, cursor, page)
            if (page.groups.length) usable = pages
            return {
              pages: page.partial && !page.groups.length && usable ? usable : pages,
              retry:
                page.partial && !page.nextCursor && (!usable || remainingPartialRetries-- > 0),
            }
          } catch (error) {
            if (!usable || signal.aborted || cloudReadScopeMessage(error)) throw error
            return { pages: usable, retry: remainingPartialRetries-- > 0 }
          } finally {
            if (this.pendingPage === request) this.pendingPage = undefined
          }
        },
        signal,
        ({ retry }) => retry,
        onExtendedRetry && (() => onExtendedRetry("folders")),
      )
      if (!current()) return
      projectPagesByPort.set(this.port, { binding: scopeKey, pages })
      this.pageScope = scopeKey
      observer.publishGroups(pages, workspaces, false)
    } catch (error) {
      const scopeMessage = cloudReadScopeMessage(error)
      if (current())
        observer.publishProjects({
          ...observer.getSnapshot().projects!,
          loading: false,
          retrying: false,
          paging: undefined,
          scopeInvalid: !!scopeMessage,
          error: scopeMessage ?? "Could not check session folders.",
        })
    } finally {
      if (this.read === read) this.read = undefined
    }
  }
}
