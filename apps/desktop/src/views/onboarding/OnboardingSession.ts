import { open } from "@tauri-apps/plugin-dialog"

import { applyTheme } from "../../lib/appearance"
import {
  addScanRoot,
  appInfo,
  defaultScanRoots,
  finishOnboarding,
  getFolderPermissions,
  getScanStatus,
  getSettings,
  listRepositories,
  listScanRoots,
  noteInteraction,
  onScanEvent,
  recheckFolderPermissions,
  removeScanRoot,
  requestFolderAccess,
  scanNow,
  setRepositoryEnabled,
  type Interaction,
  type RepositoryItemPayload,
  type ScanStatus,
} from "../../lib/ipc"
import { getHygieneSummary, type HygieneSummary } from "../../lib/insightsIpc"
import type {
  FolderPermissions,
  LocalRepositoryItem,
  LocalRepositoryStatus,
} from "../../lib/types/repository"
import type {
  FlowPhase,
  FolderPermissionFlow,
  FolderVerdict,
} from "../../lib/useFolderPermissionFlow"
import { AGENT_SLUGS } from "../../lib/presentation/agents"

const EMPTY_PERMISSIONS: FolderPermissions = {
  deferred: [],
  granted: [],
  supported: false,
}

const BETWEEN_FOLDERS_MS = 800

export type OnboardingStep = Extract<Interaction, { kind: "onboardingStepViewed" }>["step"]

export type OnboardingSnapshot = {
  loadState: "loading" | "ready" | "error"
  loadError: string | null
  activityWindowDays: number
  launchAtLogin: boolean
  /** Whether this build includes an active analytics client. */
  analyticsSupported: boolean
  /** Whether the process environment disables analytics for this launch. */
  analyticsEnvironmentDisabled: boolean
  scanRoots: string[]
  defaultRoots: string[]
  permissions: FolderPermissions
  repositories: LocalRepositoryItem[]
  scanStatus: ScanStatus | null
  /**
   * Draft of the disabled-agent set, persisted by `finish`. Seeded once from
   * scan results: agents with sessions start on, the rest start off.
   */
  disabledAgents: string[]
  /** Draft of the Do Not Disturb opt-in, persisted by `finish`. */
  nudgesRespectDnd: boolean
  /** The aggregate check numbers the Ready step shows. Null until fetched. */
  hygieneSummary: HygieneSummary | null
  recheckingPermissions: boolean
  finishing: boolean
  finishError: string | null
  permissionFlow: FolderPermissionFlow
}

type PermissionState = Pick<
  FolderPermissionFlow,
  "phase" | "current" | "verdicts" | "position" | "total"
>

const INITIAL_PERMISSION_STATE: PermissionState = {
  phase: "idle",
  current: null,
  verdicts: {},
  position: 0,
  total: 0,
}

/**
 * The imperative boundary between the onboarding window and the shell.
 *
 * React reads immutable snapshots through `useSyncExternalStore`; commands,
 * event subscriptions, timers and cancellation stay here, where they belong to
 * the external systems that created them rather than to a component lifecycle.
 */
export class OnboardingSession {
  private listeners = new Set<() => void>()
  private started = false
  private generation = 0
  private scanEventVersion = 0
  private stopScanListening: (() => void) | null = null
  private permissionState: PermissionState = { ...INITIAL_PERMISSION_STATE }
  private permissionQueue: string[] = []
  private permissionRunning = false
  private permissionCancelled = false
  private permissionTimer: ReturnType<typeof setTimeout> | null = null
  private rescanInFlight = false
  private rescanQueued = false
  private agentChoicesTouched = false
  private hygieneTimer: ReturnType<typeof setInterval> | null = null
  private analyticsSteps = new Set<OnboardingStep>()

  private snapshot: OnboardingSnapshot

