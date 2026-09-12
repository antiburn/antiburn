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
  getMainWindowVisible,
  acknowledgeMainWindowSessionTarget,
  noteInteraction,
  onMainWindowSessionTarget,
  onMainWindowVisibilityChanged,
  onSettingsChanged,
  onSessionEntryChanged,
  onSessionsInvalidated,
  onScanEvent,
  onLiveUsageChanged,
  peekMainWindowSessionTarget,
  type AppSettings,
  type SessionAnalysisPayload,
  type LiveUsageSummaryPayload,
  type SessionLimitAllocationSummaryPayload,
  type MainWindowSessionRequest,
  type SurfaceOrigin,
} from "../../lib/ipc"
import { localSessionKey } from "../../lib/presentation/localIdentity"
import { costOutlierThreshold } from "../../lib/presentation/sessionAnalysis"
import { AGENT_SLUGS } from "../../lib/presentation/agents"
import {
  parseSessionFilterId,
  sessionFilterId,
  type SessionFilter,
} from "../../lib/sessionFilters"
import { sessionKey, loadSessionAnalysis, type SessionSubject } from "../../lib/sessionSubject"
import { SurfaceExposureTracker } from "../../lib/surfaceExposure"

export interface MainActivitySnapshot {
  active: boolean
  /** The full, unfiltered list. The sidebar's selected filter applies at render time. */
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
  /** The selected Sessions sidebar filter, parsed from `settings.sessionFilter`. */
  filter: SessionFilter
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
    filter: parseSessionFilterId(DEFAULT_SETTINGS.sessionFilter),
  }
  private listeners = new Set<() => void>()
  private activeListeners = new Set<() => void>()
  private stops: (() => void)[] = []
  private generation = 0
  private workVersion = 0
  private analysisVersion = 0
  private analysisRun = 0
  private listVersion = 0
  private settingsVersion = 0
  private visible = false
  private initialized = false
  private defaultSelectionPending = true
  private timer: ReturnType<typeof setInterval> | null = null
  private listTask: Promise<void> | null = null
  private listDirty = false
  private invalidated = false
  private analysisTask: Promise<void> | null = null
  private analysisDirty = false
  private usageTask: Promise<void> | null = null
  private usageDirty = false
  private usageRevision = 0
  private targetRevision = 0
  private readonly exposure = new SurfaceExposureTracker()
  private exposureOrigin: SurfaceOrigin = "automatic"

  getSnapshot = (): MainActivitySnapshot => this.snapshot
  subscribe = (listener: () => void): (() => void) => this.attach(listener, true)
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
        onMainWindowSessionTarget((request) => {
          if (generation !== this.generation) return
          this.applyAndAcknowledgeSessionTarget(request)
          void this.peekSessionTarget(generation)
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
        onSessionsInvalidated(() => {
          if (generation !== this.generation) return
          this.invalidated = true
          this.refreshList()
          this.refreshUsage()
          this.refreshAnalysis()
        }),
      ),
      this.listen(
        generation,
        onScanEvent((_status, phase) => {
          if (generation !== this.generation || phase !== "finished") return
          this.refreshList()
          this.refreshUsage()
        }),
      ),
      this.listen(
        generation,
        onLiveUsageChanged((liveUsage) => {
          if (generation !== this.generation) return
          this.usageRevision += 1
          if (this.snapshot.active) this.update({ liveUsage })
          this.refreshUsage()
        }),
      ),
      this.listen(
        generation,
        onSessionEntryChanged((entry) => {
          if (generation !== this.generation || !this.snapshot.active) return
          this.listVersion += 1
          const entries = this.snapshot.entries
          const key = localSessionKey(entry.agent, entry.sessionId, entry.wslDistro)
          if (
            entries?.some(
              (item) =>
                localSessionKey(item.agent, item.sessionId ?? "", item.wslDistro) === key,
            )
          ) {
            const threshold = costOutlierThreshold(
              entries.flatMap((item) => (item.cost ? [item.cost.totalUsd] : [])),
            )
            this.update({
              entries: entries.map((item) =>
                localSessionKey(item.agent, item.sessionId ?? "", item.wslDistro) === key
                  ? toActivityEntry(entry, threshold)
                  : item,
              ),
            })
            this.selectDefaultEntry()
          } else this.refreshList()
          const subject = this.snapshot.subject
          if (
            subject &&
            localSessionKey(
              subject.agent,
              subject.subagent?.parentSessionId ?? subject.sessionId,
              subject.wslDistro,
            ) === key
          )
            this.refreshAnalysis()
          this.refreshUsage()
        }),
      ),
    ])
    if (generation !== this.generation) return
    const settingsVersion = this.settingsVersion
    const revision = visibilityRevision
    const rendererGeneration = this.rendererGeneration()
    const [settings, visible, target] = await Promise.all([
      getSettings().catch(() => DEFAULT_SETTINGS),
      getMainWindowVisible().catch(() => false),
      rendererGeneration === null
        ? Promise.resolve(null)
        : peekMainWindowSessionTarget(rendererGeneration).catch(() => null),
    ])
    if (generation !== this.generation) return
    if (target) this.applyAndAcknowledgeSessionTarget(target)
    if (settingsVersion === this.settingsVersion) this.applySettings(settings)
    if (revision === visibilityRevision) this.visible = visible
    this.initialized = true
    this.syncActive()
  }

  private rendererGeneration(): number | null {
    const generation = window.__ANTIBURN_WINDOW_GENERATION__
    return typeof generation === "number" && Number.isSafeInteger(generation)
      ? generation
      : null
  }

  private applySessionTarget(request: MainWindowSessionRequest): void {
    if (request.revision <= this.targetRevision) return
    this.targetRevision = request.revision
    this.open(request.target, [], "user")
  }

  private applyAndAcknowledgeSessionTarget(request: MainWindowSessionRequest): void {
    if (request.revision < this.targetRevision) return
    this.applySessionTarget(request)
    const generation = this.rendererGeneration()
    if (generation === null) return
    void acknowledgeMainWindowSessionTarget(generation, request.revision).catch(() => {
      console.error("The main window could not acknowledge its session target.")
    })
  }

  private async peekSessionTarget(generation: number): Promise<void> {
    const rendererGeneration = this.rendererGeneration()
    if (rendererGeneration === null) return
    const request = await peekMainWindowSessionTarget(rendererGeneration).catch(() => null)
    if (generation === this.generation && request) {
      this.applyAndAcknowledgeSessionTarget(request)
    }
  }

  private applySettings(settings: AppSettings): void {
    const previous = this.snapshot.settings
    this.update({
      settings,
      settingsError: false,
      filter: parseSessionFilterId(settings.sessionFilter),
    })
    if (
      settings.activityWindowDays !== previous.activityWindowDays ||
      settings.disabledAgents.join() !== previous.disabledAgents.join()
    )
      this.refreshList()
  }

  private syncActive(): void {
    const active = this.initialized && this.visible && this.activeListeners.size > 0
    if (active === this.snapshot.active) return
    this.workVersion += 1
    this.update({ active, now: Date.now(), refreshing: false })
    if (this.timer) clearInterval(this.timer)
    this.timer = null
    if (!active) {
      this.analysisRun += 1
      this.analysisTask = null
      return
    }
    this.timer = setInterval(() => this.update({ now: Date.now() }), 30_000)
    this.refreshList()
    this.refreshUsage()
    this.refreshAnalysis()
  }

  dispose = (): void => {
    this.generation += 1
    this.workVersion += 1
    this.analysisVersion += 1
    this.analysisRun += 1
    this.analysisTask = null
    this.initialized = false
    this.visible = false
    for (const stop of this.stops.splice(0)) stop()
    if (this.timer) clearInterval(this.timer)
    this.timer = null
    this.update({ active: false, refreshing: false })
  }

  refreshList = (): void => {
    this.listVersion += 1
    this.listDirty = true
    if (!this.snapshot.active || this.listTask) return
    this.listTask = this.loadList().finally(() => {
      this.listTask = null
      if (this.listDirty && this.snapshot.active) this.refreshList()
    })
  }

  private async loadList(): Promise<void> {
    while (this.listDirty && this.snapshot.active) {
      this.listDirty = false
      const version = this.workVersion
      const listVersion = this.listVersion
      const days = this.snapshot.settings.activityWindowDays
      const settingsVersion = this.settingsVersion
      const invalidated = this.invalidated
      this.invalidated = false
      try {
        const entries = toActivityEntries(await listRecentSessions(days))
        if (
          version !== this.workVersion ||
          settingsVersion !== this.settingsVersion ||
          listVersion !== this.listVersion
        ) {
          this.invalidated ||= invalidated
          continue
        }
        this.update({ entries, listError: false, now: Date.now() })
        this.selectDefaultEntry()
        const subject = this.snapshot.subject
        if (
          invalidated &&
          subject &&
          !entries.some(
            (entry) =>
              localSessionKey(entry.agent, entry.sessionId ?? "", entry.wslDistro) ===
              localSessionKey(
                subject.agent,
                subject.subagent?.parentSessionId ?? subject.sessionId,
                subject.wslDistro,
              ),
          )
        )
          this.clearSelection()
      } catch {
        if (version === this.workVersion && listVersion === this.listVersion)
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
    this.update({ subject, history, analysis: null, loading: true, refreshing: false })
    this.refreshAnalysis()
  }

  clearSelection = (): void => {
    this.defaultSelectionPending = false
    this.analysisVersion += 1
    this.analysisRun += 1
    this.analysisTask = null
    this.update({
      subject: null,
      history: [],
      analysis: null,
      loading: false,
      refreshing: false,
    })
  }

  deleted = (): void => {
    this.clearSelection()
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

  setBadgeMetric = async (metric: AppSettings["sessionBadgeMetric"]): Promise<void> => {
    try {
      const latest = await getSettings()
      await setSettings({ ...latest, sessionBadgeMetric: metric })
    } catch {
      this.update({ settingsError: true })
    }
  }

  /**
   * Select a Sessions sidebar filter and persist the choice.
   *
   * Optimistic, the same way the popover's badge-metric setter writes: the
   * sidebar selection must not lag behind the click, and the stored answer
   * replaces this one a moment later. A no-op reselection neither writes nor
   * reports, so restoring the persisted filter on load — which calls
   * `applySettings`, not this method — never reports a selection either.
   */
  setFilter = (filter: SessionFilter): void => {
    const current = this.snapshot.settings
    const id = sessionFilterId(filter)
    if (current.sessionFilter === id) return
    const next = { ...current, sessionFilter: id }
    this.update({ settings: next, filter })
    void setSettings(next)
      .then((saved) =>
        this.update({ settings: saved, filter: parseSessionFilterId(saved.sessionFilter) }),
      )
      .catch(() => this.update({ settingsError: true }))
    noteInteraction(
      filter.kind === "agent"
        ? {
            kind: "sessionFilterSelected",
            filter: "agent",
            // Only a slug the shell's closed agent enum recognizes; an
            // unrecognized harness omits the field rather than send one.
            ...(AGENT_SLUGS.includes(filter.agent) ? { agent: filter.agent } : {}),
          }
        : { kind: "sessionFilterSelected", filter: filter.kind },
    )
  }
}
