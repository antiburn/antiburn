import { getMainWindowVisible, onMainWindowVisibilityChanged } from "../../../lib/ipc"
import {
  getQuotaAccounts,
  getQuotaUsage,
  type QuotaAccountPayload,
  type QuotaAccountsPayload,
  type QuotaLanePayload,
  type QuotaUsagePayload,
  type QuotaUsageRequest,
} from "../../../lib/providerUsageIpc"
import { traceEvent } from "../../../lib/perfTrace"
import { SurfaceExposureTracker } from "../../../lib/surfaceExposure"
import {
  isCustomRange,
  isQuotaRangePreset,
  resolveQuotaRange,
  type QuotaRange,
  type QuotaRangePreset,
  type QuotaRangeSelection,
} from "./quotaSeries"
import { readQuotaViewPrefs, writeQuotaViewPrefs } from "./quotaViewPrefs"

/** How long a cached usage reading stays fresh. The Quota screen does not
 *  live-update, so a reading this fresh is as good as a new query. An
 *  explicit refresh always bypasses this cache. */
export const QUOTA_USAGE_CACHE_TTL_MS = 10 * 60 * 1000

interface QuotaUsageCacheEntry {
  usage: QuotaUsagePayload
  now: number
  loadedAt: number
}

export interface QuotaSelection {
  provider: string
  accountKey: string
  lane: string
}

export interface QuotaSnapshot {
  /** Every observed `(provider, account)`, or null until the first load. */
  accounts: QuotaAccountPayload[] | null
  accountsError: boolean
  selection: QuotaSelection | null
  range: QuotaRangeSelection
  usage: QuotaUsagePayload | null
  usageError: boolean
  loading: boolean
  /** Epoch seconds captured at the last usage refresh. */
  now: number
}

export interface QuotaAdapter {
  getAccounts(): Promise<QuotaAccountsPayload>
  getUsage(request: QuotaUsageRequest): Promise<QuotaUsagePayload>
  getVisible(): Promise<boolean>
  onVisible(handler: (visible: boolean) => void): Promise<() => void>
  now(): number
}

const productionAdapter: QuotaAdapter = {
  getAccounts: () => getQuotaAccounts(),
  getUsage: (request) => getQuotaUsage(request),
  getVisible: () => getMainWindowVisible(),
  onVisible: (handler) => onMainWindowVisibilityChanged(handler),
  now: () => Math.floor(Date.now() / 1000),
}

function findAccount(
  accounts: readonly QuotaAccountPayload[],
  provider: string,
  accountKey: string,
): QuotaAccountPayload | null {
  return (
    accounts.find(
      (account) => account.provider === provider && account.accountKey === accountKey,
    ) ?? null
  )
}

function findLane(
  account: QuotaAccountPayload | null,
  laneId: string | null,
): QuotaLanePayload | null {
  if (!account || !laneId) return null
  return account.lanes.find((lane) => lane.lane === laneId) ?? null
}

/** The account's own weekly lane, not a model-scoped one, for the five-hour range fallback. */
function weeklyLaneOf(account: QuotaAccountPayload | null): QuotaLanePayload | null {
  return account?.lanes.find((lane) => lane.lane === "weekly") ?? null
}

/** The lane a fresh selection defaults to: the weekly lane, else the account's first lane. */
function defaultLane(account: QuotaAccountPayload): QuotaLanePayload | null {
  return weeklyLaneOf(account) ?? account.lanes[0] ?? null
}

function selectionEquals(left: QuotaSelection, right: QuotaSelection | null): boolean {
  return (
    right != null &&
    left.provider === right.provider &&
    left.accountKey === right.accountKey &&
    left.lane === right.lane
  )
}

/** The range half of a usage cache key: the preset name, or the custom
 *  range's own epochs. Keyed on the selection a reader made, not on the
 *  epochs a preset resolves to, since those move with `now`. */
function rangeCacheKey(range: QuotaRangeSelection): string {
  return isCustomRange(range) ? `custom:${range.startEpoch}:${range.endEpoch}` : range
}

function quotaUsageCacheKey(selection: QuotaSelection, range: QuotaRangeSelection): string {
  return `${selection.provider}|${selection.accountKey}|${selection.lane}|${rangeCacheKey(range)}`
}

/**
 * Own the Quota section's reads: the observed provider accounts and their
 * lanes, and one lane's usage over a chosen range. Loads only while a viewer
 * is active and the main window is visible. This screen does not live-update:
 * a reload happens only on activation, a selection or range change, `open`,
 * or the Retry button.
 */
