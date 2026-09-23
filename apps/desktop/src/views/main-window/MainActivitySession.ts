import type { SessionListEntry } from "../../components/session/SessionList"
import { activityDayAge } from "../../components/activity/activityWindow"
import { groupActivityByDay } from "../../components/activity/activityFeedGrouping"
import { toActivityEntries, toActivityEntry } from "../../lib/activityEntries"
import {
  DEFAULT_SETTINGS,
  EMPTY_LIVE_USAGE,
  EMPTY_SESSION_LIMIT_ALLOCATIONS,
  getSettings,
  setSettings,
  listRecentSessions,
  getLiveUsage,
  getSessionLimitAllocations,
  getSessionQuota,
  getMainWindowVisible,
  noteInteraction,
  onMainWindowVisibilityChanged,
  onSettingsChanged,
  onSessionIndexChanged,
  onSessionUpdated,
  onLiveUsageChanged,
  type AppSettings,
  type SessionAnalysisPayload,
  type LiveUsageSummaryPayload,
  type SessionLimitAllocationSummaryPayload,
  type SessionQuotaPayload,
  type SessionUpdatedPayload,
  type SurfaceOrigin,
  type SessionFilterAction,
} from "../../lib/ipc"
import { localSessionKey } from "../../lib/presentation/localIdentity"
import { listInterests, liveSessions, withRegistryActivity } from "../../lib/sessionLifecycle"
import { costOutlierThreshold } from "../../lib/presentation/sessionAnalysis"
import { AGENT_SLUGS } from "../../lib/presentation/agents"
import {
  normalizeSessionFilters,
  parseSessionFilters,
  serializeSessionFilters,
  type SessionFilters,
  type SessionResultFilter,
  type SessionSpendFilter,
} from "../../lib/sessionFilters"
import { sessionKey, loadSessionAnalysis, type SessionSubject } from "../../lib/sessionSubject"
import { SurfaceExposureTracker } from "../../lib/surfaceExposure"

export interface MainActivitySnapshot {
  active: boolean
  /** The collection applies filters to this full list at render time. */
  entries: SessionListEntry[] | null
  listError: boolean
  settings: AppSettings
  settingsError: boolean
  subject: SessionSubject | null
  history: SessionSubject[]
  analysis: { key: string; payload: SessionAnalysisPayload | null; error: boolean } | null
  loading: boolean
  refreshing: boolean
  now: number
  liveUsage: LiveUsageSummaryPayload
  allocations: SessionLimitAllocationSummaryPayload
  /** The open subject's quota contributions, loaded alongside its analysis. */
  sessionQuota: SessionQuotaPayload | null
  filters: SessionFilters
  /** A newer shell target must reveal the compact detail pane, even for the same session. */
  detailRevealRevision: number
}

export function subjectForEntry(entry: SessionListEntry): SessionSubject {
  return {
    agent: entry.agent,
    sessionId: entry.sessionId ?? "",
    repo: entry.repo,
    timestamp: entry.timestamp,
    wslDistro: entry.wslDistro ?? null,
    title: entry.title,
  }
}

export function orderedActivityEntries(snapshot: MainActivitySnapshot): SessionListEntry[] {
  return groupActivityByDay(
    (snapshot.entries ?? []).map((entry) => ({
      entry,
      key: localSessionKey(entry.agent, entry.sessionId ?? "", entry.wslDistro),
      at: entry.timestamp,
      isActive: entry.isActive,
    })),
    { days: snapshot.settings.activityWindowDays, now: new Date(snapshot.now) },
  ).flatMap((group) => group.items.map((item) => item.entry))
}

