import type { SessionSearchScope } from "../../lib/sessionSearchScope"
import {
  cancelDeepSessionSearch,
  continueDeepSessionSearch,
  startDeepSessionSearch,
  type DeepSearchResponse,
} from "../../lib/deepSessionSearchIpc"
import type { SessionEvidenceHit } from "../../lib/sessionEvidenceIpc"

export interface DeepSearchSnapshot {
  phase: "idle" | "searching" | "stopped" | "finished" | "partial" | "failed"
  response: DeepSearchResponse | null
  results: SessionEvidenceHit[]
  settling: boolean
  showProgress?: boolean
}

export interface DeepSearchDependencies {
  start: typeof startDeepSessionSearch
  next: typeof continueDeepSessionSearch
  cancel: typeof cancelDeepSessionSearch
}

const DEFAULT_DEPENDENCIES: DeepSearchDependencies = {
  start: startDeepSessionSearch,
  next: continueDeepSessionSearch,
  cancel: cancelDeepSessionSearch,
}
const EMPTY: DeepSearchSnapshot = {
  phase: "idle",
  response: null,
  results: [],
  settling: false,
  showProgress: false,
}
let nextScanId = 1

type Scan = {
  id: number
  revision: number
  query: string
  stopped: boolean
  inFlight: Promise<void> | null
  cancellation: Promise<void> | null
}

export function deepSessionKey(session: {
  environmentKey: string
  agent: string
  sessionId: string
}): string {
  return JSON.stringify([session.environmentKey, session.agent, session.sessionId])
}

export function stableDeepResults(
  previous: SessionEvidenceHit[],
  response: DeepSearchResponse,
  selected: string | null,
): SessionEvidenceHit[] {
  const invalidated = new Set(response.invalidatedSessions.map(deepSessionKey))
  const incoming = new Map(
    response.results
      .filter((hit) => !invalidated.has(deepSessionKey(hit.session)))
      .map((hit) => [deepSessionKey(hit.session), hit]),
  )
  const pinned = previous.find((hit) => deepSessionKey(hit.session) === selected)
  if (pinned && selected && !invalidated.has(selected) && !incoming.has(selected)) {
    const last = [...incoming.keys()].at(-1)
    if (incoming.size >= 100 && last) incoming.delete(last)
    incoming.set(selected, pinned)
  }
  const ordered: SessionEvidenceHit[] = []
  for (const hit of previous) {
    const key = deepSessionKey(hit.session)
    const replacement = incoming.get(key)
    if (replacement) {
      ordered.push(replacement)
      incoming.delete(key)
    }
  }
  const results = [...ordered, ...incoming.values()].slice(0, 100)
  const oldIndex = previous.findIndex((hit) => deepSessionKey(hit.session) === selected)
  const newIndex = results.findIndex((hit) => deepSessionKey(hit.session) === selected)
  if (oldIndex >= 0 && newIndex >= 0 && oldIndex !== newIndex) {
    const [hit] = results.splice(newIndex, 1)
    results.splice(Math.min(oldIndex, results.length), 0, hit!)
  }
  return results
}

export class DeepSearchSession {
  private scope: SessionSearchScope | null | undefined
  private snapshot = EMPTY
  private listeners = new Set<() => void>()
  private query = ""
  private revision = 0
  private scan: Scan | null = null
  private timer: ReturnType<typeof setTimeout> | null = null
  private idleTimer: ReturnType<typeof setTimeout> | null = null
  private progressTimer: ReturnType<typeof setTimeout> | null = null
  private selected: string | null = null

  private readonly deps: DeepSearchDependencies

  constructor(deps: DeepSearchDependencies = DEFAULT_DEPENDENCIES) {
    this.deps = deps
  }

