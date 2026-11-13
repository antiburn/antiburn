import {
  emptyCloudSnapshot,
  nativeCloudPort,
  type AgentReadiness,
  type CloudDraft,
  type CloudPort,
  type CloudScreen,
  type CloudSnapshot,
  type SyncResult,
} from "../lib/cloud"
import { noteCloudAction, noteCloudScreenViewed } from "../lib/onboardingAnalytics"

export interface CloudViewSnapshot {
  cloud: CloudSnapshot
  screen: CloudScreen
  sync: SyncResult
  agent: AgentReadiness
  busy: boolean
  error: string | null
  loaded: boolean
}
const messages = {
  idle: "",
  opening: "Opening your browser…",
  waiting: "Waiting for browser…",
  returned: "Welcome back. Cloud is coming soon. Continue using Antiburn locally.",
  signed_in: "Your account is verified. No sessions have been uploaded.",
  signed_out: "You’re signed out on this device.",
  cancelled: "Browser handoff cancelled. You can try again or continue locally.",
  expired: "This browser link expired. Try again to open a new link.",
  browser_failed: "Your browser could not open. Try again or continue locally.",
  offline:
    "Cloud could not connect. Your saved credentials are kept. Try again when you’re online.",
  rejected: "Your session was not accepted. Sign in again to continue.",
  invalid_response: "Cloud returned an unexpected response. Try again later.",
  credential_failed: "Secure credential storage is unavailable. Unlock it and try again.",
  unavailable: "Cloud is not available in this build. You can continue locally.",
} as const
export const cloudStatusMessage = (status: CloudSnapshot["status"]) => messages[status]