function analysisSurfaceState(
  subject: SessionSubject,
  analysis: MainActivitySnapshot["analysis"],
): "ready" | "empty" | "error" | null {
  if (!analysis || analysis.key !== sessionKey(subject)) return null
  if (analysis.error) return "error"
  const payload = analysis.payload
  if (!payload) return "empty"
  if (payload.analysisPending) return null
  const hasData =
    (payload.summary?.sessions.length ?? 0) > 0 ||
    payload.cost !== null ||
    payload.topLevelCost !== null ||
    payload.efficiency !== null ||
    (payload.orchestration?.members.length ?? 0) > 0 ||
    (payload.relations?.parent ?? null) !== null ||
    (payload.relations?.children.length ?? 0) > 0 ||
    payload.models.length > 0
  return hasData ? "ready" : "empty"
}

/** Own main-window requests without changing the popover's session state. */
export class MainActivitySession {
  onNavigation?: (origin: SurfaceOrigin) => void
  onDeleted?: (subject: SessionSubject) => void
  onSessionInventoryInvalidated?: () => void
  private restoringNavigation = false

  restoreNavigation(
    filters: SessionFilters,
    subject: SessionSubject | null,
    origin: SurfaceOrigin = "automatic",
    reportFilterSelection = false,
  ): void {
    this.restoringNavigation = true
    try {
      const previous = this.snapshot.filters
      const next = normalizeSessionFilters(filters)
      const added = next.agents.filter((agent) => !previous.agents.includes(agent))
      const removed = previous.agents.filter((agent) => !next.agents.includes(agent))
      let action: SessionFilterAction | undefined
      let agent: string | undefined
      if (reportFilterSelection) {
        if (next.agents.length === 0 && next.result === "all" && next.spend === "all") {
          action = "cleared_all"
        } else if (added.length > 0) {
          action = "agent_added"
          agent = added.length === 1 ? added[0] : undefined
        } else if (removed.length > 0) {
          action = next.agents.length === 0 ? "agents_all" : "agent_removed"
          agent = action === "agent_removed" && removed.length === 1 ? removed[0] : undefined
        } else if (next.result !== previous.result) {
          action = `result_${next.result}`
        } else if (next.spend !== previous.spend) {
          action = `spend_${next.spend}`
        }
      }
      this.changeFilters(next, action, agent)
      if (
        subject &&
        (!this.snapshot.subject || sessionKey(subject) !== sessionKey(this.snapshot.subject))
      )
        this.open(subject, [], origin)
      else if (!subject && this.snapshot.subject) this.clearSelection()
    } finally {
      this.restoringNavigation = false
    }
  }

  private snapshot: MainActivitySnapshot = {
    active: false,
    entries: null,
    listError: false,
    settings: DEFAULT_SETTINGS,
    settingsError: false,
    subject: null,
    history: [],
    analysis: null,
    loading: false,
    refreshing: false,
    now: Date.now(),
    liveUsage: EMPTY_LIVE_USAGE,
    allocations: EMPTY_SESSION_LIMIT_ALLOCATIONS,
    sessionQuota: null,
    filters: parseSessionFilters(DEFAULT_SETTINGS.sessionFilter),
    detailRevealRevision: 0,
  }
  private listeners = new Set<() => void>()
  private activeListeners = new Set<() => void>()
  private listListeners = new Set<() => void>()
  private settingsWrite: Promise<void> | null = null
  private stops: (() => void)[] = []
  private generation = 0
  private workVersion = 0
  private analysisVersion = 0
  private analysisRun = 0
  private listVersion = 0
  private listWorkVersion = 0
  private settingsVersion = 0
  private visible = false
  private initialized = false
  private listRunning = false
  private defaultSelectionPending = true
  private timer: ReturnType<typeof setInterval> | null = null
  private listTask: Promise<void> | null = null
  private listDirty = false
  private invalidated = false
  private analysisTask: Promise<void> | null = null
  private analysisDirty = false
  private sessionQuotaVersion = 0
  private sessionQuotaRun = 0
  private sessionQuotaTask: Promise<void> | null = null
  private sessionQuotaDirty = false
  private usageTask: Promise<void> | null = null
  private usageDirty = false
  private usageRevision = 0
  private readonly exposure = new SurfaceExposureTracker()
  private exposureOrigin: SurfaceOrigin = "automatic"