  getSnapshot = (): DeepSearchSnapshot => this.snapshot

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener)
    return () => {
      this.listeners.delete(listener)
      if (!this.listeners.size) this.close()
    }
  }

  setScope(scope: SessionSearchScope | null): void {
    this.close()
    this.scope = scope
  }

  setQuery(query: string, schedule = true): void {
    if (this.query !== query) {
      this.close()
      this.query = query
    }
    if (this.idleTimer !== null) clearTimeout(this.idleTimer)
    this.idleTimer = null
    const length = Array.from(query.trim()).length
    if (schedule && !this.scan && this.listeners.size && length >= 3 && length <= 200)
      this.idleTimer = setTimeout(() => {
        this.idleTimer = null
        this.start(true)
      }, 600)
  }

  select(key: string | null): void {
    this.selected = key
  }

  start = (automatic = false): void => {
    if (!this.listeners.size || !this.query.trim() || Array.from(this.query).length > 200)
      return
    if (this.scan) return
    if (this.idleTimer !== null) clearTimeout(this.idleTimer)
    this.idleTimer = null
    const scan: Scan = {
      id: nextScanId++,
      revision: this.revision,
      query: this.query,
      stopped: false,
      inFlight: null,
      cancellation: null,
    }
    this.scan = scan
    this.publish({ ...EMPTY, phase: "searching", showProgress: !automatic })
    if (automatic)
      this.progressTimer = setTimeout(() => {
        this.progressTimer = null
        if (this.scan === scan && this.snapshot.phase === "searching")
          this.publish({ ...this.snapshot, showProgress: true })
      }, 500)
    this.run(scan, () =>
      this.deps.start(
        scan.query,
        scan.id,
        scan.revision,
        ...(this.scope === undefined ? [] : [this.scope]),
      ),
    )
  }

  stop = (): void => {
    const scan = this.scan
    if (!scan || scan.stopped || this.snapshot.phase !== "searching") return
    scan.stopped = true
    this.clearTimer()
    this.publish({ ...this.snapshot, phase: "stopped", settling: true })
    scan.cancellation = this.deps.cancel(scan.id, scan.revision, false).catch(() => {
      if (this.scan === scan) this.publish({ ...this.snapshot, phase: "partial" })
    })
    void Promise.all([scan.inFlight, scan.cancellation]).then(() => {
      if (this.scan === scan) this.publish({ ...this.snapshot, settling: false })
    })
  }

  continue = (): void => {
    const scan = this.scan
    if (
      !scan ||
      this.snapshot.settling ||
      scan.inFlight ||
      !this.snapshot.response?.continuationAvailable
    )
      return
    if (!["stopped", "partial", "failed"].includes(this.snapshot.phase)) return
    scan.stopped = false
    this.publish({
      ...this.snapshot,
      phase: "searching",
      showProgress: true,
    })
    this.run(scan, () => this.deps.next(scan.id, scan.revision))
  }

  close = (): void => {
    this.clearTimer()
    if (this.idleTimer !== null) clearTimeout(this.idleTimer)
    if (this.progressTimer !== null) clearTimeout(this.progressTimer)
    this.idleTimer = null
    this.progressTimer = null
    const scan = this.scan
    this.scan = null
    this.revision += 1
    this.selected = null
    if (scan) void this.deps.cancel(scan.id, scan.revision, true).catch(() => {})
    this.publish(EMPTY)
  }

  private run(scan: Scan, request: () => Promise<DeepSearchResponse>): void {
    scan.inFlight = this.read(scan, request).finally(() => {
      scan.inFlight = null
    })
  }

  private async read(scan: Scan, request: () => Promise<DeepSearchResponse>): Promise<void> {
    try {
      const response = await request()
      if (this.scan !== scan || !this.listeners.size) return
      if (response.scanId !== scan.id || response.queryRevision !== scan.revision) {
        this.publish({ ...this.snapshot, phase: "failed" })
        return
      }
      const phase = scan.stopped
        ? "stopped"
        : !response.available
          ? "failed"
          : response.status === "finished_unavailable"
            ? "finished"
            : response.status
      const results = stableDeepResults(this.snapshot.results, response, this.selected)
      if (!this.selected && results[0]) this.selected = deepSessionKey(results[0].session)
      this.publish({
        phase,
        response,
        results,
        settling: this.snapshot.settling,
        showProgress: this.snapshot.showProgress ?? false,
      })
      if (!scan.stopped && response.continuationAvailable && phase === "searching") {
        this.timer = setTimeout(() => {
          this.timer = null
          if (this.scan === scan && !scan.stopped && this.listeners.size)
            this.run(scan, () => this.deps.next(scan.id, scan.revision))
        }, 0)
      }
    } catch {
      if (this.scan === scan)
        this.publish({ ...this.snapshot, phase: scan.stopped ? "stopped" : "failed" })
    }
  }

  private clearTimer(): void {
    if (this.timer !== null) clearTimeout(this.timer)
    this.timer = null
  }

  private publish(snapshot: DeepSearchSnapshot): void {
    this.snapshot = snapshot
    for (const listener of this.listeners) listener()
  }
}
