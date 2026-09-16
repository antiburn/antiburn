/**
 * One tracker per window follows the canonical lifecycle registry. It attaches the
 * listener before reading a snapshot. It buffers events during the read and applies
 * only later deltas. A resync requests a new snapshot. Global sequences cover all
 * session scopes, so lifecycle gaps alone do not indicate loss. Exact registry counts
 * decide HUD liveness independently of bounded rows. Lists register interests for
 * omitted identities. One presence request runs at a time, in bounded chunks. Unknown
 * evidence differs from valid sequence zero. Present and absent evidence reject
 * duplicate or older answers. Complete snapshots prove omitted identities absent. The
 * registry alone expires anonymous activity; the tracker has no anonymous timer.
 */

import {
  getLiveSessions,
  getLiveSessionsFor,
  LIVE_PRESENCE_REQUEST_LIMIT,
  onSessionLifecycleEvent,
  type SweepCountsPayload,
  type LivePresencePayload,
  type LiveSnapshotPayload,
  type SessionLifecycleEventPayload,
  type SessionRefPayload,
} from "./ipc"
import { environmentKey } from "./presentation/localIdentity"

/**
 * Failed listener attachments, snapshot reads, and presence reads use this retry interval. A new truncated snapshot
 * requests unanswered interests again.
 */
export const SNAPSHOT_RETRY_MS = 5_000

/** This state mirrors one live registry session. */
export interface TrackedSession {
  agent: string
  /** The timestamp records the last observed write in Unix seconds. */
  lastActivityAt: number
  /** The registry sets this flag when the session becomes quiet. */
  quiet: boolean
}

/** Consumers read this immutable snapshot through external-store subscriptions. */
export interface LiveSessionsSnapshot {
  /** This sequence identifies the latest included lifecycle event or base snapshot. */
  seq: number
  /**
   * A successful registry snapshot makes this state ready. Until then, surfaces keep
   * their fallback state.
   */
  ready: boolean
  /**
   * The map uses {@link sessionRefKey} identities. Missing keys are unknown unless the
   * snapshot is complete or the absent set names them.
   */
  sessions: ReadonlyMap<string, TrackedSession>
  /**
   * The registry retains these agents until a cover or deadline clears their anonymous
   * activity.
   */
  keylessAgents: ReadonlySet<string>
  /** This exact count includes sessions inside the quiet window. */
  working: number
  /** This exact count includes working and quiet sessions. */
  total: number
  /** This exact count includes agents with anonymous activity. */
  anonymous: number
  sweep: readonly SweepCountsPayload[]
  /** A complete base snapshot names every live session. */
  complete: boolean
  /** The registry confirms these registered interests are absent. */
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
  sweep: [],
  complete: false,
  absent: new Set(),
}

/**
 * This key matches `localSessionKey` in `presentation/localIdentity.ts`. Both serialize
 * the environment, agent, and session ID.
 */
export function sessionRefKey(ref: SessionRefPayload): string {
  return JSON.stringify([ref.environmentKey, ref.agent, ref.sessionId])
}

/** Create a lifecycle identity for a listed session. */
export function sessionInterest(
  agent: string,
  sessionId: string,
  wslDistro?: string | null,
): SessionRefPayload {
  return { environmentKey: environmentKey(wslDistro), agent, sessionId }
}

/**
 * Use exact registry counts to determine whether any session or anonymous agent is
 * working.
 */
export function hasWorkingActivity(snapshot: LiveSessionsSnapshot): boolean {
  return snapshot.working > 0 || snapshot.anonymous > 0
}

/**
 * Return true for live keys, false for confirmed absence, and null without evidence.
 * Surfaces keep their fallback state for null.
 */
export function registryActivity(snapshot: LiveSessionsSnapshot, key: string): boolean | null {
  if (!snapshot.ready) return null
  if (snapshot.sessions.has(key)) return true
  if (snapshot.complete || snapshot.absent.has(key)) return false
  return null
}

/** These fields connect a listed row to registry evidence. */
export interface ListedSession {
  agent: string
  sessionId?: string | undefined
  wslDistro?: string | null | undefined
  isActive: boolean
}

/** Collect lifecycle identities for rows with session IDs. */
export function listInterests(entries: readonly ListedSession[]): SessionRefPayload[] {
  const refs: SessionRefPayload[] = []
  for (const entry of entries) {
    if (entry.sessionId)
      refs.push(sessionInterest(entry.agent, entry.sessionId, entry.wslDistro))
  }
  return refs
}

/**
 * Registry evidence decides active pills. Unknown identities keep their existing flags
 * until a presence answer arrives.
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

/** List consumers depend on this external-store interface. */
export interface LiveSessionsSource {
  subscribe(listener: () => void): () => void
  getSnapshot(): LiveSessionsSnapshot
  setInterest(owner: object, refs: readonly SessionRefPayload[]): void
  clearInterest(owner: object): void
}

