/**
 * The webview's view of the session lifecycle registry.
 *
 * One tracker per window follows `session:lifecycle` and the versioned
 * live snapshot, in that order: the listener attaches first, events buffer
 * while the snapshot is in flight, and the snapshot then becomes the base
 * that only higher-sequence deltas may move. An in-flight snapshot that
 * resolves older than what events already built is discarded rather than
 * applied. `resync` re-reads the snapshot.
 *
 * Sequences are global across every session event scope, so gaps between
 * lifecycle events are normal. Only `resync` means events were lost.
 *
 * The snapshot's rows are bounded; its counts are not. `working`, `total`,
 * and `anonymous` come from the snapshot and then from the counts the
 * registry stamps on the last lifecycle event of each atomic batch, so a
 * surface that asks "is anything working" never counts a truncated list.
 *
 * A list that shows rows the bounded snapshot omitted registers those
 * identities as an interest. The tracker asks the registry for them by
 * name (`get_live_sessions_for`) at one sequence, with one read in flight
 * and the union chunked to the command's limit. Every identity then has
 * evidence: present at a sequence, or absent at a sequence. A delta or a
 * presence row moves a key only when it is newer than that key's evidence,
 * and nothing below the base snapshot's sequence moves anything. A complete
 * snapshot needs no presence overlay: a key it lacks is absent.
 *
 * Anonymous activity — `activity` with a null session — is a watched write
 * the store has not indexed yet. The registry owns its lifetime: it stays
 * until `anonymous_cleared` says a pass covered it or the registry's own
 * quiet window passed. The snapshot carries it too, so a resync replaces
 * it. This tracker keeps no timer for it and `started` does not clear it.
 */

import {
  getLiveSessions,
  getLiveSessionsFor,
  LIVE_PRESENCE_REQUEST_LIMIT,
  onSessionLifecycleEvent,
  type LivePresencePayload,
  type LiveSnapshotPayload,
  type SessionLifecycleEventPayload,
  type SessionRefPayload,
} from "./ipc"
import { environmentKey } from "./presentation/localIdentity"

/**
 * How long the tracker waits before it retries a snapshot read that
 * failed. A failed read after a resync would otherwise leave the state
 * built on a base that lost events, with nothing scheduled to repair it.
 * A failed presence read takes the same path: the re-read snapshot asks
 * for every interest again.
 */
export const SNAPSHOT_RETRY_MS = 5_000

/** One live session, as the tracker mirrors the registry. */
export interface TrackedSession {
  agent: string
  /** Unix seconds of the last observed write. */
  lastActivityAt: number
  /** True after the registry published `quiet` for that write. */
  quiet: boolean
}

/** The tracker's immutable snapshot for `useSyncExternalStore`-style reads. */
export interface LiveSessionsSnapshot {
  /** The registry sequence this state includes. */
  seq: number
  /**
   * True once a registry snapshot was actually read. Without a shell, or
   * before the first read settles, surfaces keep their own fallback state
   * instead of treating an empty tracker as "nothing is live".
   */
  ready: boolean
  /**
   * Live sessions by {@link sessionRefKey}. Presence means active. Absence
   * means inactive only when `complete` holds or the key is in `absent`;
   * see {@link registryActivity}.
   */
  sessions: ReadonlyMap<string, TrackedSession>
  /** Agents with anonymous (not yet indexed) activity the registry still holds. */
  keylessAgents: ReadonlySet<string>
  /** Exact count of sessions with a write inside the quiet window. */
  working: number
  /** Exact count of live sessions, working or quiet. */
  total: number
  /** Exact count of agents with anonymous activity. */
  anonymous: number
  /** True when `sessions` names every live session. */
  complete: boolean
  /** Registered interests the registry said are not live. */
  absent: ReadonlySet<string>
}

const EMPTY_SNAPSHOT: LiveSessionsSnapshot = {
  seq: 0,
  ready: false,
  sessions: new Map(),
  keylessAgents: new Set(),
  working: 0,
  total: 0,
  anonymous: 0,
  complete: false,
  absent: new Set(),
}

/**
 * The identity key for one lifecycle session reference. Matches
 * `localSessionKey` in `presentation/localIdentity.ts`, which serializes
 * `[environmentKey, agent, sessionId]`.
 */
export function sessionRefKey(ref: SessionRefPayload): string {
  return JSON.stringify([ref.environmentKey, ref.agent, ref.sessionId])
}