  constructor() {
    this.snapshot = {
      loadState: "loading",
      loadError: null,
      activityWindowDays: 7,
      launchAtLogin: true,
      analyticsSupported: false,
      analyticsEnvironmentDisabled: false,
      scanRoots: [],
      defaultRoots: [],
      permissions: EMPTY_PERMISSIONS,
      repositories: [],
      scanStatus: null,
      disabledAgents: [],
      nudgesRespectDnd: false,
      hygieneSummary: null,
      recheckingPermissions: false,
      finishing: false,
      finishError: null,
      permissionFlow: this.permissionFlow(),
    }
  }

  getSnapshot = (): OnboardingSnapshot => this.snapshot

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener)
    if (!this.started) void this.start()
    return () => {
      this.listeners.delete(listener)
      if (this.listeners.size === 0) this.stop()
    }
  }

  retry = (): void => {
    if (this.started) this.stop()
    this.update({ loadState: "loading", loadError: null })
    void this.start()
  }

  setLaunchAtLogin = (enabled: boolean): void => {
    this.update({ launchAtLogin: enabled, finishError: null })
  }

  setNudgesRespectDnd = (enabled: boolean): void => {
    this.update({ nudgesRespectDnd: enabled, finishError: null })
  }

  /**
   * Fetch the check numbers now and re-fetch each second until analysis
   * settles. The flow calls this when the reader reaches the Ready step,
   * so the poll never runs behind an earlier step.
   */
  beginHygienePolling = (): void => {
    if (this.hygieneTimer !== null) return
    this.hygieneTimer = setInterval(() => void this.refreshHygieneSummary(), 1_000)
    void this.refreshHygieneSummary()
  }

  private refreshHygieneSummary = async (): Promise<void> => {
    const generation = this.generation
    const summary = await getHygieneSummary().catch(() => null)
    if (summary === null || generation !== this.generation) return
    this.update({ hygieneSummary: summary })
    if (summary.settledSessions >= summary.totalSessions) this.stopHygienePolling()
  }

  private stopHygienePolling(): void {
    if (this.hygieneTimer === null) return
    clearInterval(this.hygieneTimer)
    this.hygieneTimer = null
  }

  setAgentEnabled = (slug: string, enabled: boolean): void => {
    this.agentChoicesTouched = true
    const disabled = new Set(this.snapshot.disabledAgents)
    if (enabled) disabled.delete(slug)
    else disabled.add(slug)
    this.update({ disabledAgents: [...disabled].sort(), finishError: null })
  }

  noteOnboardingStep = (step: OnboardingStep): void => {
    if (
      !this.snapshot.analyticsSupported ||
      this.snapshot.analyticsEnvironmentDisabled ||
      this.analyticsSteps.has(step)
    )
      return
    this.analyticsSteps.add(step)
    noteInteraction({ kind: "onboardingStepViewed", step })
  }

  addScanRoot = async (): Promise<void> => {
    const picked = await open({ directory: true, multiple: false })
    if (typeof picked !== "string") return
    this.update({ scanRoots: await addScanRoot(picked) })
    await this.rescan()
  }

  removeScanRoot = async (path: string): Promise<void> => {
    this.update({ scanRoots: await removeScanRoot(path) })
    await this.rescan()
  }

  rescan = async (): Promise<void> => {
    if (this.rescanInFlight) {
      this.rescanQueued = true
      return
    }

    this.rescanInFlight = true
    try {
      do {
        this.rescanQueued = false
        const status = await scanNow(this.snapshot.activityWindowDays).catch(() => null)
        if (status) this.applyScanStatus(status)
        const permissions = await getFolderPermissions().catch(() => null)
        if (permissions) this.update({ permissions })
      } while (this.rescanQueued)
    } finally {
      this.rescanInFlight = false
    }
  }

  toggleRepository = async (item: LocalRepositoryItem, enabled: boolean): Promise<void> => {
    const payloads = await setRepositoryEnabled(item.key, enabled).catch(() => [])
    if (payloads.length > 0) this.update({ repositories: toRepositories(payloads) })
  }

  recheckPermissions = async (): Promise<void> => {
    this.update({ recheckingPermissions: true })
    const found = await recheckFolderPermissions().catch(() => [])
    if (found.length > 0) {
      await this.rescan()
      await this.refreshRepositories()
    } else {
      const permissions = await getFolderPermissions().catch(() => null)
      if (permissions) this.update({ permissions })
    }
    this.update({ recheckingPermissions: false })
  }

  finish = async (): Promise<void> => {
    if (this.snapshot.finishing) return
    this.update({ finishing: true, finishError: null })
    try {
      await finishOnboarding(
        this.snapshot.activityWindowDays,
        this.snapshot.launchAtLogin,
        this.snapshot.disabledAgents,
        this.snapshot.nudgesRespectDnd,
      )
    } catch (error) {
      this.update({
        finishing: false,
        finishError: error instanceof Error ? error.message : "Could not save your choices.",
      })
    }
  }

  private start = async (): Promise<void> => {
    this.started = true
    const generation = ++this.generation

    const pendingScanListener = onScanEvent((status, phase) => {
      if (generation !== this.generation) return
      this.scanEventVersion += 1
      this.applyScanStatus(status)
      if (phase === "finished") {
        void this.refreshRepositories()
        void this.refreshScanStatus()
      }
    })

    try {
      this.stopScanListening = await pendingScanListener
      if (generation !== this.generation) {
        this.stopScanListening()
        this.stopScanListening = null
        return
      }

      const scanVersion = this.scanEventVersion
      const [settings, info, scanRoots, defaultRoots, scanStatus, permissions, repositories] =
        await Promise.all([
          getSettings(),
          appInfo().catch(() => null),
          listScanRoots().catch(() => []),
          defaultScanRoots().catch(() => []),
          getScanStatus().catch(() => null),
          getFolderPermissions()
            .then((value) => value ?? EMPTY_PERMISSIONS)
            .catch(() => EMPTY_PERMISSIONS),
          listRepositories().catch(() => []),
        ])
      if (generation !== this.generation) return

      applyTheme(settings.theme)
      this.update({
        loadState: "ready",
        loadError: null,
        activityWindowDays: settings.activityWindowDays,
        launchAtLogin: settings.launchAtLogin,
        nudgesRespectDnd: settings.nudgesRespectDnd,
        analyticsSupported: info?.analyticsSupported ?? false,
        analyticsEnvironmentDisabled: info?.analyticsEnvironmentDisabled ?? false,
        scanRoots,
        defaultRoots,
        permissions,
        repositories: toRepositories(repositories),
      })
      if (scanVersion === this.scanEventVersion && scanStatus) this.applyScanStatus(scanStatus)
      this.noteOnboardingStep("welcome")
    } catch (error) {
      if (generation !== this.generation) return
      this.update({
        loadState: "error",
        loadError: error instanceof Error ? error.message : "Could not load onboarding.",
      })
    }
  }

  private stop(): void {
    this.started = false
    this.generation += 1
    this.stopScanListening?.()
    this.stopScanListening = null
    this.stopHygienePolling()
    this.cancelPermissionFlow()
  }

  private refreshRepositories = async (): Promise<void> => {
    const payloads = await listRepositories().catch(() => [])
    this.update({ repositories: toRepositories(payloads) })
  }

  /**
   * Apply a scan status while it keeps the known agent list. Scan events carry
   * an empty `agents` array, so an event must not erase the fetched list.
   */
  private applyScanStatus(status: ScanStatus): void {
    const previous = this.snapshot.scanStatus
    const agents = status.agents.length > 0 ? status.agents : (previous?.agents ?? [])
    const merged = { ...status, agents }
    this.update({ scanStatus: merged })
    this.seedAgentChoices(merged)
  }

  /** Fetch the per-agent scan state, which scan events do not carry. */
  private refreshScanStatus = async (): Promise<void> => {
    const version = this.scanEventVersion
    const fetched = await getScanStatus().catch(() => null)
    if (!fetched) return
    if (version === this.scanEventVersion) {
      this.applyScanStatus(fetched)
      return
    }
    // A newer scan event arrived during the fetch. Keep the event's counters
    // and take only the agent list from the fetched status.
    const current = this.snapshot.scanStatus
    if (current === null || fetched.agents.length === 0) return
    const merged = { ...current, agents: fetched.agents }
    this.update({ scanStatus: merged })
    this.seedAgentChoices(merged)
  }

  /**
   * Seed the draft toggles from scan results until the user touches one.
   * Agents with sessions start on; agents with none start off.
   */
  private seedAgentChoices(status: ScanStatus): void {
    if (this.agentChoicesTouched || status.agents.length === 0) return
    const sessions = new Map(status.agents.map((entry) => [entry.agent, entry.sessionsSeen]))
    const disabled = AGENT_SLUGS.filter((slug) => (sessions.get(slug) ?? 0) === 0)
    this.update({ disabledAgents: disabled })
  }

  private startPermissionFlow = (): void => {
    if (this.permissionRunning) return
    const pending = this.snapshot.permissions.deferred
      .map((entry) => entry.dir)
      .filter(
        (dir) =>
          this.permissionState.verdicts[dir] !== "recorded-denial" &&
          this.permissionState.verdicts[dir] !== "granted",
      )
    if (pending.length === 0) return

    this.permissionCancelled = false
    this.permissionRunning = true
    this.permissionQueue = pending
    this.setPermissionState({
      phase: "idle",
      current: null,
      position: 0,
      total: pending.length,
    })
    void this.runPermissionStep(0)
  }

  private runPermissionStep = async (index: number): Promise<void> => {
    if (this.permissionCancelled) return
    const dir = this.permissionQueue[index]
    if (dir === undefined) {
      this.permissionRunning = false
      this.setPermissionState({ phase: "done", current: null })
      return
    }

    this.setPermissionState({ phase: "asking", current: dir, position: index + 1 })
    let verdict: FolderVerdict
    try {
      const outcome = await requestFolderAccess(dir)
      verdict = outcome.outcome
    } catch {
      verdict = "error"
    }
    if (this.permissionCancelled) return

    this.setPermissionState({
      verdicts: { ...this.permissionState.verdicts, [dir]: verdict },
      ...(verdict === "granted" ? { phase: "settling" as FlowPhase } : {}),
    })
    if (verdict === "granted") {
      void this.rescan()
      void this.refreshRepositories()
    }

    this.permissionTimer = setTimeout(() => {
      this.permissionTimer = null
      void this.runPermissionStep(index + 1)
    }, BETWEEN_FOLDERS_MS)
  }

  private resetPermissionFlow = (): void => {
    this.cancelPermissionFlow()
    this.permissionRunning = false
    this.permissionQueue = []
    this.permissionState = { ...INITIAL_PERMISSION_STATE }
    this.update({ permissionFlow: this.permissionFlow() })
  }

  private cancelPermissionFlow(): void {
    this.permissionCancelled = true
    if (this.permissionTimer !== null) {
      clearTimeout(this.permissionTimer)
      this.permissionTimer = null
    }
  }

  private setPermissionState(change: Partial<PermissionState>): void {
    this.permissionState = { ...this.permissionState, ...change }
    this.update({ permissionFlow: this.permissionFlow() })
  }

  private permissionFlow(): FolderPermissionFlow {
    return {
      ...this.permissionState,
      recordedDenials: Object.entries(this.permissionState.verdicts)
        .filter(([, verdict]) => verdict === "recorded-denial")
        .map(([dir]) => dir),
      start: this.startPermissionFlow,
      reset: this.resetPermissionFlow,
    }
  }

  private update(change: Partial<OnboardingSnapshot>): void {
    this.snapshot = { ...this.snapshot, ...change }
    for (const listener of this.listeners) listener()
  }
}

function toRepositories(payloads: RepositoryItemPayload[]): LocalRepositoryItem[] {
  return payloads.map((payload) => ({ ...payload, status: repositoryStatus(payload.status) }))
}

function repositoryStatus(status: string): LocalRepositoryStatus {
  switch (status) {
    case "accessible":
    case "permission_denied":
    case "not_cloned":
    case "disabled":
      return status
    default:
      return "accessible"
  }
}