/** This tracker shares the live registry state within one window. */
export class LiveSessionsTracker implements LiveSessionsSource {
  private listeners = new Set<() => void>()
  private generation = 0
  private snapshot: LiveSessionsSnapshot = EMPTY_SNAPSHOT
  private stopListening: (() => void) | null = null
  /** The buffer retains events during snapshot reads. */
  private buffered: SessionLifecycleEventPayload[] = []
  private syncing = false
  private retryTimer: ReturnType<typeof setTimeout> | null = null
  /** The accepted base sequence rejects older deltas and presence evidence. */
  private baseSeq = 0
  /** This sequence orders aggregate counts independently of presence answers. */
  private aggregateSeq = 0
  private sweepFloor = 0
  /** This map records the sequence of present evidence for each key. */
  private presentAsOf = new Map<string, number>()
  /** This map records absence sequences only for registered interests. */
  private absentAsOf = new Map<string, number>()
  /** Unresolved quiet transitions retain no timestamp and only belong to current interests. */
  private quietAsOf = new Map<string, number>()
  /** Each owner registers the identities its list shows. */
  private interests = new Map<object, Map<string, SessionRefPayload>>()
  /** Each current interest rejects answers from before its registration watermark. */
  private interestSince = new Map<string, number>()
  private presenceInFlight = false
  /** Changes during a presence read require another request after it completes. */
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
   * Replace the identities this owner shows. A changed set requests unanswered
   * interests when the base is truncated.
   */
  setInterest = (owner: object, refs: readonly SessionRefPayload[]): void => {
    const next = new Map<string, SessionRefPayload>()
    for (const ref of refs) next.set(sessionRefKey(ref), ref)
    const previous = this.interests.get(owner)
    if (previous && sameKeys(previous, next)) return
    this.interests.set(owner, next)
    this.interestsChanged()
  }

  /** Remove this owner’s registered interests. */
  clearInterest = (owner: object): void => {
    if (!this.interests.delete(owner)) return
    this.interestsChanged()
  }

  private start(): void {
    const generation = ++this.generation
    this.resetState()
    this.attachListener(generation)
  }

