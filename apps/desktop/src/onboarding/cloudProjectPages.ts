import {
  mergeCloudProjectGroups,
  type CloudPort,
  type CloudProjectCursor,
  type CloudSourceSelection,
  type DesktopCloudProjectGroup,
} from "../lib/cloud"
import { mergeProjectSources } from "./cloudSessionState"

type ProjectPage = Awaited<ReturnType<NonNullable<CloudPort["ongoing"]>["groups"]>>

/** Keep one port's last inspected pages for an exact, verified account binding. */
export const projectPagesByPort = new WeakMap<
  CloudPort,
  { binding: string; pages: ProjectPages }
>()

/** Replace a page and derive folder scope from the connected cursor chain only.
 * Keep disconnected rows visible while their replacement pages are still unread.
 */
export function mergeProjectPage(
  previous: ProjectPages | undefined,
  cursor: CloudProjectCursor | null,
  page: ProjectPage,
): ProjectPages {
  const pages = new Map(previous?.pages).set(JSON.stringify(cursor), page)
  const connected = new Map<string, ProjectPage>()
  let next: CloudProjectCursor | null = null
  let deferred: CloudProjectCursor | null | undefined
  do {
    const key = JSON.stringify(next)
    if (connected.has(key)) throw new Error("Project discovery repeated a page.")
    const current = pages.get(key)
    if (!current) break
    connected.set(key, current)
    if (deferred === undefined && current.retrySamePage) deferred = next
    next = current.nextCursor
  } while (next)

  const verified = [...connected.values()]
  const partial = verified.some((item) => item.partial)
  const groups = verified.reduce<DesktopCloudProjectGroup[]>(
    (groups, { groups: pageGroups }) => mergeCloudProjectGroups(groups, pageGroups),
    [],
  )
  const groupIds = new Set(groups.map(({ id }) => id))
  if (next || partial) {
    for (const group of previous?.groups ?? [])
      if (!groupIds.has(group.id))
        groups.push({ ...group, captureSupported: false, discoveryStale: true })
  }
  const order = new Map(previous?.groups.map(({ id }, index) => [id, index]))
  groups.sort((a, b) => (order.get(a.id) ?? order.size) - (order.get(b.id) ?? order.size))
  const sources = verified.reduce<CloudSourceSelection[]>(
    (sources, { sourceRoots }) => mergeProjectSources(sources, sourceRoots),
    [],
  )
  return { pages: connected, groups, sources, next, deferred, partial }
}

/** Resume deferred work on a visible page before requesting a new page. */
export function nextProjectCursor(pages: ProjectPages | undefined) {
  return pages?.deferred === undefined ? (pages?.next ?? undefined) : pages.deferred
}

/** Display data and loaded pages for one account and verified destination. */
export type ProjectPages = {
  pages: ReadonlyMap<string, ProjectPage>
  groups: DesktopCloudProjectGroup[]
  sources: CloudSourceSelection[]
  next: CloudProjectCursor | null
  deferred: CloudProjectCursor | null | undefined
  partial: boolean
}