export class QuotaSession {
  private readonly adapter: QuotaAdapter
  private snapshot: QuotaSnapshot = {
    accounts: null,
    accountsError: false,
    selection: null,
    range: "thisWeek",
    usage: null,
    usageError: false,
    loading: false,
    now: Math.floor(Date.now() / 1000),
  }
  private readonly listeners = new Set<() => void>()
  private readonly activeListeners = new Set<() => void>()
  private readonly stops: Array<() => void> = []
  private generation = 0
  private workVersion = 0
  private accountsVersion = 0
  private usageVersion = 0
  private visible = false
  private initialized = false
  private active = false
  private accountsTask: Promise<void> | null = null
  private accountsDirty = false
  private usageTask: Promise<void> | null = null
  private usageDirty = false
  private readonly usageCache = new Map<string, QuotaUsageCacheEntry>()
  private readonly exposure = new SurfaceExposureTracker()
  /** The reader's saved account, lane, and range, read once at construction.
   *  `resolveSelection` applies the account and lane the first time accounts
   *  load with no selection yet; the constructor below applies the range
   *  preset directly to the initial snapshot. */
  private readonly savedPrefs = readQuotaViewPrefs()

  constructor(adapter: QuotaAdapter = productionAdapter) {
    this.adapter = adapter
    if (isQuotaRangePreset(this.savedPrefs.rangePreset)) {
      this.snapshot = { ...this.snapshot, range: this.savedPrefs.rangePreset }
    }
  }

  getSnapshot = (): QuotaSnapshot => this.snapshot
  subscribe = (listener: () => void): (() => void) => this.attach(listener, true)
  subscribeInactive = (listener: () => void): (() => void) => this.attach(listener, false)

  private update(patch: Partial<QuotaSnapshot>): void {
    this.snapshot = { ...this.snapshot, ...patch }
    this.syncExposure()
    for (const listener of this.listeners) listener()
  }

  private attach(listener: () => void, active: boolean): () => void {
    this.listeners.add(listener)
    if (active) this.activeListeners.add(listener)
    if (this.listeners.size === 1) void this.start()
    this.syncActive()
    return () => {
      this.listeners.delete(listener)
      this.activeListeners.delete(listener)
      this.syncActive()
      if (this.listeners.size === 0) this.dispose()
    }
  }

  private async listen(generation: number, pending: Promise<() => void>): Promise<void> {
    const stop = await pending.catch(() => null)
    if (!stop) return
    if (generation !== this.generation) stop()
    else this.stops.push(stop)
  }

  private async start(): Promise<void> {
    const generation = ++this.generation
    let visibilityRevision = 0
    await this.listen(
      generation,
      this.adapter.onVisible((visible) => {
        if (generation !== this.generation) return
        visibilityRevision += 1
        this.visible = visible
        this.syncActive()
      }),
    )
    const revision = visibilityRevision
    const visible = await this.adapter.getVisible().catch(() => false)
    if (generation !== this.generation) return
    if (revision === visibilityRevision) this.visible = visible
    this.initialized = true
    this.syncActive()
  }

  private syncActive(): void {
    const active = this.initialized && this.visible && this.activeListeners.size > 0
    if (active === this.active) return
    this.active = active
    this.workVersion += 1
    if (!active) {
      this.exposure.conceal("quota")
      return
    }
    traceEvent("quota.refresh", { trigger: "activate" })
    this.loadAccounts()
    // A caller outside the Quota screen (a session detail row's `open`) may
    // have set a selection and queued a usage load while this session was
    // inactive, and `loadUsage` returned early without starting it. A
    // custom range ignores the lane lookup, so it can run before accounts
    // arrive; with no selection yet there is nothing to resolve, so this
    // only fires once `open` (or an earlier active load) has set one.
    if (this.usageDirty && this.snapshot.selection) this.loadUsage()
  }