  private attachListener(generation: number): void {
    // The listener buffers events before the snapshot read starts.
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
        this.scheduleRetry(generation)
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

  /** Clear registry evidence without changing owner interests. */
  private resetState(): void {
    this.snapshot = EMPTY_SNAPSHOT
    this.baseSeq = 0
    this.aggregateSeq = 0
    this.sweepFloor = 0
    this.presentAsOf = new Map()
    this.absentAsOf = new Map()
    this.quietAsOf = new Map()
    this.interestSince = new Map([...this.interestUnion().keys()].map((key) => [key, 0]))
    this.presenceInFlight = false
    this.presenceDirty = false
  }

  private receive(event: SessionLifecycleEventPayload): void {
    if (event.kind === "resync") {
      // Recovery removes positive scoped evidence until the registry supplies current counts.
      this.sweepFloor = Math.max(this.sweepFloor, event.seq)
      this.publish({ sweep: [] })
      // A recovery marker starts buffering before the new snapshot read.
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
      // A failed read preserves event-derived state until the retry succeeds.
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
      // Newer event-derived state takes precedence over a stale snapshot.
      this.publish({ ready: true })
    }
    for (const event of buffered) this.apply(event)
    // An accepted truncated snapshot leaves omitted interests unknown until the
    // registry answers them.
    if (accepted && !this.snapshot.complete) this.requestPresence()
  }

  /** Accept the base without discarding newer presence evidence. */
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
    for (const [key, asOf] of this.quietAsOf) {
      if (asOf <= seq) this.quietAsOf.delete(key)
    }
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
      sweep: seq >= this.sweepFloor ? (snapshot.sweep ?? []) : [],
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
            sweep: event.seq >= this.sweepFloor ? (event.aggregate.sweep ?? []) : [],
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
        if (this.hasEvidenceAtOrAfter(key, event.seq)) {
          this.publish({ ...counts, seq })
          return
        }
        const sessions = new Map(this.snapshot.sessions)
        sessions.set(key, { agent: event.agent, lastActivityAt: event.at, quiet: false })
        this.presentAsOf.set(key, event.seq)
        this.quietAsOf.delete(key)
        // Only `anonymous_cleared` confirms which anonymous activity the registry ends.
        this.publish({ ...counts, seq, sessions, absent: this.forgetAbsent(key) })
        return
      }
      case "quiet": {
        const key = sessionRefKey(event.session)
        const current = this.snapshot.sessions.get(key)
        if (this.hasEvidenceAtOrAfter(key, event.seq)) {
          this.publish({ ...counts, seq })
          return
        }
        if (!current) {
          if (this.interestSince.has(key)) this.quietAsOf.set(key, event.seq)
          this.publish({ ...counts, seq })
          this.requestPresence()
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
        if (this.hasEvidenceAtOrAfter(key, event.seq)) {
          this.publish({ ...counts, seq })
          return
        }
        const sessions = new Map(this.snapshot.sessions)
        sessions.delete(key)
        this.presentAsOf.delete(key)
        this.quietAsOf.delete(key)
        this.publish({ ...counts, seq, sessions, absent: this.recordAbsent(key, event.seq) })
        return
      }
      case "anonymous_cleared": {
        this.publish({ ...counts, seq, keylessAgents: this.withoutKeyless(event.agent) })
        return
      }
      case "sweep_changed":
        this.publish({ ...counts, seq })
        return
      case "resync":
        return
    }
  }

  /** Missing evidence is unknown, but sequence zero is valid evidence. */
  private hasEvidenceAtOrAfter(key: string, seq: number): boolean {
    const quiet = this.quietAsOf.get(key)
    return (quiet !== undefined && quiet >= seq) || this.hasRowEvidenceAtOrAfter(key, seq)
  }

  /** A quiet transition alone cannot supply the last activity timestamp. */
  private hasRowEvidenceAtOrAfter(key: string, seq: number): boolean {
    const present = this.presentAsOf.get(key)
    const absent = this.absentAsOf.get(key)
    return (present !== undefined && present >= seq) || (absent !== undefined && absent >= seq)
  }

  /** Combine all owner interests by full identity. */
  private interestUnion(): Map<string, SessionRefPayload> {
    const union = new Map<string, SessionRefPayload>()
    for (const refs of this.interests.values()) {
      for (const [key, ref] of refs) union.set(key, ref)
    }
    return union
  }

  private interestsChanged(): void {
    // Only registered interests retain absence evidence.
    const union = this.interestUnion()
    for (const key of this.interestSince.keys()) {
      if (!union.has(key)) {
        this.interestSince.delete(key)
        this.quietAsOf.delete(key)
      }
    }
    for (const key of union.keys()) {
      if (!this.interestSince.has(key)) this.interestSince.set(key, this.snapshot.seq)
    }
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
   * Request unanswered interests from a truncated base. A change during a read marks
   * the request dirty for another pass.
   */
  private requestPresence(): void {
    if (!this.snapshot.ready || this.snapshot.complete || this.syncing) return
    if (this.presenceInFlight) {
      this.presenceDirty = true
      return
    }
    const refs: SessionRefPayload[] = []
    for (const [key, ref] of this.interestUnion()) {
      if (!this.hasRowEvidenceAtOrAfter(key, this.baseSeq)) refs.push(ref)
    }
    if (refs.length === 0) return
    void this.readPresence(this.generation, refs)
  }

  /** Read the interest union in bounded chunks, one request at a time. */
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
      // A failed presence read retries through a new base snapshot.
      this.scheduleRetry(generation)
      return
    }
    if (this.presenceDirty) this.requestPresence()
  }

  /**
   * Merge an answer only for current interests at or above the base sequence.
   * Accept unknown keys or answers newer than their known evidence.
   */
  private applyPresence(presence: LivePresencePayload): void {
    if (presence.seq < this.baseSeq) return
    const union = this.interestUnion()
    const sessions = new Map(this.snapshot.sessions)
    let changed = false
    for (const live of presence.present) {
      const key = sessionRefKey(live.session)
      if (
        !union.has(key) ||
        presence.seq < (this.interestSince.get(key) ?? this.baseSeq) ||
        this.hasRowEvidenceAtOrAfter(key, presence.seq)
      )
        continue
      const quiet = this.quietAsOf.get(key)
      sessions.set(key, {
        agent: live.agent,
        lastActivityAt: live.lastActivityAt,
        quiet: quiet !== undefined && quiet > presence.seq ? true : live.quiet,
      })
      this.presentAsOf.set(key, Math.max(presence.seq, quiet ?? presence.seq))
      this.quietAsOf.delete(key)
      this.absentAsOf.delete(key)
      changed = true
    }
    for (const ref of presence.absent) {
      const key = sessionRefKey(ref)
      if (
        !union.has(key) ||
        presence.seq < (this.interestSince.get(key) ?? this.baseSeq) ||
        this.hasEvidenceAtOrAfter(key, presence.seq)
      )
        continue
      sessions.delete(key)
      this.presentAsOf.delete(key)
      this.quietAsOf.delete(key)
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
      if (this.stopListening) void this.readSnapshot(generation)
      else this.attachListener(generation)
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

/** All surfaces in this window share this tracker. */
export const liveSessions = new LiveSessionsTracker()
