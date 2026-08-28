import { open } from "@tauri-apps/plugin-dialog"

import { applyTheme } from "../../lib/appearance"
import {
  addScanRoot,
  appInfo,
  cancelScan,
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
  type RepositoryItemPayload,
  type ScanStatus,
} from "../../lib/ipc"
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

export type OnboardingStep =
  "welcome" | "sources" | "repositories" | "historical_scan" | "ready"

const EMPTY_PERMISSIONS: FolderPermissions = {
  deferred: [],
  granted: [],
  supported: false,
}

const BETWEEN_FOLDERS_MS = 800

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

  setActivityWindowDays = (days: number): void => {
    this.update({ activityWindowDays: days, finishError: null })
  }

  setLaunchAtLogin = (enabled: boolean): void => {
    this.update({ launchAtLogin: enabled, finishError: null })
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
  }

  removeScanRoot = async (path: string): Promise<void> => {
    this.update({ scanRoots: await removeScanRoot(path) })
  }

  rescan = async (): Promise<void> => {
    const status = await scanNow(this.snapshot.activityWindowDays).catch(() => null)
    if (status) this.update({ scanStatus: status })
    const permissions = await getFolderPermissions().catch(() => null)
    if (permissions) this.update({ permissions })
  }

  cancelScan = async (): Promise<void> => {
    const status = await cancelScan().catch(() => null)
    if (status) this.update({ scanStatus: status })
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
      await finishOnboarding(this.snapshot.activityWindowDays, this.snapshot.launchAtLogin)
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
      this.update({ scanStatus: status })
      if (phase === "finished") void this.refreshRepositories()
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
        analyticsSupported: info?.analyticsSupported ?? false,
        analyticsEnvironmentDisabled: info?.analyticsEnvironmentDisabled ?? false,
        scanRoots,
        defaultRoots,
        permissions,
        repositories: toRepositories(repositories),
        ...(scanVersion === this.scanEventVersion ? { scanStatus } : {}),
      })
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
    this.cancelPermissionFlow()
  }

  private refreshRepositories = async (): Promise<void> => {
    const payloads = await listRepositories().catch(() => [])
    this.update({ repositories: toRepositories(payloads) })
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