  getSnapshot = (): MainActivitySnapshot => this.snapshot
  subscribe = (listener: () => void): (() => void) => this.attach(listener, true)
  /** Subscribe to the shared session list without starting session detail work. */
  subscribeList = (listener: () => void): (() => void) => this.attach(listener, false, true)
  subscribeInactive = (listener: () => void): (() => void) => this.attach(listener, false)

  private update(patch: Partial<MainActivitySnapshot>): void {
    this.snapshot = { ...this.snapshot, ...patch }
    this.syncExposure()
    for (const listener of this.listeners) listener()
  }

  private syncExposure(): void {
    const subject = this.snapshot.subject
    if (!this.snapshot.active || !subject) {
      this.exposure.conceal("session_detail")
      return
    }
    const generation = this.exposure.expose({
      surface: "session_detail",
      origin: this.exposureOrigin,
      identity: sessionKey(subject),
    })
    const state = analysisSurfaceState(subject, this.snapshot.analysis)
    if (state) this.exposure.observe(state, generation)
  }

  private attach(listener: () => void, active: boolean, list = false): () => void {
    this.listeners.add(listener)
    if (active) this.activeListeners.add(listener)
    if (list) this.listListeners.add(listener)
    if (this.listeners.size === 1) void this.start()
    this.syncActive()
    return () => {
      this.listeners.delete(listener)
      this.activeListeners.delete(listener)
      this.listListeners.delete(listener)
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
    await Promise.all([
      this.listen(
        generation,
        onMainWindowVisibilityChanged((visible) => {
          if (generation !== this.generation) return
          visibilityRevision += 1
          this.visible = visible
          this.syncActive()
        }),
      ),
      this.listen(
        generation,
        onSettingsChanged((settings) => {
          if (generation !== this.generation) return
          this.settingsVersion += 1
          this.applySettings(settings)
        }),
      ),
      this.listen(
        generation,
        onSessionIndexChanged((change) => {
          if (generation !== this.generation) return
          if (change.cause !== "scan_pass") {
            this.invalidated = true
            this.onSessionInventoryInvalidated?.()
          }
          if (this.listRunning) this.refreshList()
          if (this.snapshot.active) {
            this.refreshUsage()
            if (change.cause !== "scan_pass") this.refreshAnalysis()
          }
        }),
      ),
      this.listen(
        generation,
        onLiveUsageChanged((liveUsage) => {
          if (generation !== this.generation) return
          this.usageRevision += 1
          if (this.snapshot.active) this.update({ liveUsage })
          this.refreshUsage()
          this.refreshSessionQuota()
        }),
      ),
      this.listen(
        generation,
        onSessionUpdated((update) => {
          if (generation !== this.generation || !this.listRunning) return
          this.applySessionUpdate(update)
        }),
      ),
    ])
    if (generation === this.generation) {
      // The registry, not row data, decides which rows show as active.
      this.stops.push(
        liveSessions.subscribe(() => {
          if (generation !== this.generation || !this.listRunning) return
          const entries = this.snapshot.entries
          if (entries) this.update({ entries: this.withRegistryActivity(entries) })
        }),
      )
    }
    if (generation !== this.generation) return
    const settingsVersion = this.settingsVersion
    const revision = visibilityRevision
    const [settings, visible] = await Promise.all([
      getSettings().catch(() => DEFAULT_SETTINGS),
      getMainWindowVisible().catch(() => false),
    ])
    if (generation !== this.generation) return
    if (settingsVersion === this.settingsVersion) this.applySettings(settings)
    if (revision === visibilityRevision) this.visible = visible
    this.initialized = true
    this.syncActive()
  }

  private applySessionUpdate(update: SessionUpdatedPayload): void {
    const entry = update.entry
    this.listVersion += 1
    const entries = this.snapshot.entries
    const key = localSessionKey(entry.agent, entry.sessionId, entry.wslDistro)
    if (
      entries?.some(
        (item) => localSessionKey(item.agent, item.sessionId ?? "", item.wslDistro) === key,
      )
    ) {
      const replaced = entries.map((item) =>
        localSessionKey(item.agent, item.sessionId ?? "", item.wslDistro) === key
          ? toActivityEntry(entry)
          : item,
      )
      const threshold = costOutlierThreshold(
        replaced.flatMap((item) => (item.cost ? [item.cost.totalUsd] : [])),
      )
      const classified = replaced.map((item) => {
        if (!item.cost) return item
        const isHighCost = threshold != null && item.cost.totalUsd > threshold
        return item.cost.isHighCost === isHighCost
          ? item
          : { ...item, cost: { ...item.cost, isHighCost } }
      })
      this.update({ entries: this.withRegistryActivity(classified) })
      if (this.snapshot.active) this.selectDefaultEntry()
    } else if (this.listRunning) this.refreshList()
    const subject = this.snapshot.subject
    // The analysis surface reloads only when the change touched what it
    // renders, and only for the session on screen.
    if (
      (update.facets.analysis || update.facets.checks || update.facets.metadata) &&
      subject &&
      localSessionKey(
        subject.agent,
        subject.subagent?.parentSessionId ?? subject.sessionId,
        subject.wslDistro,
      ) === key
    ) {
      // Session quota loads alongside analysis: same subject match, same
      // trigger, per loadSessionQuota's contract.
      this.refreshAnalysis()
      this.refreshSessionQuota()
    }
    if (
      this.snapshot.active &&
      (update.facets.usage || update.facets.limits || update.facets.analysis)
    ) {
      this.refreshUsage()
    }
  }

  /** Active pills come from the lifecycle registry, never from row timestamps. */
  private withRegistryActivity(entries: SessionListEntry[]): SessionListEntry[] {
    return withRegistryActivity(liveSessions.getSnapshot(), entries)
  }

  private applySettings(settings: AppSettings): void {
    const previous = this.snapshot.settings
    const next = this.settingsWrite
      ? { ...settings, sessionFilter: previous.sessionFilter }
      : settings
    const filters = this.settingsWrite
      ? this.snapshot.filters
      : parseSessionFilters(next.sessionFilter)
    const filterChanged =
      serializeSessionFilters(filters) !== serializeSessionFilters(this.snapshot.filters)
    this.update({
      settings: next,
      settingsError: false,
      filters,
    })
    if (filterChanged && !this.restoringNavigation) this.onNavigation?.("automatic")
    if (
      settings.activityWindowDays !== previous.activityWindowDays ||
      settings.disabledAgents.join() !== previous.disabledAgents.join()
    )
      this.refreshList()
  }

  private syncActive(): void {
    const listActive =
      this.initialized &&
      this.visible &&
      (this.activeListeners.size > 0 || this.listListeners.size > 0)
    const detailActive = this.initialized && this.visible && this.activeListeners.size > 0
    const listChanged = listActive !== this.listRunning
    const detailChanged = detailActive !== this.snapshot.active
    if (!listChanged && !detailChanged) return
    this.workVersion += 1
    if (listChanged) this.listWorkVersion += 1
    this.listRunning = listActive
    if (detailChanged) this.update({ active: detailActive, now: Date.now(), refreshing: false })
    if (this.timer) clearInterval(this.timer)
    this.timer = null
    if (!listActive) {
      liveSessions.clearInterest(this)
    }
    if (!detailActive) {
      this.analysisRun += 1
      this.analysisTask = null
      this.sessionQuotaRun += 1
      this.sessionQuotaTask = null
    }
    if (listActive) {
      const rows = this.snapshot.entries
      if (rows) {
        this.update({ entries: this.withRegistryActivity(rows) })
        liveSessions.setInterest(this, listInterests(rows))
      }
      if (listChanged) this.refreshList()
    }
    if (detailActive) {
      this.timer = setInterval(() => this.update({ now: Date.now() }), 30_000)
      if (detailChanged) {
        const hadSubject = this.snapshot.subject !== null
        if (this.snapshot.entries && !hadSubject) this.selectDefaultEntry()
        this.refreshUsage()
        if (hadSubject || this.snapshot.subject === null) {
          this.refreshAnalysis()
          this.refreshSessionQuota()
        }
      }
    }
  }

  dispose = (): void => {
    this.generation += 1
    this.workVersion += 1
    this.listWorkVersion += 1
    this.analysisVersion += 1
    this.analysisRun += 1
    this.analysisTask = null
    this.sessionQuotaVersion += 1
    this.sessionQuotaRun += 1
    this.sessionQuotaTask = null
    this.initialized = false
    this.visible = false
    this.listRunning = false
    for (const stop of this.stops.splice(0)) stop()
    liveSessions.clearInterest(this)
    if (this.timer) clearInterval(this.timer)
    this.timer = null
    this.update({ active: false, refreshing: false })
  }

  refreshList = (): void => {
    this.listVersion += 1
    this.listDirty = true
    if (!this.listRunning || this.listTask) return
    this.listTask = this.loadList().finally(() => {
      this.listTask = null
      if (this.listDirty && this.listRunning) this.refreshList()
    })
  }

  private async loadList(): Promise<void> {
    while (this.listDirty && this.listRunning) {
      this.listDirty = false
      const version = this.listWorkVersion
      const listVersion = this.listVersion
      const days = this.snapshot.settings.activityWindowDays
      const invalidated = this.invalidated
      this.invalidated = false
      try {
        const entries = this.withRegistryActivity(
          toActivityEntries(await listRecentSessions(days)),
        )
        if (version !== this.listWorkVersion || listVersion !== this.listVersion) {
          this.invalidated ||= invalidated
          continue
        }
        this.update({ entries, listError: false, now: Date.now() })
        // The listed rows are this surface's interest: the registry names
        // any of them the bounded snapshot omitted.
        liveSessions.setInterest(this, listInterests(entries))
        if (this.snapshot.active) this.selectDefaultEntry()
      } catch {
        if (version === this.listWorkVersion && listVersion === this.listVersion)
          this.update({ listError: true })
        this.invalidated ||= invalidated
      }
    }
  }

  private selectDefaultEntry(): void {
    if (!this.defaultSelectionPending || this.snapshot.subject) return
    const now = new Date(this.snapshot.now)
    const entry = orderedActivityEntries(this.snapshot).find(
      (item) => item.sessionId && (item.isActive || activityDayAge(item.timestamp, now) === 0),
    )
    if (entry) this.open(subjectForEntry(entry), [], "automatic")
  }

  selectEntry = (entry: SessionListEntry): void => {
    if (!entry.sessionId) return
    const subject = subjectForEntry(entry)
    if (this.snapshot.subject && sessionKey(this.snapshot.subject) === sessionKey(subject))
      return
    this.open(subject, [], "user")
  }

  revealDetail(): void {
    if (this.snapshot.subject) {
      this.update({ detailRevealRevision: this.snapshot.detailRevealRevision + 1 })
    }
  }

  openRelated = (subject: SessionSubject): void => {
    const current = this.snapshot.subject
    if (current && sessionKey(current) === sessionKey(subject)) return
    this.open(subject, current ? [...this.snapshot.history, current] : [], "user")
  }

  goBack = (): void => {
    const previous = this.snapshot.history.at(-1)
    if (previous) this.open(previous, this.snapshot.history.slice(0, -1), "user")
  }

  private open(
    subject: SessionSubject,
    history: SessionSubject[],
    origin: SurfaceOrigin,
  ): void {
    this.exposureOrigin = origin
    this.defaultSelectionPending = false
    this.analysisVersion += 1
    this.analysisRun += 1
    this.analysisTask = null
    this.sessionQuotaVersion += 1
    this.sessionQuotaRun += 1
    this.sessionQuotaTask = null
    this.update({
      subject,
      history,
      analysis: null,
      loading: true,
      refreshing: false,
      sessionQuota: null,
    })
    this.refreshAnalysis()
    this.refreshSessionQuota()
    if (!this.restoringNavigation) this.onNavigation?.(origin)
  }

  clearSelection = (): void => {
    this.defaultSelectionPending = false
    this.analysisVersion += 1
    this.analysisRun += 1
    this.analysisTask = null
    this.sessionQuotaVersion += 1
    this.sessionQuotaRun += 1
    this.sessionQuotaTask = null
    this.update({
      subject: null,
      history: [],
      analysis: null,
      loading: false,
      refreshing: false,
      sessionQuota: null,
    })
  }

  private removeSubject(subject: SessionSubject): void {
    this.clearSelection()
    this.onDeleted?.(subject)
  }

  removeDeletedSubject = (subject: SessionSubject): void => {
    const current = this.snapshot.subject
    if (!current || sessionKey(current) !== sessionKey(subject)) return
    this.removeSubject(current)
  }

  deleted = (): void => {
    const subject = this.snapshot.subject
    if (subject) this.removeSubject(subject)
    this.invalidated = true
    this.refreshList()
    this.refreshUsage()
  }

  refreshAnalysis = (): void => {
    this.analysisVersion += 1
    this.analysisDirty = true
    if (!this.snapshot.active || !this.snapshot.subject || this.analysisTask) return
    const run = ++this.analysisRun
    this.analysisTask = this.loadAnalysis(run).finally(() => {
      if (run !== this.analysisRun) return
      this.analysisTask = null
      if (this.analysisDirty && this.snapshot.active && this.snapshot.subject)
        this.refreshAnalysis()
    })
  }

  private async loadAnalysis(run: number): Promise<void> {
    while (
      run === this.analysisRun &&
      this.analysisDirty &&
      this.snapshot.active &&
      this.snapshot.subject
    ) {
      this.analysisDirty = false
      const subject = this.snapshot.subject
      const key = sessionKey(subject)
      const version = this.analysisVersion
      const work = this.workVersion
      this.update({ loading: !this.snapshot.analysis, refreshing: !!this.snapshot.analysis })
      try {
        const payload = await loadSessionAnalysis(subject)
        if (version !== this.analysisVersion || work !== this.workVersion) continue
        this.update({
          analysis: { key, payload, error: false },
          loading: false,
          refreshing: false,
        })
      } catch {
        if (version !== this.analysisVersion || work !== this.workVersion) continue
        this.update({
          analysis: { key, payload: this.snapshot.analysis?.payload ?? null, error: true },
          loading: false,
          refreshing: false,
        })
      }
    }
  }

  refreshSessionQuota = (): void => {
    this.sessionQuotaVersion += 1
    this.sessionQuotaDirty = true
    if (!this.snapshot.active || !this.snapshot.subject || this.sessionQuotaTask) return
    const run = ++this.sessionQuotaRun
    this.sessionQuotaTask = this.loadSessionQuota(run).finally(() => {
      if (run !== this.sessionQuotaRun) return
      this.sessionQuotaTask = null
      if (this.sessionQuotaDirty && this.snapshot.active && this.snapshot.subject)
        this.refreshSessionQuota()
    })
  }

  /**
   * One subject's quota contributions, loaded alongside its analysis. A
   * failure keeps the last good value, so it never blanks the rest of the
   * detail view.
   */
  private async loadSessionQuota(run: number): Promise<void> {
    while (
      run === this.sessionQuotaRun &&
      this.sessionQuotaDirty &&
      this.snapshot.active &&
      this.snapshot.subject
    ) {
      this.sessionQuotaDirty = false
      const subject = this.snapshot.subject
      const version = this.sessionQuotaVersion
      const work = this.workVersion
      try {
        const sessionQuota = await getSessionQuota({
          agent: subject.agent,
          sessionId: subject.subagent?.parentSessionId ?? subject.sessionId,
          wslDistro: subject.wslDistro ?? null,
        })
        if (version !== this.sessionQuotaVersion || work !== this.workVersion) continue
        this.update({ sessionQuota })
      } catch {
        // Keep the last good payload; a failed load is not shown.
      }
    }
  }

  private refreshUsage(): void {
    this.usageDirty = true
    if (!this.snapshot.active || this.usageTask) return
    this.usageTask = this.loadUsage().finally(() => {
      this.usageTask = null
      if (this.usageDirty && this.snapshot.active) this.refreshUsage()
    })
  }

  private async loadUsage(): Promise<void> {
    while (this.usageDirty && this.snapshot.active) {
      this.usageDirty = false
      const version = this.workVersion
      const revision = this.usageRevision
      const [liveUsage, allocations] = await Promise.all([
        getLiveUsage().catch(() => null),
        getSessionLimitAllocations().catch(() => null),
      ])
      if (version !== this.workVersion) continue
      this.update({
        ...(liveUsage && revision === this.usageRevision ? { liveUsage } : {}),
        ...(allocations ? { allocations } : {}),
      })
    }
  }

  setBadgeMetric = (metric: AppSettings["sessionBadgeMetric"]): Promise<void> => {
    this.settingsVersion += 1
    return this.persistSettings({ sessionBadgeMetric: metric })
  }

  private persistSettings(patch: Partial<AppSettings>): Promise<void> {
    // Save in gesture order and read current settings before each write.
    const write: Promise<void> = (this.settingsWrite ?? Promise.resolve())
      .then(async () => setSettings({ ...(await getSettings()), ...patch }))
      .then((saved) => {
        if (this.settingsWrite !== write) return
        this.settingsWrite = null
        try {
          this.applySettings(saved)
        } catch {
          if (this.settingsWrite === null) this.update({ settingsError: true })
        }
      })
      .catch(() => {
        if (this.settingsWrite !== write) return
        this.settingsWrite = null
        this.update({ settingsError: true })
      })
    this.settingsWrite = write
    return write
  }

  toggleAgent = (agent: string): void => {
    if (!agent.trim()) return
    const filters = this.snapshot.filters
    const selected = filters.agents.includes(agent)
    this.changeFilters(
      {
        ...filters,
        agents: selected
          ? filters.agents.filter((value) => value !== agent)
          : [...filters.agents, agent],
      },
      selected ? "agent_removed" : "agent_added",
      agent,
    )
  }

  resetAgents = (): void => {
    this.changeFilters({ ...this.snapshot.filters, agents: [] }, "agents_all")
  }

  setResultFilter = (result: SessionResultFilter): void => {
    this.changeFilters({ ...this.snapshot.filters, result }, `result_${result}`)
  }

  setSpendFilter = (spend: SessionSpendFilter): void => {
    this.changeFilters({ ...this.snapshot.filters, spend }, `spend_${spend}`)
  }

  clearFilters = (): void => {
    this.changeFilters(parseSessionFilters("all"), "cleared_all")
  }

  private changeFilters(
    filters: SessionFilters,
    action?: SessionFilterAction,
    agent?: string,
  ): void {
    const normalized = normalizeSessionFilters(filters)
    const id = serializeSessionFilters(normalized)
    if (serializeSessionFilters(this.snapshot.filters) === id) return
    this.settingsVersion += 1
    this.update({
      settings: { ...this.snapshot.settings, sessionFilter: id },
      filters: normalized,
    })
    void this.persistSettings({ sessionFilter: id })
    if (!this.restoringNavigation) this.onNavigation?.("user")
    if (action) {
      noteInteraction({
        kind: "sessionFiltersChanged",
        action,
        ...(agent && AGENT_SLUGS.includes(agent) ? { agent } : {}),
      })
    }
  }
}