/** The lifecycle identity of one listed session. */
export function sessionInterest(
  agent: string,
  sessionId: string,
  wslDistro?: string | null,
): SessionRefPayload {
  return { environmentKey: environmentKey(wslDistro), agent, sessionId }
}

/**
 * True when any session is working or any agent has anonymous activity.
 * Decided by the registry's exact counts, never by the bounded rows.
 */
export function hasWorkingActivity(snapshot: LiveSessionsSnapshot): boolean {
  return snapshot.working > 0 || snapshot.anonymous > 0
}

/**
 * What the registry says about one identity: `true` when it is live,
 * `false` when the registry said it is not, and `null` when the tracker has
 * no evidence yet (no base read, or a truncated base and no presence answer).
 * A surface keeps its own state for `null`.
 */
export function registryActivity(snapshot: LiveSessionsSnapshot, key: string): boolean | null {
  if (!snapshot.ready) return null
  if (snapshot.sessions.has(key)) return true
  if (snapshot.complete || snapshot.absent.has(key)) return false
  return null
}

/** What a listed row must carry for the registry to name it. */
export interface ListedSession {
  agent: string
  sessionId?: string | undefined
  wslDistro?: string | null | undefined
  isActive: boolean
}

/** The lifecycle identities of the listed rows that have a session id. */
export function listInterests(entries: readonly ListedSession[]): SessionRefPayload[] {
  const refs: SessionRefPayload[] = []
  for (const entry of entries) {
    if (entry.sessionId) refs.push(sessionInterest(entry.agent, entry.sessionId, entry.wslDistro))
  }
  return refs
}

/**
 * Re-derive each row's active pill from the registry. Presence is what
 * "active" means, not the row's own timestamp-derived flag. A row the
 * registry has not answered yet keeps its flag, so a fresh window never
 * flashes every pill off and an identity the bounded snapshot omitted is
 * not treated as idle.
 */
export function withRegistryActivity<T extends ListedSession>(
  live: LiveSessionsSnapshot,
  entries: T[],
): T[] {
  if (!live.ready) return entries
  return entries.map((entry) => {
    const isActive = registryActivity(
      live,
      sessionRefKey(sessionInterest(entry.agent, entry.sessionId ?? "", entry.wslDistro)),
    )
    if (isActive === null || entry.isActive === isActive) return entry
    return { ...entry, isActive }
  })
}

/** The part of the tracker a list consumer depends on. */
export interface LiveSessionsSource {
  subscribe(listener: () => void): () => void
  getSnapshot(): LiveSessionsSnapshot
  setInterest(owner: object, refs: readonly SessionRefPayload[]): void
  clearInterest(owner: object): void
}

/** Tracks the live registry for one window. See the module doc. */
export class LiveSessionsTracker implements LiveSessionsSource {
  private listeners = new Set<() => void>()
  private generation = 0
  private snapshot: LiveSessionsSnapshot = EMPTY_SNAPSHOT
  private stopListening: (() => void) | null = null
  /** Events held back while a snapshot read is in flight. */
  private buffered: SessionLifecycleEventPayload[] = []
  private syncing = false
  private retryTimer: ReturnType<typeof setTimeout> | null = null
  /** The sequence of the accepted base snapshot. Nothing below it applies. */
  private baseSeq = 0
  /** The sequence of the counts on screen. */
  private aggregateSeq = 0
  /** Per key: the sequence of the evidence that says it is live. */
  private presentAsOf = new Map<string, number>()
  /** Per registered key: the sequence of the evidence that says it is not. */
  private absentAsOf = new Map<string, number>()
  /** Each consumer's named identities. */
  private interests = new Map<object, Map<string, SessionRefPayload>>()
  private presenceInFlight = false
  /** An interest or base change happened while a presence read ran. */
  private presenceDirty = false