export class CloudSession {
  private snapshot: CloudViewSnapshot = {
    cloud: emptyCloudSnapshot,
    screen: "handoff",
    sync: { status: "unavailable" },
    agent: "pending",
    busy: false,
    error: null,
    loaded: false,
  }
  private listeners = new Set<() => void>()
  private stop: (() => void) | undefined
  private timer: ReturnType<typeof setTimeout> | undefined
  private generation = 0
  private revision = 0
  private active = false
  private visible = true
  private viewed: CloudScreen | null = null
  private requestId: string | null = null
  private readonly port: CloudPort
  private readonly analytics: boolean
  private readonly holdIdentity: boolean
  constructor(port: CloudPort = nativeCloudPort, analytics = true, holdIdentity = false) {
    this.port = port
    this.analytics = analytics
    this.holdIdentity = holdIdentity
  }
  getSnapshot = () => this.snapshot
  subscribe = (listener: () => void) => {
    this.listeners.add(listener)
    if (!this.active) {
      this.active = true
      document.addEventListener("visibilitychange", this.visibility)
      void this.load()
    }
    return () => {
      this.listeners.delete(listener)
      if (this.listeners.size === 0) {
        this.active = false
        this.generation++
        this.stop?.()
        this.stop = undefined
        clearTimeout(this.timer)
        document.removeEventListener("visibilitychange", this.visibility)
        this.viewed = null
      }
    }
  }
  setVisible = (visible: boolean) => {
    if (this.visible === visible) return
    this.visible = visible
    this.visibility()
  }
  private visibility = () => {
    if (!this.visible || document.hidden) this.viewed = null
    this.schedule()
    this.recordView()
  }
  private recordView() {
    if (
      this.analytics &&
      this.active &&
      this.visible &&
      !document.hidden &&
      this.snapshot.loaded &&
      this.viewed !== this.snapshot.screen
    ) {
      this.viewed = this.snapshot.screen
      noteCloudScreenViewed(this.snapshot.screen)
    }
  }
  private publish(patch: Partial<CloudViewSnapshot>) {
    this.snapshot = { ...this.snapshot, ...patch }
    this.listeners.forEach((listener) => listener())
    this.recordView()
    this.schedule()
  }
  private receive(value: CloudSnapshot) {
    this.revision++
    const accountChanged = value.identity?.id !== this.snapshot.cloud.identity?.id
    if (!value.identity || accountChanged) this.requestId = null
    const screen = value.identity
      ? value.draft.screen === "handoff"
        ? "identity"
        : value.draft.screen
      : "handoff"
    if (value.identity && value.pendingSync) this.requestId = value.pendingSync.requestId
    this.publish({
      cloud: value,
      screen: value.identity && value.pendingSync ? "complete" : screen,
      loaded: true,
      ...(value.identity && value.pendingSync
        ? { sync: value.pendingSync.result }
        : !value.identity || accountChanged
          ? { sync: { status: "unavailable" as const } }
          : {}),
    })
  }
  private schedule() {
    clearTimeout(this.timer)
    if (
      this.active &&
      this.visible &&
      !document.hidden &&
      !this.snapshot.busy &&
      this.snapshot.screen === "identity" &&
      !this.holdIdentity &&
      !this.snapshot.error
    ) {
      const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches
      const token = reduced ? "--cloud-identity-handoff-reduced" : "--cloud-identity-handoff"
      const delay = Number.parseFloat(
        getComputedStyle(document.documentElement).getPropertyValue(token),
      )
      this.timer = setTimeout(
        () => void this.go("folders"),
        Number.isFinite(delay) ? delay : reduced ? 2400 : 2960,
      )
      return
    }
    if (
      this.active &&
      this.visible &&
      !document.hidden &&
      !this.snapshot.busy &&
      this.snapshot.cloud.authEnabled &&
      ["opening", "waiting"].includes(this.snapshot.cloud.status)
    ) {
      const expiresAt = this.snapshot.cloud.expiresAt
      if (expiresAt)
        this.timer = setTimeout(
          () => void this.refresh(),
          Math.max(1, expiresAt * 1000 - Date.now() + 100),
        )
    }
  }
  private async refresh() {
    const generation = this.generation
    const revision = this.revision
    try {
      const value = await this.port.read()
      if (this.active && generation === this.generation && revision === this.revision)
        this.receive(value)
    } catch {
      if (generation === this.generation)
        this.publish({ error: "Could not check Cloud. Try again." })
    }
  }
  load = async () => {
    const generation = this.generation
    this.publish({ busy: true, error: null })
    try {
      if (!this.stop) {
        const stop = await this.port.watch((value) => {
          if (this.active && generation === this.generation) this.receive(value)
        })
        if (!this.active || generation !== this.generation) {
          stop()
          return
        }
        this.stop = stop
      }
      const revision = this.revision
      const value = await this.port.read()
      if (!this.active || generation !== this.generation) return
      const appliedRead = revision === this.revision
      if (appliedRead) this.receive(value)
      if (appliedRead && value.identity && value.pendingSync) {
        this.requestId = value.pendingSync.requestId
        this.publish({
          screen: "complete",
          busy: false,
          sync: value.pendingSync.result,
        })
        const reconcileRevision = this.revision
        const sync = await this.port.reconcile(this.requestId)
        if (!this.active || generation !== this.generation) return
        if (reconcileRevision !== this.revision) {
          this.publish({ busy: false })
          return
        }
        this.publish({ sync, screen: "complete" })
      }
      this.publish({ busy: false })
      void this.port
        .agent()
        .then((agent) => {
          if (this.active && generation === this.generation) this.publish({ agent })
        })
        .catch(() => {
          if (this.active && generation === this.generation) this.publish({ agent: "failed" })
        })
    } catch {
      if (this.active && generation === this.generation)
        this.publish({
          busy: false,
          loaded: true,
          error: "Could not load Cloud. Try again or continue locally.",
        })
    }
  }
  private async run(
    action: Parameters<typeof noteCloudAction>[1] | null,
    task: () => Promise<CloudSnapshot>,
  ) {
    if (this.snapshot.busy) return false
    if (this.analytics && action) noteCloudAction(this.snapshot.screen, action)
    const generation = this.generation
    this.publish({ busy: true, error: null })
    const revision = this.revision
    try {
      const value = await task()
      if (!this.active || generation !== this.generation) return false
      const fresh = revision === this.revision
      if (fresh) this.receive(value)
      this.publish({ busy: false })
      const consistent = fresh || JSON.stringify(this.snapshot.cloud) === JSON.stringify(value)
      return consistent && value.status !== "credential_failed"
    } catch {
      if (this.active && generation === this.generation)
        this.publish({ busy: false, error: "Cloud could not complete that action. Try again." })
      return false
    }
  }
  begin = () =>
    this.run(this.snapshot.cloud.authEnabled ? "sign_in" : "learn_more", this.port.begin)
  retry = () =>
    !this.snapshot.cloud.authEnabled
      ? this.begin()
      : this.run("retry", async () => {
          const cancelled = await this.port.cancel()
          if (cancelled.status === "credential_failed") return cancelled
          return this.port.begin()
        })
  cancel = () => this.run("cancel", this.port.cancel)
  cancelSilently = () => this.run(null, this.port.cancel)
  signOut = () => this.run("sign_out", this.port.signOut)
  go = async (screen: CloudDraft["screen"], action: "back" | "continue" = "continue") => {
    if (screen !== "handoff" && !this.snapshot.cloud.identity) return
    await this.run(action, () =>
      this.port.save({ ...this.snapshot.cloud.draft, screen }, false),
    )
  }
  addFolders = async () => {
    const generation = this.generation
    await this.run("add_folder", async () => {
      const paths = await this.port.pick()
      if (!this.active || generation !== this.generation) return this.snapshot.cloud
      const folders = [...this.snapshot.cloud.draft.folders]
      for (const path of paths)
        if (!folders.some((folder) => folder.path === path) && folders.length < 100)
          folders.push({ path, selected: true })
      if (folders.length === this.snapshot.cloud.draft.folders.length)
        return this.snapshot.cloud
      return this.port.save({ ...this.snapshot.cloud.draft, folders })
    })
  }
  selectFolder = (path: string, selected: boolean) =>
    this.run("select_folder", () =>
      this.port.save({
        ...this.snapshot.cloud.draft,
        folders: this.snapshot.cloud.draft.folders.map((folder) =>
          folder.path === path ? { ...folder, selected } : folder,
        ),
      }),
    )
  removeFolder = (path: string) =>
    this.run(null, () =>
      this.port.save({
        ...this.snapshot.cloud.draft,
        folders: this.snapshot.cloud.draft.folders.filter((folder) => folder.path !== path),
      }),
    )
  startSync = async () => {
    const selected = this.snapshot.cloud.draft.folders.filter((folder) => folder.selected)
    if (
      this.snapshot.busy ||
      this.snapshot.screen !== "folders" ||
      !this.snapshot.cloud.syncEnabled ||
      !this.snapshot.cloud.identity ||
      !selected.length
    )
      return
    if (this.analytics) noteCloudAction("folders", "start_sync")
    const generation = this.generation
    const revision = this.revision
    const account = this.snapshot.cloud.identity.id
    const existing = this.requestId
    const requestId = this.requestId ?? crypto.randomUUID()
    this.requestId = requestId
    this.publish({ screen: "complete", busy: false, sync: { status: "starting" }, error: null })
    try {
      if (!existing) await this.port.prepareSync(selected, requestId)
      if (this.snapshot.cloud.identity?.id !== account) return
      const sync = existing
        ? await this.port.reconcile(existing)
        : await this.port.start(selected, requestId)
      if (this.active && generation === this.generation && revision === this.revision)
        this.publish({ sync, busy: false, screen: "complete" })
    } catch {
      if (this.active && generation === this.generation && revision === this.revision)
        this.publish({ sync: { status: "ambiguous" }, busy: false, screen: "complete" })
    }
  }
}