  /** Resolve the account and lane to use after accounts load: keep the current
   *  selection when it still exists, falling back to its account's default
   *  lane. With no current selection yet (the very first resolution), prefer
   *  the reader's saved account and lane when the payload still carries
   *  them; otherwise fall back to the first account's default lane. */
  private resolveSelection(accounts: readonly QuotaAccountPayload[]): QuotaSelection | null {
    const current = this.snapshot.selection
    if (current) {
      const account = findAccount(accounts, current.provider, current.accountKey)
      if (account) {
        const lane = findLane(account, current.lane) ?? defaultLane(account)
        if (lane)
          return { provider: account.provider, accountKey: account.accountKey, lane: lane.lane }
      }
    } else if (this.savedPrefs.provider != null && this.savedPrefs.accountKey != null) {
      const account = findAccount(
        accounts,
        this.savedPrefs.provider,
        this.savedPrefs.accountKey,
      )
      if (account) {
        const lane = findLane(account, this.savedPrefs.lane ?? null) ?? defaultLane(account)
        if (lane)
          return { provider: account.provider, accountKey: account.accountKey, lane: lane.lane }
      }
    }
    const first = accounts[0]
    if (!first) return null
    const lane = defaultLane(first)
    return lane
      ? { provider: first.provider, accountKey: first.accountKey, lane: lane.lane }
      : null
  }

  selectAccount = (provider: string, accountKey: string): void => {
    const account = findAccount(this.snapshot.accounts ?? [], provider, accountKey)
    if (!account) return
    const lane =
      findLane(account, this.snapshot.selection?.lane ?? null) ?? defaultLane(account)
    if (!lane) return
    this.update({ selection: { provider, accountKey, lane: lane.lane } })
    writeQuotaViewPrefs({ provider, accountKey, lane: lane.lane })
    traceEvent("quota.refresh", { trigger: "selectAccount" })
    this.loadUsage()
  }

  selectLane = (lane: string): void => {
    const selection = this.snapshot.selection
    if (!selection) return
    this.update({ selection: { ...selection, lane } })
    // Written together with the account: a lane saved on its own, with no
    // account beside it, could never be restored later.
    writeQuotaViewPrefs({
      provider: selection.provider,
      accountKey: selection.accountKey,
      lane,
    })
    traceEvent("quota.refresh", { trigger: "selectLane" })
    this.loadUsage()
  }

  selectRange = (range: QuotaRangePreset): void => {
    this.update({ range })
    writeQuotaViewPrefs({ rangePreset: range })
    traceEvent("quota.refresh", { trigger: "selectRange" })
    this.loadUsage()
  }

  /**
   * Open a specific account, lane, and explicit range, as a session detail's
   * Quota row does. The range shows as "Custom" until the reader picks a
   * preset of their own. Never persisted: a custom range is only ever the
   * one a caller hands in, not a reader's own choice to restore.
   */
  open = (selection: QuotaSelection, range: QuotaRange): void => {
    this.update({
      selection,
      range: { kind: "custom", startEpoch: range.startEpoch, endEpoch: range.endEpoch },
    })
    traceEvent("quota.refresh", { trigger: "open" })
    this.loadUsage()
  }

  /** Re-read accounts and usage. Exposed for a Retry button. An explicit
   *  refresh always bypasses the usage cache. */
  refresh = (): void => {
    traceEvent("quota.refresh", { trigger: "refresh" })
    this.usageCache.clear()
    this.loadAccounts()
    this.loadUsage()
  }

  private loadAccounts(): void {
    this.accountsDirty = true
    if (!this.active || this.accountsTask) return
    this.accountsTask = this.runAccountsLoop().finally(() => {
      this.accountsTask = null
      if (this.accountsDirty && this.active) {
        traceEvent("quota.refresh", { trigger: "accountsLoopContinue" })
        this.loadAccounts()
      }
    })
  }

  private async runAccountsLoop(): Promise<void> {
    while (this.accountsDirty && this.active) {
      this.accountsDirty = false
      await this.runLoadAccounts(this.workVersion, ++this.accountsVersion)
    }
  }

  private async runLoadAccounts(work: number, version: number): Promise<void> {
    if (!this.snapshot.accounts) this.update({ loading: true })
    const previousSelection = this.snapshot.selection
    try {
      const payload = await this.adapter.getAccounts()
      if (work !== this.workVersion || version !== this.accountsVersion) return
      const selection = this.resolveSelection(payload.accounts)
      const unchanged = selection != null && selectionEquals(selection, previousSelection)
      this.update({
        accounts: payload.accounts,
        accountsError: false,
        selection,
        // An unchanged selection leaves `loading` to whatever usage load is
        // already running for it (started here, or already in flight from
        // elsewhere), so this never resurrects "loading" after that load
        // already finished.
        ...(unchanged ? {} : { loading: selection != null }),
      })
      // Load usage only when the selection actually changed. A reload that
      // confirms the same account and lane leaves any in-flight or already
      // coalesced usage refresh to run on its own trigger.
      if (selection && !unchanged) {
        traceEvent("quota.refresh", { trigger: "accountsResolved" })
        this.loadUsage()
      } else if (!selection) this.update({ loading: false })
    } catch {
      if (work === this.workVersion && version === this.accountsVersion) {
        this.update({ accountsError: true, loading: false })
      }
    }
  }

