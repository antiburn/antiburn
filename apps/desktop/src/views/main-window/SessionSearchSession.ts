import type { SessionSearchScope } from "../../lib/sessionSearchScope"
import { searchSessions, type SessionSearchEntry } from "../../lib/sessionSearchIpc"

export function sessionSearchIdentity(entry: SessionSearchEntry): string {
  return `session:${encodeURIComponent(JSON.stringify([entry.environmentKey, entry.agent, entry.sessionId]))}`
}

/** Keep queries transient and ignore responses from obsolete search generations. */
export class SessionSearchSession {
  private scope: SessionSearchScope | null | undefined
  private snapshot = {
    query: "",
    results: [] as SessionSearchEntry[],
    expanded: false,
    loading: false,
    indexing: false,
    error: false,
    nextCursor: null as string | null,
    hasMore: false,
  }
  private listeners = new Set<() => void>()
  private generation = 0
  private timer: ReturnType<typeof setTimeout> | undefined

  private readonly search: typeof searchSessions

  constructor(search: typeof searchSessions = searchSessions) {
    this.search = search
  }

  getSnapshot = () => this.snapshot
  subscribe = (listener: () => void) => {
    this.listeners.add(listener)
    if (this.listeners.size === 1 && this.snapshot.query.trim()) this.retry()
    return () => {
      this.listeners.delete(listener)
      if (!this.listeners.size) {
        this.generation += 1
        clearTimeout(this.timer)
      }
    }
  }
  private update(change: Partial<typeof this.snapshot>) {
    this.snapshot = { ...this.snapshot, ...change }
    for (const listener of this.listeners) listener()
  }
  setScope = (scope: SessionSearchScope | null) => {
    this.scope = scope
    this.reset(this.snapshot.query, 0)
  }
  setQuery = (value: string) => {
    const query = value.slice(0, 200)
    if (query === this.snapshot.query) return
    this.reset(query, 100)
  }
  retry = () => this.reset(this.snapshot.query, 0)

  private reset(query: string, delay: number) {
    const generation = ++this.generation
    clearTimeout(this.timer)
    this.update({
      query,
      results: [],
      expanded: false,
      loading: !!query.trim(),
      indexing: false,
      error: false,
      nextCursor: null,
      hasMore: false,
    })
    if (query.trim() && this.listeners.size)
      this.timer = setTimeout(() => void this.request(generation, false), delay)
  }

  more = () => {
    if (this.snapshot.loading || this.snapshot.indexing || this.snapshot.error) return
    if (!this.snapshot.expanded) {
      this.update({ expanded: true })
      if (this.snapshot.results.length > 5) return
    }
    if (this.snapshot.hasMore && this.snapshot.nextCursor)
      void this.request(this.generation, true)
  }

  private async request(generation: number, append: boolean) {
    const { query, nextCursor } = this.snapshot
    this.update({ loading: true, error: false })
    try {
      const page = await this.search(
        query.trim(),
        append ? nextCursor : null,
        ...(this.scope === undefined ? [] : [this.scope]),
      )
      if (generation !== this.generation || !this.listeners.size) return
      const results = append ? [...this.snapshot.results, ...page.results] : page.results
      this.update({
        results: [
          ...new Map(results.map((entry) => [sessionSearchIdentity(entry), entry])).values(),
        ],
        loading: false,
        indexing: page.indexing,
        error: false,
        nextCursor: page.nextCursor,
        hasMore: page.hasMore,
      })
      if (page.indexing)
        this.timer = setTimeout(() => void this.request(generation, false), 500)
    } catch {
      if (generation !== this.generation || !this.listeners.size) return
      this.update({ loading: false, indexing: false, error: true })
    }
  }
}
