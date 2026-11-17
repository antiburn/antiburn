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
import { personalCloudDestination } from "../lib/cloudDestination"

function selectedFoldersKey(value: CloudSnapshot) {
  return JSON.stringify(
    value.draft.folders
      .filter((folder) => folder.selected)
      .map((folder) => folder.path)
      .sort(),
  )
}

export interface CloudViewSnapshot {
  cloud: CloudSnapshot
  screen: CloudScreen
  sync: SyncResult
  agent: AgentReadiness
  busy: boolean
  error: string | null
  loaded: boolean
  fullContentConsent: boolean
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
  offline: "Cloud could not connect. No sessions were uploaded.",
  rejected: "Your session was not accepted. Sign in again to continue.",
  invalid_response: "Cloud returned an unexpected response. Try again later.",
  credential_failed:
    "Secure connection state is unavailable. Unlock credential storage, or sign out and connect again.",
  unavailable: "Cloud is not available in this build. You can continue locally.",
  reauth_required:
    "Revoke the previous connection in Settings, then connect again. The refresh response is not safe to retry.",
  revocation_unconfirmed:
    "Signed out locally. Cloud has not confirmed revocation. Open Cloud settings to finish cleanup or reset.",
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
    fullContentConsent: false,
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
  private preparedRequestId: string | null = null
  private syncInFlight = false
  private consentRevision = 0
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
    const destinationChanged =
      value.destination?.id !== this.snapshot.cloud.destination?.id ||
      value.destination?.kind !== this.snapshot.cloud.destination?.kind
    const sourcesChanged = selectedFoldersKey(value) !== selectedFoldersKey(this.snapshot.cloud)
    const contextChanged =
      !value.identity ||
      accountChanged ||
      destinationChanged ||
      sourcesChanged ||
      value.status !== "signed_in" ||
      !!value.setupError
    if (contextChanged) {
      this.requestId = null
      this.preparedRequestId = null
      this.consentRevision++
    }
    const screen = value.identity
      ? value.draft.screen === "handoff"
        ? "identity"
        : value.draft.screen
      : "handoff"
    if (value.identity && value.pendingSync) {
      this.requestId = value.pendingSync.requestId
      this.preparedRequestId = value.pendingSync.requestId
    }
    this.publish({
      cloud: value,
      screen: value.identity && value.pendingSync ? "complete" : screen,
      loaded: true,
      ...(contextChanged ? { fullContentConsent: false } : {}),
      ...(value.identity && value.pendingSync
        ? { sync: value.pendingSync.result }
        : contextChanged
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
  resetLocal = () => this.run(null, this.port.resetLocal)
  setFullContentConsent = (fullContentConsent: boolean) => {
    if (fullContentConsent !== this.snapshot.fullContentConsent) this.consentRevision++
    this.publish({ fullContentConsent })
  }
  private saveDraft(draft: CloudDraft, foldersEdited = true) {
    const { identity, destination } = this.snapshot.cloud
    const personal = personalCloudDestination(destination ?? null)
    if (!identity || !personal || this.snapshot.cloud.setupError)
      return Promise.reject(new Error("Choose a verified personal destination."))
    return this.port.save(
      draft,
      { accountId: identity.id, destinationId: personal.id },
      foldersEdited,
    )
  }
  go = async (screen: CloudDraft["screen"], action: "back" | "continue" = "continue") => {
    if (screen !== "handoff" && !this.snapshot.cloud.identity) return
    await this.run(action, () =>
      this.saveDraft({ ...this.snapshot.cloud.draft, screen }, false),
    )
  }
  addFolders = async () => {
    const generation = this.generation
    const account = this.snapshot.cloud.identity?.id
    const destination = this.snapshot.cloud.destination?.id
    await this.run("add_folder", async () => {
      const paths = await this.port.pick()
      if (
        !this.active ||
        generation !== this.generation ||
        account !== this.snapshot.cloud.identity?.id ||
        destination !== this.snapshot.cloud.destination?.id
      )
        return this.snapshot.cloud
      const folders = this.snapshot.cloud.draft.folders.map((folder) => ({ ...folder }))
      let changed = false
      for (const path of paths) {
        const existing = folders.find((folder) => folder.path === path)
        if (existing && !existing.selected) {
          existing.selected = true
          changed = true
        } else if (!existing && folders.length < 100) {
          folders.push({ path, selected: true })
          changed = true
        }
      }
      if (!changed) return this.snapshot.cloud
      this.setFullContentConsent(false)
      return this.saveDraft({ ...this.snapshot.cloud.draft, folders })
    })
  }
  selectFolder = (path: string, selected: boolean) => {
    this.setFullContentConsent(false)
    return this.run("select_folder", () =>
      this.saveDraft({
        ...this.snapshot.cloud.draft,
        folders: this.snapshot.cloud.draft.folders.map((folder) =>
          folder.path === path ? { ...folder, selected } : folder,
        ),
      }),
    )
  }
  removeFolder = (path: string) => {
    this.setFullContentConsent(false)
    return this.run(null, () =>
      this.saveDraft({
        ...this.snapshot.cloud.draft,
        folders: this.snapshot.cloud.draft.folders.filter((folder) => folder.path !== path),
      }),
    )
  }
  startSync = async () => {
    const selected = this.snapshot.cloud.draft.folders.filter((folder) => folder.selected)
    const selectionKey = selectedFoldersKey(this.snapshot.cloud)
    const destination = personalCloudDestination(this.snapshot.cloud.destination ?? null)
    if (
      this.snapshot.busy ||
      this.syncInFlight ||
      (!this.snapshot.cloud.captureEnabled && this.snapshot.screen !== "folders") ||
      !(this.snapshot.cloud.captureEnabled || this.snapshot.cloud.syncEnabled) ||
      this.snapshot.cloud.status !== "signed_in" ||
      !!this.snapshot.cloud.setupError ||
      !this.snapshot.cloud.identity ||
      !destination ||
      !this.snapshot.fullContentConsent ||
      this.snapshot.cloud.pendingSync?.result.status === "cancelled" ||
      !selected.length
    )
      return
    if (this.analytics)
      noteCloudAction(
        this.snapshot.cloud.captureEnabled ? this.snapshot.screen : "folders",
        "start_sync",
      )
    const generation = this.generation
    const revision = this.revision
    const account = this.snapshot.cloud.identity.id
    const nativeCapture = this.snapshot.cloud.captureEnabled === true
    const destinationId = destination.id
    const consent = {
      version: "native-full-content-v1" as const,
      accountId: account,
      destinationId,
      fullContent: true as const,
      attachments: false as const,
      selectedRoots: selected.map((folder) => folder.path).sort(),
    }
    const requestId = this.requestId ?? crypto.randomUUID()
    this.requestId = requestId
    const existing = this.preparedRequestId === requestId
    const consentRevision = this.consentRevision
    const current = () =>
      this.active &&
      generation === this.generation &&
      this.requestId === requestId &&
      this.snapshot.cloud.status === "signed_in" &&
      !this.snapshot.cloud.setupError &&
      this.snapshot.cloud.identity?.id === account &&
      personalCloudDestination(this.snapshot.cloud.destination ?? null)?.id === destinationId &&
      selectedFoldersKey(this.snapshot.cloud) === selectionKey &&
      this.snapshot.fullContentConsent &&
      consentRevision === this.consentRevision &&
      !(
        this.snapshot.cloud.pendingSync?.requestId === requestId &&
        this.snapshot.cloud.pendingSync.result.status === "cancelled"
      )
    this.syncInFlight = true
    this.publish({ screen: "complete", busy: false, sync: { status: "starting" }, error: null })
    try {
      if (!existing) {
        await this.port.prepareSync(selected, requestId, consent)
        if (!current()) return
        this.preparedRequestId = requestId
      }
      if (!current()) return
      const sync =
        existing && !nativeCapture
          ? await this.port.reconcile(requestId)
          : await this.port.start(selected, requestId, consent)
      if (current() && (nativeCapture || revision === this.revision))
        this.publish({ sync, busy: false, screen: "complete" })
    } catch {
      if (current() && (nativeCapture || revision === this.revision)) {
        const saved = nativeCapture && this.preparedRequestId === requestId
        this.publish({
          sync: { status: saved ? "upload_unavailable" : "ambiguous" },
          busy: false,
          screen: "complete",
          ...(saved
            ? {
                error:
                  "Could not finish listing sources. Your request is saved. Retry source listing.",
              }
            : {}),
        })
      }
    } finally {
      this.syncInFlight = false
    }
  }
}