  private loadUsage(): void {
    this.usageDirty = true
    if (!this.active) return
    if (this.usageTask) {
      // A load is already running: the dirty flag alone carries this
      // request into the loop's next pass once the running load finishes.
      traceEvent("quota.loadUsage.skipped", {})
      return
    }
    this.usageTask = this.runUsageLoop().finally(() => {
      this.usageTask = null
      if (this.usageDirty && this.active) {
        traceEvent("quota.refresh", { trigger: "usageLoopContinue" })
        this.loadUsage()
      }
    })
  }

  private async runUsageLoop(): Promise<void> {
    while (this.usageDirty && this.active) {
      this.usageDirty = false
      const selection = this.snapshot.selection
      if (!selection) continue
      await this.runLoadUsage(
        this.workVersion,
        ++this.usageVersion,
        selection,
        this.snapshot.range,
      )
    }
  }

  private async runLoadUsage(
    work: number,
    version: number,
    selection: QuotaSelection,
    range: QuotaRangeSelection,
  ): Promise<void> {
    // `now` doubles as the cache clock: it is the adapter's own epoch-second
    // reading, the only clock a test can drive, and the same value this load
    // would stamp on a fresh result.
    const now = this.adapter.now()
    const key = quotaUsageCacheKey(selection, range)
    const cached = this.usageCache.get(key)
    if (cached && now * 1000 - cached.loadedAt < QUOTA_USAGE_CACHE_TTL_MS) {
      this.update({ usage: cached.usage, usageError: false, loading: false, now: cached.now })
      traceEvent("quota.loadUsage.cached", { version })
      return
    }
    traceEvent("quota.loadUsage.start", { version })
    // Every usage load, including a reload beside existing usage, shows as
    // loading, so a selection change gives visible feedback for the round
    // trip. The run/version guards below still stop a stale response from
    // clearing a newer load's flag.
    this.update({ loading: true })
    const account = findAccount(
      this.snapshot.accounts ?? [],
      selection.provider,
      selection.accountKey,
    )
    const lane = findLane(account, selection.lane)
    const { startEpoch, endEpoch } = resolveQuotaRange(range, lane, now, weeklyLaneOf(account))
    try {
      const usage = await this.adapter.getUsage({
        provider: selection.provider,
        accountKey: selection.accountKey,
        lane: selection.lane,
        rangeStartEpoch: startEpoch,
        rangeEndEpoch: endEpoch,
      })
      if (work !== this.workVersion || version !== this.usageVersion) {
        traceEvent("quota.loadUsage.done", { version, stale: true })
        return
      }
      this.usageCache.set(key, { usage, now, loadedAt: now * 1000 })
      this.update({ usage, usageError: false, loading: false, now })
      traceEvent("quota.loadUsage.done", { version, stale: false })
    } catch {
      const stale = work !== this.workVersion || version !== this.usageVersion
      if (!stale) {
        this.update({ usageError: true, loading: false, now })
      }
      traceEvent("quota.loadUsage.done", { version, stale, error: true })
    }
  }

  private syncExposure(): void {
    if (!this.active) {
      this.exposure.conceal("quota")
      return
    }
    const selection = this.snapshot.selection
    const generation = this.exposure.expose({
      surface: "quota",
      origin: "user",
      ...(selection
        ? {
            identity: `${selection.provider}:${selection.accountKey}:${selection.lane}:${this.snapshot.range}`,
          }
        : {}),
    })
    const state = this.quotaState()
    if (state) this.exposure.observe(state, generation)
  }

  private quotaState(): "ready" | "empty" | "error" | null {
    if (this.snapshot.accountsError || this.snapshot.usageError) return "error"
    if (this.snapshot.accounts != null && this.snapshot.accounts.length === 0) return "empty"
    if (this.snapshot.usage != null)
      return this.snapshot.usage.periods.length > 0 ? "ready" : "empty"
    return null
  }

  dispose = (): void => {
    this.generation += 1
    this.workVersion += 1
    this.initialized = false
    this.visible = false
    this.active = false
    this.accountsTask = null
    this.usageTask = null
    this.usageCache.clear()
    this.exposure.conceal("quota")
    for (const stop of this.stops.splice(0)) stop()
  }
}