  getSnapshot = (): LiveSessionsSnapshot => this.snapshot

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener)
    if (this.listeners.size === 1) this.start()
    return () => {
      this.listeners.delete(listener)
      if (this.listeners.size === 0) this.stop()
    }
  }

  /**
   * Name the identities `owner` shows. Replaces the owner's earlier set. An
   * unchanged set asks nothing; a changed one asks the registry when the
   * base is truncated.
   */
  setInterest = (owner: object, refs: readonly SessionRefPayload[]): void => {
    const next = new Map<string, SessionRefPayload>()
    for (const ref of refs) next.set(sessionRefKey(ref), ref)
    const previous = this.interests.get(owner)
    if (previous && sameKeys(previous, next)) return
    this.interests.set(owner, next)
    this.interestsChanged()
  }

  /** Forget the identities `owner` named. */
  clearInterest = (owner: object): void => {
    if (!this.interests.delete(owner)) return
    this.interestsChanged()
  }

  private start(): void {
    const generation = ++this.generation
    this.resetState()
    // The listener attaches before the snapshot read, so nothing published
    // between the two is lost; it lands in the buffer instead.
    this.syncing = true
    this.buffered = []
    void onSessionLifecycleEvent((event) => {
      if (generation !== this.generation) return
      this.receive(event)
    })
      .then((stop) => {
        if (generation !== this.generation) {
          stop()
          return
        }
        this.stopListening = stop
        void this.readSnapshot(generation)
      })
      .catch(() => {
        if (generation !== this.generation) return
        // No listener means no deltas; the snapshot alone is the state.
        void this.readSnapshot(generation)
      })
  }

  private stop(): void {
    this.generation += 1
    this.stopListening?.()
    this.stopListening = null
    if (this.retryTimer !== null) clearTimeout(this.retryTimer)
    this.retryTimer = null
    this.buffered = []
    this.syncing = false
    this.resetState()
  }

  /** Drop every piece of registry evidence. Interests belong to their owners. */
  private resetState(): void {
    this.snapshot = EMPTY_SNAPSHOT
    this.baseSeq = 0
    this.aggregateSeq = 0
    this.presentAsOf = new Map()
    this.absentAsOf = new Map()
    this.presenceInFlight = false
    this.presenceDirty = false
  }

  private receive(event: SessionLifecycleEventPayload): void {
    if (event.kind === "resync") {
      // Events were lost. The snapshot is the recovery path; buffering
      // starts now so nothing published during the read is dropped.
      this.syncing = true
      this.buffered = []
      void this.readSnapshot(this.generation)
      return
    }
    if (this.syncing) {
      this.buffered.push(event)
      return
    }
    this.apply(event)
  }

  private async readSnapshot(generation: number): Promise<void> {
    let snapshot: LiveSnapshotPayload | null = null
    let failed = false
    try {
      snapshot = await getLiveSessions()
    } catch {
      // A shell answered nothing it should have. The event-built state
      // stands, and the retry below repairs the lost base.
      failed = true
    }
    if (generation !== this.generation) return
    this.syncing = false
    const buffered = this.buffered
    this.buffered = []
    if (failed) this.scheduleRetry(generation)
    let accepted = false
    if (snapshot && snapshot.seq >= this.snapshot.seq) {
      this.applySnapshot(snapshot)
      accepted = true
    } else if (snapshot) {
      // An in-flight snapshot older than what the deltas already built.
      // The event-derived state stands; the read still proves a registry.
      this.publish({ ready: true })
    }
    for (const event of buffered) this.apply(event)
    // Every accepted truncated base asks for every interest again: the
    // rows it omitted are unknown until the registry names them.
    if (accepted && !this.snapshot.complete) this.requestPresence()
  }

  /**
   * Make `snapshot` the base. Evidence newer than the base survives it: a
   * presence answer that raced ahead of the snapshot read is not undone.
   */
  private applySnapshot(snapshot: LiveSnapshotPayload): void {
    const seq = snapshot.seq
    const sessions = new Map<string, TrackedSession>()
    const presentAsOf = new Map<string, number>()
    for (const live of snapshot.sessions) {
      const key = sessionRefKey(live.session)
      sessions.set(key, {
        agent: live.agent,
        lastActivityAt: live.lastActivityAt,
        quiet: live.quiet,
      })
      presentAsOf.set(key, seq)
    }
    for (const [key, asOf] of this.presentAsOf) {
      if (asOf <= seq) continue
      const current = this.snapshot.sessions.get(key)
      if (!current) continue
      sessions.set(key, current)
      presentAsOf.set(key, asOf)
    }
    const absentAsOf = new Map<string, number>()
    for (const [key, asOf] of this.absentAsOf) {
      if (asOf <= seq) continue
      absentAsOf.set(key, asOf)
      sessions.delete(key)
      presentAsOf.delete(key)
    }
    const keylessAgents = new Set<string>()
    for (const live of snapshot.anonymous) keylessAgents.add(live.agent)
    this.baseSeq = seq
    this.aggregateSeq = seq
    this.presentAsOf = presentAsOf
    this.absentAsOf = absentAsOf
    this.publish({
      seq,
      ready: true,
      sessions,
      keylessAgents,
      working: snapshot.working,
      total: snapshot.total,
      anonymous: snapshot.anonymous.length,
      complete: snapshot.sessions.length >= snapshot.total,
      absent: new Set(absentAsOf.keys()),
    })
  }

  private apply(event: SessionLifecycleEventPayload): void {
    if (event.seq <= this.baseSeq) return
    const counts =
      event.aggregate && event.seq > this.aggregateSeq
        ? {
            working: event.aggregate.working,
            total: event.aggregate.total,
            anonymous: event.aggregate.anonymous,
          }
        : {}
    if (event.aggregate && event.seq > this.aggregateSeq) this.aggregateSeq = event.seq
    const seq = Math.max(this.snapshot.seq, event.seq)
    switch (event.kind) {
      case "started":
      case "activity": {
        if (event.session === null) {
          this.publish({ ...counts, seq, keylessAgents: this.withKeyless(event.agent) })
          return
        }
        const key = sessionRefKey(event.session)
        if (event.seq <= this.evidence(key)) {
          this.publish({ ...counts, seq })
          return
        }
        const sessions = new Map(this.snapshot.sessions)
        sessions.set(key, { agent: event.agent, lastActivityAt: event.at, quiet: false })
        this.presentAsOf.set(key, event.seq)
        // A start does not clear anonymous state: only the registry's
        // `anonymous_cleared` says which touches a pass accounted for.
        this.publish({ ...counts, seq, sessions, absent: this.forgetAbsent(key) })
        return
      }
      case "quiet": {
        const key = sessionRefKey(event.session)
        const current = this.snapshot.sessions.get(key)
        if (!current || event.seq <= this.evidence(key)) {
          this.publish({ ...counts, seq })
          return
        }
        const sessions = new Map(this.snapshot.sessions)
        sessions.set(key, { ...current, quiet: true })
        this.presentAsOf.set(key, event.seq)
        this.publish({ ...counts, seq, sessions })
        return
      }
      case "idle": {
        const key = sessionRefKey(event.session)
        if (event.seq <= this.evidence(key)) {
          this.publish({ ...counts, seq })
          return
        }
        const sessions = new Map(this.snapshot.sessions)
        sessions.delete(key)
        this.presentAsOf.delete(key)
        this.publish({ ...counts, seq, sessions, absent: this.recordAbsent(key, event.seq) })
        return
      }
      case "anonymous_cleared": {
        this.publish({ ...counts, seq, keylessAgents: this.withoutKeyless(event.agent) })
        return
      }
      case "resync":
        return
    }
  }

  /** The newest sequence that said anything about `key`. */
  private evidence(key: string): number {
    return Math.max(this.presentAsOf.get(key) ?? 0, this.absentAsOf.get(key) ?? 0)
  }

  /** Every identity some owner shows, by key. */
  private interestUnion(): Map<string, SessionRefPayload> {
    const union = new Map<string, SessionRefPayload>()
    for (const refs of this.interests.values()) {
      for (const [key, ref] of refs) union.set(key, ref)
    }
    return union
  }

  private interestsChanged(): void {
    // Absence evidence lives only for registered interests.
    const union = this.interestUnion()
    let pruned = false
    for (const key of this.absentAsOf.keys()) {
      if (union.has(key)) continue
      this.absentAsOf.delete(key)
      pruned = true
    }
    if (pruned) this.publish({ absent: new Set(this.absentAsOf.keys()) })
    this.requestPresence()
  }

  /**
   * Ask the registry about every interest the base did not answer, unless
   * the base names every live session or a read is in flight. A read in
   * flight marks the request dirty and runs again when it ends.
   */
  private requestPresence(): void {
    if (!this.snapshot.ready || this.snapshot.complete || this.syncing) return
    if (this.presenceInFlight) {
      this.presenceDirty = true
      return
    }
    const refs: SessionRefPayload[] = []
    for (const [key, ref] of this.interestUnion()) {
      if (this.evidence(key) < this.baseSeq) refs.push(ref)
    }
    if (refs.length === 0) return
    void this.readPresence(this.generation, refs)
  }

  /** One presence read: the union in bounded chunks, one request at a time. */
  private async readPresence(generation: number, refs: SessionRefPayload[]): Promise<void> {
    this.presenceInFlight = true
    this.presenceDirty = false
    let failed = false
    for (let start = 0; start < refs.length && !failed; start += LIVE_PRESENCE_REQUEST_LIMIT) {
      const chunk = refs.slice(start, start + LIVE_PRESENCE_REQUEST_LIMIT)
      let presence: LivePresencePayload | null = null
      try {
        presence = await getLiveSessionsFor(chunk)
      } catch {
        failed = true
      }
      if (generation !== this.generation) return
      if (presence) this.applyPresence(presence)
    }
    this.presenceInFlight = false
    if (failed) {
      // The bounded retry re-reads the base, and a truncated base asks for
      // every interest again.
      this.scheduleRetry(generation)
      return
    }
    if (this.presenceDirty) this.requestPresence()
  }

  /**
   * Merge one presence answer. Nothing below the base applies; a key moves
   * only when the answer is newer than the key's own evidence; a key no
   * owner shows any more is ignored.
   */
  private applyPresence(presence: LivePresencePayload): void {
    if (presence.seq < this.baseSeq) return
    const union = this.interestUnion()
    const sessions = new Map(this.snapshot.sessions)
    let changed = false
    for (const live of presence.present) {
      const key = sessionRefKey(live.session)
      if (!union.has(key) || presence.seq <= this.evidence(key)) continue
      sessions.set(key, {
        agent: live.agent,
        lastActivityAt: live.lastActivityAt,
        quiet: live.quiet,
      })
      this.presentAsOf.set(key, presence.seq)
      this.absentAsOf.delete(key)
      changed = true
    }
    for (const ref of presence.absent) {
      const key = sessionRefKey(ref)
      if (!union.has(key) || presence.seq <= this.evidence(key)) continue
      sessions.delete(key)
      this.presentAsOf.delete(key)
      this.absentAsOf.set(key, presence.seq)
      changed = true
    }
    if (!changed) return
    this.publish({ sessions, absent: new Set(this.absentAsOf.keys()) })
  }

  /** Record that `key` is not live at `seq`, when some owner shows it. */
  private recordAbsent(key: string, seq: number): ReadonlySet<string> {
    if (!this.interestUnion().has(key)) return this.snapshot.absent
    this.absentAsOf.set(key, seq)
    if (this.snapshot.absent.has(key)) return this.snapshot.absent
    const absent = new Set(this.snapshot.absent)
    absent.add(key)
    return absent
  }

  private forgetAbsent(key: string): ReadonlySet<string> {
    if (!this.absentAsOf.delete(key)) return this.snapshot.absent
    const absent = new Set(this.snapshot.absent)
    absent.delete(key)
    return absent
  }

  /** Re-read the snapshot once per interval while reads keep failing. */
  private scheduleRetry(generation: number): void {
    if (this.retryTimer !== null) return
    this.retryTimer = setTimeout(() => {
      this.retryTimer = null
      if (generation !== this.generation) return
      this.syncing = true
      this.buffered = []
      void this.readSnapshot(generation)
    }, SNAPSHOT_RETRY_MS)
  }

  private withKeyless(agent: string): ReadonlySet<string> {
    if (this.snapshot.keylessAgents.has(agent)) return this.snapshot.keylessAgents
    const keylessAgents = new Set(this.snapshot.keylessAgents)
    keylessAgents.add(agent)
    return keylessAgents
  }

  private withoutKeyless(agent: string): ReadonlySet<string> {
    if (!this.snapshot.keylessAgents.has(agent)) return this.snapshot.keylessAgents
    const keylessAgents = new Set(this.snapshot.keylessAgents)
    keylessAgents.delete(agent)
    return keylessAgents
  }

  private publish(change: Partial<LiveSessionsSnapshot>): void {
    this.snapshot = { ...this.snapshot, ...change }
    for (const listener of this.listeners) listener()
  }
}

function sameKeys(a: ReadonlyMap<string, unknown>, b: ReadonlyMap<string, unknown>): boolean {
  if (a.size !== b.size) return false
  for (const key of a.keys()) if (!b.has(key)) return false
  return true
}

/** The one tracker this window shares across its surfaces. */
export const liveSessions = new LiveSessionsTracker()
