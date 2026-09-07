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
  onMainWindowVisibilityChanged,
  onSettingsChanged,
  onSessionEntryChanged,
  onSessionsInvalidated,
  onScanEvent,
  onLiveUsageChanged,
  type AppSettings,
  type SessionAnalysisPayload,
  type LiveUsageSummaryPayload,
  type SessionLimitAllocationSummaryPayload,
} from "../../lib/ipc"
import { localSessionKey } from "../../lib/presentation/localIdentity"
import { costOutlierThreshold } from "../../lib/presentation/sessionAnalysis"
import { sessionKey, loadSessionAnalysis, type SessionSubject } from "../../lib/sessionSubject"

export interface MainActivitySnapshot {
  active: boolean
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

  getSnapshot = (): MainActivitySnapshot => this.snapshot
  subscribe = (listener: () => void): (() => void) => this.attach(listener, true)
  subscribeInactive = (listener: () => void): (() => void) => this.attach(listener, false)

  private update(patch: Partial<MainActivitySnapshot>): void {
    this.snapshot = { ...this.snapshot, ...patch }
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

  private applySettings(settings: AppSettings): void {
    const previous = this.snapshot.settings
    this.update({ settings, settingsError: false })
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
    if (entry) this.selectEntry(entry)
  }

  selectEntry = (entry: SessionListEntry): void => {
    if (!entry.sessionId) return
    const subject = subjectForEntry(entry)
    if (this.snapshot.subject && sessionKey(this.snapshot.subject) === sessionKey(subject))
      return
    this.open(subject, [])
  }

  openRelated = (subject: SessionSubject): void => {
    const current = this.snapshot.subject
    if (current && sessionKey(current) === sessionKey(subject)) return
    this.open(subject, current ? [...this.snapshot.history, current] : [])
  }

  goBack = (): void => {
    const previous = this.snapshot.history.at(-1)
    if (previous) this.open(previous, this.snapshot.history.slice(0, -1))
  }

  private open(subject: SessionSubject, history: SessionSubject[]): void {
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
}
