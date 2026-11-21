import {
  emptyCloudSnapshot,
  CLOUD_BUSY_MESSAGE,
  desktopCloudPort,
  cloudSourceKind,
  cloudProjectChoices,
  mergeCloudProjectGroups,
  cloudProjectSelectionKey,
  type CloudDraft,
  type CloudPort,
  type CloudScreen,
  type CloudSnapshot,
  type CloudSourceKind,
  type CaptureConsent,
  type SyncResult,
  type CloudProjectGroup,
  type CloudProjectChoices,
  type DesktopCloudProjectGroup,
  type CloudProjectSelection,
  type CloudOngoingStatus,
  type CloudOngoingConsent,
  type CloudDestinationBinding,
  type CloudProjectCursor,
  type CloudSourceSelection,
} from "../lib/cloud"
import { noteCloudAction, noteCloudScreenViewed } from "../lib/onboardingAnalytics"
import { personalCloudDestination } from "../lib/cloudDestination"
import type { LocalToolsStatus } from "../lib/ipcPayloads"
import {
  confirmationContextKey,
  folderDiscoveryBinding,
  folderGroupHasPolicy,
  folderRouteConsent,
  folderRouteControlBinding,
  folderRouteResults,
  folderRoutesOverlap,
  folderRoutingAuthority,
  lockedFolderRoutes,
  matchesFolderConsent,
  personalFolderRemovalBinding,
  readSavedFolderPolicies,
  sameSavedFolderInventory,
  savedFolderPolicyStatuses,
  savedPersonalFolderPolicy,
  selectedFoldersKey,
  type FolderAssignment,
  type FolderRoutes,
} from "./cloudFolderRouting"

/** One surface's view state. Start or a confirmed Settings addition grants content consent. */
export interface CloudViewSnapshot {
  /** Fence actions rendered before an account, source, or folder selection change. */
  contextRevision: number
  /** Current home is display-only. Raw source and project paths retain authority. */
  homeDirectory: string | null
  cloud: CloudSnapshot
  screen: CloudScreen
  sync: SyncResult
  /** CLI and skill evidence is independent of Cloud authority and vendor hooks. */
  localTools: LocalToolsStatus | null
  /** A failed status read or subscription cannot establish current tool readiness. */
  localToolsError: boolean
  busy: boolean
  authAction?: "check_sign_in" | "sign_in" | "sign_out" | null
  error: string | null
  loaded: boolean
  /** Retry the saved request's listing even after a transient warning clears. */
  sourceListingNeedsRetry: boolean
  /** Desktop project discovery with local defaults. No selection write happens on load. */
  projects?: {
    groups: CloudProjectGroup[]
    choices: CloudProjectChoices
    loading: boolean
    loaded: boolean
    partial: boolean
    hasMore: boolean
    needsChoiceReview: boolean
    error: string | null
    selectionKey: string
  } | null
  /** Desktop-owned ongoing policy and transfer counters, separate from local agents. */
  ongoing?: CloudOngoingStatus | null
  /** Exact saved route states. Null means the full saved inventory was not verified. */
  routedPolicies?: readonly CloudOngoingStatus[] | null
  /** Unsaved choices stay local. An unconfirmed result is not an upload status. */
  routedFolders?: {
    choices: FolderRoutes
    locked: FolderRoutes
    ready: boolean
    results: readonly { id: string; name: string; saved: boolean }[]
    retry: boolean
    message: string | null
  } | null
}
const messages = {
  idle: "",
  opening: "Opening your browser…",
  waiting: "Waiting for browser approval. Finish in your browser, then return here.",
  returned: "Browser step complete. Checking your sign-in…",
  signed_in: "Your account is verified.",
  signed_out: "You’re signed out on this device.",
  cancelled: "Sign-in cancelled.",
  expired: "This browser link expired. Try again to open a new link.",
  browser_failed: "Your browser could not open. Try again or continue locally.",
  offline: "Cloud could not connect. Check your connection and try again.",
  sign_in_check_failed: "Couldn’t check sign-in. Try again.",
  sign_out_failed: "Couldn’t sign out. Try again.",
  rejected: "Cloud could not verify this connection. Sign in again to continue.",
  invalid_response: "Cloud returned an unexpected response. Try again later.",
  credential_failed: "Could not complete the account action. Try again.",
  unavailable: "Cloud is not available in this build. You can continue locally.",
  reauth_required: "Your sign-in ended. Sign in again to use Cloud.",
} as const
/** Keep the browser sign-in action named the same in setup and Settings. */
export const cloudGithubSignInLabel = "Sign in with GitHub"
/** Map closed desktop states to safe copy without exposing provider errors. */
export const cloudStatusMessage = (status: CloudSnapshot["status"]) => messages[status]
/** Distinguish fresh setup from an unreadable saved-upload state. */
export const cloudUploadStatusUnknown = (value: CloudOngoingStatus | null | undefined) =>
  !!value && !value.storageKnown && value.status !== "consent_required"
/** Missing storage evidence must not turn a cached state into a current Ready claim. */
export const cloudOngoingStatusMessage = (value: CloudOngoingStatus) => {
  if (cloudUploadStatusUnknown(value)) return "Saved upload status is unavailable"
  return {
    consent_required: "Cloud sync not started",
    ready: "Background sync enabled",
    syncing: "Sessions syncing",
    paused: "Cloud uploads paused",
    offline: "Cloud is offline",
    reauth_required: "Reconnect to Cloud",
    upload_unavailable: "Cloud upload unavailable",
    fenced: "Some uploads need attention",
    interrupted: "Some sessions could not be prepared",
    storage_unavailable: "Could not open saved uploads",
  }[value.status]
}

/** Accept only the exact nonsecret admission error, not arbitrary desktop/provider text. */
function isCloudBusy(error: unknown) {
  return (
    error === CLOUD_BUSY_MESSAGE ||
    (error instanceof Error && error.message === CLOUD_BUSY_MESSAGE)
  )
}

/**
 * Own one surface's Cloud view, timers, and explicit Start actions.
 * Desktop state owns credentials and durable requests. Events can arrive before
 * IPC replies; revisions reject late replies, and context changes clear consent.
 * Unsubscribing stops observation, not saved requests or desktop byte retention.
 */
export class CloudSession {
  private snapshot: CloudViewSnapshot = {
    contextRevision: 0,
    homeDirectory: null,
    cloud: emptyCloudSnapshot,
    screen: "handoff",
    sync: { status: "unavailable" },
    localTools: null,
    localToolsError: false,
    busy: false,
    error: null,
    loaded: false,
    sourceListingNeedsRetry: false,
  }
  private listeners = new Set<() => void>()
  private stop: (() => void) | undefined
  private stopOngoing: (() => void) | undefined
  private stopLocalTools: (() => void) | undefined
  private localToolsLoading = false
  private localToolsEventGeneration = 0
  private localToolsStatusRevision = -1
  private projectGeneration = 0
  private desktopProjects: DesktopCloudProjectGroup[] = []
  private projectReadBinding: CloudDestinationBinding | null = null
  private projectSources: CloudSourceSelection[] = []
  private projectCursor: CloudProjectCursor | null = null
  private projectChoices: CloudProjectChoices = {}
  private confirmedProjectChoices: CloudProjectChoices = {}
  private routeChoices: FolderRoutes = {}
  private routePolicies = new Map<string, CloudOngoingStatus>()
  private verifiedRouteIds: readonly string[] | null = null
  private routedScreen: CloudScreen | null = null
  private routeBatch: {
    revision: number
    authority: string
    completeOnSave: boolean
    knownDestinationIds: ReadonlySet<string>
    entries: { consent: CloudOngoingConsent; name: string; saved: boolean }[]
  } | null = null
  private needsChoiceReview = false
  private ongoingRevision = 0
  private projectSyncStarted = false
  private timer: ReturnType<typeof setTimeout> | undefined
  private identityTimer = false
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
  /** Inject isolated previews/tests explicitly. Production uses the desktop capability gate. */
  constructor(port: CloudPort = desktopCloudPort, analytics = true, holdIdentity = false) {
    this.port = port
    this.analytics = analytics
    this.holdIdentity = holdIdentity
  }
  /** Return the stable snapshot expected by useSyncExternalStore. */
  getSnapshot = () => this.snapshot
  /** Distinguish current desktop ongoing sync from legacy saved-request fixtures. */
  get projectSyncSupported() {
    return this.port.ongoing !== undefined
  }
  /** Start observation for the first reader; the last unsubscribe fences late replies. */
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
        this.stopOngoing?.()
        this.stopOngoing = undefined
        this.stopLocalTools?.()
        this.stopLocalTools = undefined
        this.localToolsLoading = false
        this.projectGeneration++
        clearTimeout(this.timer)
        document.removeEventListener("visibilitychange", this.visibility)
        this.viewed = null
      }
    }
  }
  /** Suspend timers and screen analytics while the host surface is hidden. */
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
  /** Count visible entries only, not state events or repeated renders. */
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
  /** Replace view state and notify subscribers without changing desktop authority. */
  private publish(patch: Partial<CloudViewSnapshot>) {
    this.snapshot = { ...this.snapshot, ...patch, contextRevision: this.consentRevision }
    this.listeners.forEach((listener) => listener())
    this.recordView()
    this.schedule()
  }
  /** Desktop state owns bindings. Clear consent on authority/root changes and let
   * completed desktop work clear a transient admission warning, not a real failure.
   * Setup failures fence pending discovery and keep cached choices available.
   */
  private receive(value: CloudSnapshot) {
    this.revision++
    const routed = !!this.port.ongoing && value.destinations !== undefined
    const routeChanged =
      routed && folderRoutingAuthority(value) !== folderRoutingAuthority(this.snapshot.cloud)
    const inventoryChanged =
      routed &&
      !this.routeBatch &&
      !sameSavedFolderInventory(
        value.savedDestinationIds,
        this.snapshot.cloud.savedDestinationIds,
      )
    const accountChanged = value.identity?.id !== this.snapshot.cloud.identity?.id
    const activeDestinationChanged =
      value.destination?.id !== this.snapshot.cloud.destination?.id ||
      value.destination?.kind !== this.snapshot.cloud.destination?.kind
    const destinationChanged = !routed && activeDestinationChanged
    const sourcesChanged = selectedFoldersKey(value) !== selectedFoldersKey(this.snapshot.cloud)
    const contextChanged =
      !value.identity ||
      accountChanged ||
      destinationChanged ||
      sourcesChanged ||
      value.status !== "signed_in" ||
      !!value.setupError
    if (
      accountChanged ||
      destinationChanged ||
      !value.identity ||
      value.status !== "signed_in"
    ) {
      this.projectGeneration++
      this.desktopProjects = []
      this.projectReadBinding = null
      this.projectSources = []
      this.projectCursor = null
      this.projectChoices = {}
      this.confirmedProjectChoices = {}
      this.needsChoiceReview = false
      this.projectSyncStarted = false
      this.routeChoices = {}
      this.routePolicies.clear()
      this.verifiedRouteIds = null
      this.routedScreen = null
      this.routeBatch = null
    } else if (value.setupError) {
      this.projectGeneration++
    }
    if (routeChanged) {
      this.projectGeneration++
      this.routeBatch = null
      this.invalidateContentConfirmation()
    }
    const retryInvalidated =
      contextChanged ||
      (value.pendingSync &&
        (value.pendingSync.requestId !== this.preparedRequestId ||
          value.pendingSync.result.status === "cancelled"))
    if (contextChanged) {
      this.requestId = null
      this.preparedRequestId = null
      this.consentRevision++
    }
    const screen = value.identity
      ? routed && this.routedScreen
        ? this.routedScreen
        : value.draft.screen === "handoff"
          ? "identity"
          : value.draft.screen
      : "handoff"
    if (!this.port.ongoing && value.identity && value.pendingSync) {
      this.requestId = value.pendingSync.requestId
      this.preparedRequestId = value.pendingSync.requestId
    }
    this.publish({
      cloud: value,
      ...(routed &&
      (accountChanged ||
        inventoryChanged ||
        value.status !== "signed_in" ||
        !value.savedDestinationIds)
        ? { routedPolicies: null }
        : {}),
      ...(routed && activeDestinationChanged ? { ongoing: null } : {}),
      ...(routed && routeChanged
        ? {
            routedFolders: {
              choices: this.routeChoices,
              locked: lockedFolderRoutes(
                this.snapshot.projects?.groups ?? [],
                this.routePolicies,
              ),
              ready: false,
              results: accountChanged ? [] : (this.snapshot.routedFolders?.results ?? []),
              retry: false,
              message: accountChanged
                ? null
                : "Cloud access changed. Review your folder choices.",
            },
            ...(this.snapshot.projects
              ? { projects: { ...this.snapshot.projects, loading: false } }
              : {}),
          }
        : {}),
      ...(inventoryChanged && !routeChanged && this.snapshot.routedFolders
        ? { routedFolders: { ...this.snapshot.routedFolders, ready: false } }
        : {}),
      ...(accountChanged ||
      destinationChanged ||
      !value.identity ||
      value.status !== "signed_in"
        ? { projects: null, ongoing: null }
        : value.setupError && this.snapshot.projects
          ? { projects: { ...this.snapshot.projects, loading: false } }
          : {}),
      screen:
        value.identity &&
        (this.port.ongoing
          ? !routed && (this.projectSyncStarted || value.syncEnabled)
          : value.pendingSync)
          ? "complete"
          : screen,
      loaded: true,
      ...(this.snapshot.error === CLOUD_BUSY_MESSAGE ? { error: null } : {}),
      ...(retryInvalidated ? { sourceListingNeedsRetry: false } : {}),
      ...(!this.port.ongoing && value.identity && value.pendingSync
        ? { sync: value.pendingSync.result }
        : contextChanged
          ? { sync: { status: "unavailable" as const } }
          : {}),
    })
  }
  /** Schedule only visible identity handoff or browser expiry; never poll signed-in accounts. */
  private schedule() {
    const handoff =
      this.active &&
      this.visible &&
      !document.hidden &&
      !this.snapshot.busy &&
      this.snapshot.screen === "identity" &&
      !this.holdIdentity &&
      !this.snapshot.cloud.invitationEnabled &&
      !this.snapshot.cloud.setupError &&
      !this.snapshot.error
    if (handoff && this.identityTimer) return
    clearTimeout(this.timer)
    this.identityTimer = false
    if (handoff) {
      this.identityTimer = true
      const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches
      const token = reduced ? "--cloud-identity-handoff-reduced" : "--cloud-identity-handoff"
      const delay = Number.parseFloat(
        getComputedStyle(document.documentElement).getPropertyValue(token),
      )
      this.timer = setTimeout(
        () => {
          this.identityTimer = false
          void this.go("folders")
        },
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
      if (expiresAt) {
        const delay = expiresAt * 1000 - Date.now() + 100
        // Desktop code owns expiry. An elapsed timer must not turn busy replies into polling.
        if (delay > 0) this.timer = setTimeout(() => void this.refresh(), delay)
      }
    }
  }
  /** Discard reads superseded by a desktop event or a later subscription generation. */
  private async refresh() {
    const generation = this.generation
    const revision = this.revision
    try {
      const value = await this.port.read()
      if (this.active && generation === this.generation && revision === this.revision)
        this.receive(value)
    } catch (error) {
      if (generation === this.generation)
        this.publish({
          error: isCloudBusy(error) ? CLOUD_BUSY_MESSAGE : "Could not check Cloud. Try again.",
        })
    }
  }
  /** Observe tools without blocking account or folder reads. Order successful
   * results by native revision. Later events take priority over query errors,
   * which have no revision.
   */
  private async loadLocalTools() {
    if (!this.active || this.localToolsLoading) return
    const generation = this.generation
    const current = () => this.active && generation === this.generation
    this.localToolsLoading = true
    let eventGeneration = this.localToolsEventGeneration
    try {
      if (!this.stopLocalTools) {
        this.publish({ localTools: null, localToolsError: false })
        const stop = await this.port.localTools.watch((localTools) => {
          if (current() && localTools.revision >= this.localToolsStatusRevision) {
            this.localToolsEventGeneration++
            this.localToolsStatusRevision = localTools.revision
            this.publish({ localTools, localToolsError: false })
          }
        })
        if (!current()) {
          stop()
          return
        }
        this.stopLocalTools = stop
      }
      eventGeneration = this.localToolsEventGeneration
      const localTools = await this.port.localTools.read()
      if (current() && localTools.revision >= this.localToolsStatusRevision) {
        this.localToolsStatusRevision = localTools.revision
        this.publish({ localTools, localToolsError: false })
      }
    } catch {
      if (current() && eventGeneration === this.localToolsEventGeneration)
        this.publish({ localToolsError: true })
    } finally {
      if (current()) this.localToolsLoading = false
    }
  }
  /** Attach events before independent account/tool reads. Restore saved IDs
   * without starting a request; neither status path blocks the other.
   */
  load = async (authAction: "check_sign_in" | null = null) => {
    const generation = this.generation
    this.publish({
      busy: true,
      authAction,
      error: null,
    })
    void this.loadLocalTools()
    try {
      if (!this.stop) {
        const stop = await this.port.watch((value) => {
          if (this.active && generation === this.generation) {
            const setupRecovered = !!this.snapshot.cloud.setupError && !value.setupError
            const routesChanged =
              value.destinations !== undefined &&
              folderRoutingAuthority(value) !== folderRoutingAuthority(this.snapshot.cloud)
            const inventoryChanged = !sameSavedFolderInventory(
              value.savedDestinationIds,
              this.snapshot.cloud.savedDestinationIds,
            )
            this.receive(value)
            if (
              this.port.ongoing &&
              (this.discoveryBinding() ||
                (value.destinations !== undefined &&
                  value.identity &&
                  value.status === "signed_in")) &&
              !this.snapshot.busy &&
              (!this.snapshot.projects ||
                setupRecovered ||
                routesChanged ||
                (inventoryChanged && !this.routeBatch))
            )
              void this.loadProjects()
          }
        })
        if (!this.active || generation !== this.generation) {
          stop()
          return
        }
        this.stop = stop
      }
      if (this.port.ongoing && !this.stopOngoing) {
        const stop = await this.port.ongoing.watch((status) => {
          if (this.active && generation === this.generation) this.receiveOngoing(status)
        })
        if (!this.active || generation !== this.generation) {
          stop()
          return
        }
        this.stopOngoing = stop
      }
      if (this.port.displayHome) {
        void this.port
          .displayHome()
          .then((homeDirectory) => {
            if (this.active && generation === this.generation) this.publish({ homeDirectory })
          })
          .catch(() => {
            // A failed display lookup leaves raw paths visible without blocking setup.
            if (this.active && generation === this.generation)
              this.publish({ homeDirectory: null })
          })
      }
      const revision = this.revision
      const value = await this.port.read()
      if (!this.active || generation !== this.generation) return
      const appliedRead = revision === this.revision
      if (appliedRead) this.receive(value)
      if (!this.port.ongoing && appliedRead && value.identity && value.pendingSync) {
        this.requestId = value.pendingSync.requestId
        this.publish({
          screen: "complete",
          busy: false,
          authAction: null,
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
      this.publish({ busy: false, authAction: null })
      if (this.port.ongoing && this.snapshot.cloud.identity) await this.loadProjects()
    } catch (error) {
      if (this.active && generation === this.generation)
        this.publish({
          busy: false,
          authAction: null,
          loaded: true,
          error: isCloudBusy(error)
            ? CLOUD_BUSY_MESSAGE
            : "Could not load Cloud. Try again or continue locally.",
        })
    }
  }
  /** Accept only the observed account's policy. No status event grants new consent. */
  private receiveOngoing(status: CloudOngoingStatus) {
    const { identity, destination, status: authStatus } = this.snapshot.cloud
    const personal = personalCloudDestination(destination ?? null)
    if (this.snapshot.cloud.destinations !== undefined) {
      const id = status.destinationId
      if (!identity || authStatus !== "signed_in" || status.accountId !== identity.id || !id)
        return
      if (this.snapshot.cloud.savedDestinationIds?.includes(id)) {
        const previous = this.routePolicies.get(id)
        const policyKnown = status.storageKnown && !!status.policyRevision
        this.routePolicies.set(id, status)
        if (!policyKnown) this.verifiedRouteIds = null
        const route = this.snapshot.routedFolders
        const folders =
          route && !policyKnown
            ? {
                ...route,
                ready: false,
                message:
                  "Could not verify all saved upload routes. Refresh before choosing folders.",
              }
            : route && !this.routeBatch && status.policyRevision !== previous?.policyRevision
              ? {
                  ...route,
                  locked: lockedFolderRoutes(
                    this.snapshot.projects?.groups ?? [],
                    this.routePolicies,
                  ),
                }
              : undefined
        this.publish({
          ...(this.snapshot.routedPolicies !== null
            ? { routedPolicies: this.savedRouteStatuses() }
            : {}),
          ...(personal?.id === id ? { ongoing: status } : {}),
          ...(folders ? { routedFolders: folders } : {}),
        })
      }
      return
    }
    if (
      !identity ||
      !personal ||
      authStatus !== "signed_in" ||
      status.accountId !== identity.id ||
      status.destinationId !== personal.id
    )
      return
    this.ongoingRevision++
    if (cloudUploadStatusUnknown(status)) {
      this.invalidateContentConfirmation()
      this.publish({ ongoing: status })
      return
    }
    this.confirmedProjectChoices = status.projectChoices ?? {}
    this.needsChoiceReview =
      status.consentVersion === "desktop-project-sync-v1" &&
      Object.keys(this.confirmedProjectChoices).length === 0
    const projects = this.snapshot.projects
    if (projects?.loaded) {
      const choices = cloudProjectChoices(
        projects.groups,
        this.confirmedProjectChoices,
        this.projectChoices,
        !this.needsChoiceReview,
      )
      const selectionKey = this.projectSelectionKey(projects.groups, choices)
      if (selectionKey !== projects.selectionKey) this.invalidateContentConfirmation()
      this.publish({
        ongoing: status,
        projects: {
          ...projects,
          choices,
          selectionKey,
          needsChoiceReview: this.needsChoiceReview,
        },
      })
    } else this.publish({ ongoing: status })
  }
  private projectBinding(allowSetupError = false): CloudDestinationBinding | null {
    const { identity, destination, status, setupError } = this.snapshot.cloud
    const personal = personalCloudDestination(destination ?? null)
    return identity && personal && status === "signed_in" && (allowSetupError || !setupError)
      ? { accountId: identity.id, destinationId: personal.id }
      : null
  }
  /** Discovery uses one explicit authorized route, not a consent default. */
  private discoveryBinding(): CloudDestinationBinding | null {
    return this.snapshot.cloud.destinations === undefined
      ? this.projectBinding()
      : folderDiscoveryBinding(this.snapshot.cloud)
  }
  private savedRouteStatuses = () =>
    savedFolderPolicyStatuses(this.snapshot.cloud.savedDestinationIds, this.routePolicies)
  /** Require desktop project members and returned source roots together. This is
   * metadata support, not content consent; other-agent suggestions stay visible.
   */
  private supportedProjectSelections(group: DesktopCloudProjectGroup): CloudProjectSelection[] {
    return group.captureSupported
      ? group.projects.filter(
          (project) =>
            group.folderPaths.includes(project.projectPath) &&
            this.projectSources.some(
              (source) =>
                source.sourceKind === project.sourceKind && source.path === project.sourcePath,
            ),
        )
      : []
  }
  /** Use typed desktop selections only. Suggestions from other agents stay display-only. */
  private selectedProjects(
    choices: CloudProjectChoices = this.snapshot.projects?.choices ?? {},
  ): CloudProjectSelection[] {
    const selections = this.desktopProjects
      .filter((group) => choices[group.id] ?? true)
      .flatMap((group) => this.supportedProjectSelections(group))
    return [
      ...new Map(selections.map((project) => [JSON.stringify(project), project])).values(),
    ].sort(
      (a, b) =>
        a.sourceKind.localeCompare(b.sourceKind) ||
        a.sourcePath.localeCompare(b.sourcePath) ||
        a.projectPath.localeCompare(b.projectPath),
    )
  }
  /** Include provider/source bindings in confirmation, not just the visible folder names. */
  private projectSelectionKey(groups: CloudProjectGroup[], choices: CloudProjectChoices) {
    return JSON.stringify([
      cloudProjectSelectionKey(groups, choices),
      this.selectedProjects(choices),
    ])
  }
  /** Fence one Settings confirmation even while its group is excluded from Start. */
  private projectConfirmationKey(group: DesktopCloudProjectGroup) {
    return JSON.stringify([
      group.id,
      group.environment,
      group.path,
      group.groupingComplete,
      [...group.folderPaths].sort(),
      this.supportedProjectSelections(group)
        .map((project) =>
          JSON.stringify([project.sourceKind, project.sourcePath, project.projectPath]),
        )
        .sort(),
    ])
  }
  /** Read one bounded page without saving initial ON choices. Desktop code
   * owns source evidence and worktree grouping; failed pages never become empty success.
   */
  loadProjects = async (more = false) => {
    const port = this.port.ongoing
    const binding = this.discoveryBinding()
    const { cloud } = this.snapshot
    const routeAccount =
      cloud.destinations !== undefined && cloud.status === "signed_in"
        ? cloud.identity?.id
        : undefined
    if (
      !port ||
      (!binding && !routeAccount) ||
      this.snapshot.projects?.loading ||
      (more &&
        (!this.projectCursor ||
          binding?.destinationId !== this.projectReadBinding?.destinationId))
    )
      return
    const generation = ++this.projectGeneration
    const current = () =>
      this.active &&
      generation === this.projectGeneration &&
      JSON.stringify(this.discoveryBinding()) === JSON.stringify(binding)
    const existing = this.snapshot.projects
    this.publish({
      projects: {
        groups: existing?.groups ?? [],
        choices: existing?.choices ?? {},
        loading: true,
        loaded: existing?.loaded ?? false,
        partial: existing?.partial ?? false,
        hasMore: existing?.hasMore ?? false,
        needsChoiceReview: this.needsChoiceReview,
        error: null,
        selectionKey: existing?.selectionKey ?? "",
      },
    })
    try {
      if (!more) {
        if (routeAccount) {
          this.verifiedRouteIds = null
          const ids = this.snapshot.cloud.savedDestinationIds
          if (!ids) throw new Error("Saved Cloud destinations are unavailable.")
          const policies = await readSavedFolderPolicies(port, routeAccount, ids, current)
          if (!policies || !current()) return
          if (!sameSavedFolderInventory(ids, this.snapshot.cloud.savedDestinationIds)) {
            throw new Error("Saved Cloud destinations changed.")
          }
          this.routePolicies = policies
          this.verifiedRouteIds = ids
          this.publish({ routedPolicies: this.savedRouteStatuses() })
          const personal = personalCloudDestination(this.snapshot.cloud.destination ?? null)
          const currentPersonal = personal && policies.get(personal.id)
          if (currentPersonal) this.receiveOngoing(currentPersonal)
        } else {
          const revision = this.ongoingRevision
          const status = await port.status()
          if (!current()) return
          if (status && revision === this.ongoingRevision) this.receiveOngoing(status)
        }
      }
      if (!binding) {
        this.publish({ projects: { ...this.snapshot.projects!, loading: false } })
        return
      }
      const cursor = more ? this.projectCursor : null
      const page = await port.groups(binding, cursor)
      if (!current()) return
      if (
        routeAccount &&
        !sameSavedFolderInventory(
          this.verifiedRouteIds,
          this.snapshot.cloud.savedDestinationIds,
        )
      )
        throw new Error("Saved Cloud destinations changed.")
      if (page.nextCursor && JSON.stringify(page.nextCursor) === JSON.stringify(cursor))
        throw new Error("Project discovery repeated a page.")
      const groups = mergeCloudProjectGroups(more ? this.desktopProjects : [], page.groups)
      this.desktopProjects = groups
      this.projectReadBinding = binding
      this.projectSources = [
        ...new Map(
          [...(more ? this.projectSources : []), ...page.sourceRoots].map((source) => [
            JSON.stringify([source.sourceKind, source.path]),
            source,
          ]),
        ).values(),
      ]
      this.projectCursor = page.nextCursor
      const viewGroups = groups.map((group) => ({
        id: group.id,
        name: group.label,
        path: group.path,
        memberPaths: group.folderPaths,
        worktreeCount: group.worktreeCount,
        sessionCount: Math.max(group.sessionCount, group.captureSessionCount),
        captureSupported: this.supportedProjectSelections(group).length > 0,
        confirmationKey: this.projectConfirmationKey(group),
      }))
      const choices = cloudProjectChoices(
        viewGroups,
        this.confirmedProjectChoices,
        this.projectChoices,
        !this.needsChoiceReview,
      )
      const selectionKey = this.projectSelectionKey(viewGroups, choices)
      if (existing?.selectionKey !== selectionKey) this.invalidateContentConfirmation()
      this.publish({
        ...(this.snapshot.cloud.destinations !== undefined
          ? {
              routedFolders: {
                choices: this.routeChoices,
                locked: lockedFolderRoutes(viewGroups, this.routePolicies),
                ready: true,
                results: this.snapshot.routedFolders?.results ?? [],
                retry: false,
                message: null,
              },
            }
          : {}),
        projects: {
          groups: viewGroups,
          choices,
          loaded: true,
          loading: false,
          partial: page.partial || (more && (existing?.partial ?? false)),
          hasMore: page.nextCursor !== null,
          needsChoiceReview: this.needsChoiceReview,
          error: null,
          selectionKey,
        },
      })
    } catch (error) {
      if (current())
        this.publish({
          projects: {
            ...this.snapshot.projects!,
            loading: false,
            error: isCloudBusy(error)
              ? CLOUD_BUSY_MESSAGE
              : "Could not load project folders. Try again in Cloud Settings.",
          },
          ...(routeAccount && !this.verifiedRouteIds ? { routedPolicies: null } : {}),
        })
    }
  }
  /** Extend the visible draft only after an explicit page request. */
  loadMoreProjects = () => this.loadProjects(true)
  /** Change one local group choice. Start, not a checkbox or a default, saves consent. */
  selectProject = (id: string, selected: boolean) => {
    const projects = this.snapshot.projects
    if (!projects || projects.loading || !projects.groups.some((group) => group.id === id))
      return
    this.invalidateContentConfirmation()
    this.projectChoices = { ...this.projectChoices, [id]: selected }
    const choices = { ...projects.choices, [id]: selected }
    this.publish({
      projects: {
        ...projects,
        choices,
        selectionKey: this.projectSelectionKey(projects.groups, choices),
      },
    })
  }
  /** Record only a visible draft edit. Confirmed routes are not edited here. */
  selectFolderRoute = (id: string, destinationId: string | null) => {
    const { busy, projects, routedFolders, cloud } = this.snapshot
    if (
      busy ||
      projects?.loading ||
      !routedFolders?.ready ||
      this.routeBatch ||
      Object.hasOwn(routedFolders.locked, id) ||
      !projects?.groups.some((group) => group.id === id) ||
      (destinationId !== null &&
        !cloud.destinations?.some((entry) => entry.id === destinationId))
    )
      return
    this.invalidateContentConfirmation()
    this.routeChoices = { ...this.routeChoices, [id]: destinationId }
    this.publish({
      routedFolders: { ...routedFolders, choices: this.routeChoices, message: null },
    })
  }

  /** Validate the rendered choices against current membership and exact folder groups. */
  startFolderRoutes = async (
    context: CloudViewSnapshot,
    visibleAssignments: readonly FolderAssignment[],
    completeOnSave = true,
  ) => {
    const { cloud, busy, projects, routedFolders } = this.snapshot
    if (
      !this.active ||
      !this.port.ongoing ||
      busy ||
      this.syncInFlight ||
      this.routeBatch ||
      context.contextRevision !== this.consentRevision ||
      folderRoutingAuthority(context.cloud) !== folderRoutingAuthority(cloud) ||
      !cloud.identity ||
      !cloud.destinations ||
      !this.verifiedRouteIds ||
      !sameSavedFolderInventory(this.verifiedRouteIds, cloud.savedDestinationIds) ||
      !cloud.captureEnabled ||
      cloud.status !== "signed_in" ||
      this.snapshot.error ||
      cloud.setupError ||
      !projects?.loaded ||
      projects.loading ||
      projects.error ||
      !routedFolders?.ready
    )
      return
    const assigned = new Map<string, string[]>()
    const currentAssignments: FolderAssignment[] = []
    for (const group of projects.groups) {
      const destinationId = Object.hasOwn(this.routeChoices, group.id)
        ? this.routeChoices[group.id]
        : null
      if (!destinationId || Object.hasOwn(routedFolders.locked, group.id)) continue
      if (
        !group.captureSupported ||
        !cloud.destinations.some((entry) => entry.id === destinationId)
      )
        return
      currentAssignments.push({ folderId: group.id, destinationId })
      const ids = assigned.get(destinationId) ?? []
      ids.push(group.id)
      assigned.set(destinationId, ids)
    }
    if (
      !assigned.size ||
      JSON.stringify(visibleAssignments) !== JSON.stringify(currentAssignments)
    )
      return
    const entries = []
    for (const destination of cloud.destinations) {
      const folderIds = assigned.get(destination.id)
      if (!folderIds) continue
      const policy = this.routePolicies.get(destination.id)
      const selections = this.desktopProjects
        .filter((group) => folderIds.includes(group.id))
        .flatMap((group) => this.supportedProjectSelections(group))
      const consent = folderRouteConsent(
        cloud.identity.id,
        destination,
        policy,
        folderIds,
        selections,
      )
      if (!consent) {
        this.publish({
          routedFolders: {
            ...routedFolders,
            message: `${destination.name} needs a policy review in Settings before you add folders.`,
          },
        })
        return
      }
      entries.push({ consent, name: destination.name, saved: false })
    }
    if (folderRoutesOverlap(entries.map(({ consent }) => consent))) {
      this.publish({
        routedFolders: {
          ...routedFolders,
          message: "Overlapping folders must use the same destination.",
        },
      })
      return
    }
    if (this.analytics) noteCloudAction("folders", "start_sync")
    if (completeOnSave) this.routedScreen = "folders"
    this.routeBatch = {
      revision: this.consentRevision,
      authority: folderRoutingAuthority(cloud),
      completeOnSave,
      knownDestinationIds: new Set([
        ...this.verifiedRouteIds,
        ...entries.map(({ consent }) => consent.destinationId),
      ]),
      entries,
    }
    await this.retryFolderRoutes()
  }

  /** Check for a saved policy before retrying. Never resubmit confirmed destinations. */
  retryFolderRoutes = async () => {
    const port = this.port.ongoing
    const batch = this.routeBatch
    const view = this.snapshot.routedFolders
    if (!port || !batch || !view || this.syncInFlight || this.snapshot.busy) return
    const generation = this.generation
    const current = () =>
      this.active &&
      generation === this.generation &&
      this.routeBatch === batch &&
      this.verifiedRouteIds !== null &&
      this.consentRevision === batch.revision &&
      folderRoutingAuthority(this.snapshot.cloud) === batch.authority &&
      this.snapshot.cloud.savedDestinationIds?.every((id) =>
        batch.knownDestinationIds.has(id),
      ) === true
    if (!current()) return
    this.syncInFlight = true
    this.publish({ busy: true, routedFolders: { ...view, message: null } })
    try {
      for (const entry of batch.entries) {
        if (!current()) break
        if (entry.saved) continue
        const { consent } = entry
        const binding = { accountId: consent.accountId, destinationId: consent.destinationId }
        try {
          const prior = await port.status(binding)
          if (!current()) break
          if (
            !prior ||
            prior.accountId !== binding.accountId ||
            prior.destinationId !== binding.destinationId
          )
            continue
          if (matchesFolderConsent(prior, consent)) {
            entry.saved = true
            this.routePolicies.set(binding.destinationId, prior)
            continue
          }
          if (
            cloudUploadStatusUnknown(prior) ||
            prior.paused ||
            prior.requiresDestinationReview ||
            (prior.consentVersion ? prior.policyRevision : undefined) !==
              consent.expectedPolicyRevision
          )
            continue
          const status = await port.enable(consent)
          if (!current()) break
          if (matchesFolderConsent(status, consent)) {
            entry.saved = true
            this.routePolicies.set(binding.destinationId, status)
          }
        } catch {
          // A failed reply does not prove the policy was not saved.
        }
      }
    } finally {
      this.syncInFlight = false
      if (this.active && generation === this.generation) {
        if (current()) {
          const complete = batch.entries.every((entry) => entry.saved)
          const locked = lockedFolderRoutes(
            this.snapshot.projects?.groups ?? [],
            this.routePolicies,
          )
          if (complete && batch.completeOnSave) this.routedScreen = "complete"
          this.publish({
            busy: false,
            routedPolicies: this.savedRouteStatuses(),
            ...(complete && batch.completeOnSave ? { screen: "complete" } : {}),
            routedFolders: {
              ...view,
              locked,
              results: folderRouteResults(batch.entries),
              retry: !complete,
            },
          })
          if (complete) {
            this.routeBatch = null
            if (!batch.completeOnSave) void this.loadProjects()
          }
        } else {
          const changedView = this.snapshot.routedFolders
          const changed =
            this.snapshot.cloud.identity?.id === batch.entries[0]?.consent.accountId
          this.publish({
            busy: false,
            ...(changed && changedView
              ? {
                  routedFolders: {
                    ...changedView,
                    results: folderRouteResults(batch.entries),
                  },
                }
              : {}),
          })
          this.routeBatch = null
          if (this.snapshot.cloud.identity) void this.loadProjects()
        }
      }
    }
  }
  /** Discard only the frozen request. Reload confirmed rules before another Start. */
  reviewFolderRoutes = async () => {
    if (this.syncInFlight || this.snapshot.busy || this.snapshot.projects?.loading) return
    this.routeBatch = null
    this.invalidateContentConfirmation()
    const view = this.snapshot.routedFolders
    if (view) this.publish({ routedFolders: { ...view, ready: false, retry: false } })
    await this.loadProjects()
  }

  /** Leave this draft without changing any rule the desktop already saved. */
  dismissFolderRoutes = () => {
    if (this.snapshot.busy || this.syncInFlight) return
    this.routeChoices = {}
    this.routeBatch = null
    this.invalidateContentConfirmation()
    const view = this.snapshot.routedFolders
    if (view)
      this.publish({
        routedFolders: { ...view, choices: {}, results: [], retry: false, message: null },
      })
  }
  /** Show saved local grants above or inside a discovered folder. */
  projectHasSavedPermission = (id: string) =>
    folderGroupHasPolicy(
      this.desktopProjects.find((item) => item.id === id),
      this.personalPolicy(this.snapshot),
    )
  private personalPolicy = ({ cloud, ongoing, routedPolicies }: CloudViewSnapshot) =>
    cloud.destinations === undefined
      ? ongoing
      : savedPersonalFolderPolicy(cloud, routedPolicies)
  /** Save one Settings switch. The caller confirms additions before invoking this. */
  setSavedProject = async (context: CloudViewSnapshot, id: string, selected: boolean) => {
    const { cloud } = this.snapshot
    const routed = cloud.destinations !== undefined
    if (selected && routed) return
    const port = this.port.ongoing
    const prior = this.personalPolicy(context)
    const policy = this.personalPolicy(this.snapshot)
    const binding = routed
      ? personalFolderRemovalBinding(cloud, policy, prior, this.projectReadBinding)
      : this.projectBinding(!selected)
    const group = this.desktopProjects.find((item) => item.id === id)
    const policyRevision = prior?.policyRevision
    if (
      !this.active ||
      !port ||
      !binding ||
      !group ||
      !policyRevision ||
      policyRevision !== policy?.policyRevision ||
      this.snapshot.busy ||
      this.syncInFlight ||
      this.snapshot.projects?.loading ||
      policy?.consentVersion !== "desktop-project-sync-v1" ||
      !policy.storageKnown ||
      context.contextRevision !== this.consentRevision ||
      (!routed && confirmationContextKey(context) !== confirmationContextKey(this.snapshot))
    )
      return
    if (
      (selected || routed) &&
      context.projects?.groups.find((item) => item.id === id)?.confirmationKey !==
        this.projectConfirmationKey(group)
    )
      return
    const projects = selected ? this.supportedProjectSelections(group) : []
    if (selected && !projects.length) return
    const generation = this.generation
    const revision = this.ongoingRevision
    this.publish({ busy: true, error: null })
    try {
      const status = await port.setProject({
        ...binding,
        groupId: id,
        selected,
        projects,
        folderPaths: selected
          ? group.folderPaths
          : [...new Set([group.path, ...group.folderPaths])],
        expectedPolicyRevision: policyRevision,
      })
      if (this.active && generation === this.generation && revision === this.ongoingRevision)
        this.receiveOngoing(status)
    } catch {
      if (
        this.active &&
        generation === this.generation &&
        this.snapshot.cloud.identity?.id === binding.accountId &&
        (routed || this.snapshot.cloud.destination?.id === binding.destinationId)
      )
        this.publish({ error: "Could not confirm the folder change. Try the switch again." })
    } finally {
      if (this.active && generation === this.generation) this.publish({ busy: false })
    }
  }
  /** Pause or retry the saved policy without expanding its sources or project paths. */
  controlSync = async (paused: boolean, destinationId?: string) => {
    const port = this.port.ongoing
    const { cloud, routedPolicies } = this.snapshot
    const binding =
      cloud.destinations === undefined
        ? this.projectBinding(true)
        : folderRouteControlBinding(cloud, routedPolicies, destinationId, paused)
    if (!port || !binding || this.snapshot.busy) return
    const generation = this.generation
    const revision = this.ongoingRevision
    this.publish({ busy: true, error: null })
    try {
      const status = await (paused ? port.pause(binding) : port.retry(binding))
      if (this.active && generation === this.generation && revision === this.ongoingRevision)
        this.receiveOngoing(status)
    } catch (error) {
      if (this.active && generation === this.generation)
        this.publish({
          error: isCloudBusy(error)
            ? CLOUD_BUSY_MESSAGE
            : "Could not change Cloud uploads. Try again.",
        })
    } finally {
      if (this.active && generation === this.generation) this.publish({ busy: false })
    }
  }
  /** Serialize surface actions and reject replies superseded by desktop events. */
  private async run(
    action: Parameters<typeof noteCloudAction>[1] | null,
    task: () => Promise<CloudSnapshot>,
  ) {
    if (this.snapshot.busy) return false
    if (this.analytics && action) noteCloudAction(this.snapshot.screen, action)
    const generation = this.generation
    this.publish({
      busy: true,
      authAction: action === "sign_in" || action === "sign_out" ? action : null,
      error: null,
    })
    const revision = this.revision
    try {
      const value = await task()
      if (!this.active || generation !== this.generation) return false
      const fresh = revision === this.revision
      if (fresh) this.receive(value)
      this.publish({ busy: false, authAction: null })
      const consistent = fresh || JSON.stringify(this.snapshot.cloud) === JSON.stringify(value)
      return consistent && !["credential_failed", "sign_out_failed"].includes(value.status)
    } catch (error) {
      if (this.active && generation === this.generation)
        this.publish({
          busy: false,
          authAction: null,
          error: isCloudBusy(error)
            ? CLOUD_BUSY_MESSAGE
            : action === "sign_out"
              ? messages.sign_out_failed
              : "Cloud could not complete that action. Try again.",
        })
      return false
    }
  }
  /** Start explicit browser approval; authentication alone never starts capture. */
  begin = () =>
    this.run(this.snapshot.cloud.authEnabled ? "sign_in" : "learn_more", this.port.begin)
  /** Record explicit browser cancellation without cancelling saved source requests. */
  cancel = () => this.run("cancel", this.port.cancel)
  /** Cancel while navigating away without adding a second action event. */
  cancelSilently = () => this.run(null, this.port.cancel)
  /** Ask desktop code to close this account generation and revoke only its grant. */
  signOut = () => {
    this.invalidateContentConfirmation()
    return this.run("sign_out", this.port.signOut)
  }
  /** Fence in-flight work when its account, sources, or selected folders change. */
  private invalidateContentConfirmation() {
    this.consentRevision++
  }
  /** Bind every preference write to the currently observed personal destination. */
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
  /** Persist nested navigation only when desktop account/setup state permits it. */
  go = async (screen: CloudDraft["screen"], action: "back" | "continue" = "continue") => {
    if (screen !== "handoff" && !this.snapshot.cloud.identity) return
    if (this.port.ongoing && this.snapshot.cloud.destinations !== undefined) {
      if (this.snapshot.busy) return
      if (this.analytics) noteCloudAction(this.snapshot.screen, action)
      this.routedScreen = screen
      this.publish({ screen })
      if (screen === "folders" && !this.snapshot.projects?.loaded) await this.loadProjects()
      return
    }
    await this.run(action, () =>
      this.saveDraft({ ...this.snapshot.cloud.draft, screen }, false),
    )
    if (screen === "folders" && this.port.ongoing && !this.snapshot.projects?.loaded)
      await this.loadProjects()
  }
  /** Discard picker results after account changes and clear consent before source edits. */
  addFolders = async (sourceKind: CloudSourceKind = "claude_jsonl") => {
    const generation = this.generation
    const account = this.snapshot.cloud.identity?.id
    const destination = this.snapshot.cloud.destination?.id
    await this.run("add_folder", async () => {
      const paths = await this.port.pick(sourceKind)
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
        const existing = folders.find(
          (folder) => folder.path === path && cloudSourceKind(folder) === sourceKind,
        )
        if (existing && !existing.selected) {
          existing.selected = true
          changed = true
        } else if (!existing && folders.length < 100) {
          folders.push({
            path,
            selected: true,
            ...(sourceKind === "opencode_sqlite_v2" ? { sourceKind } : {}),
          })
          changed = true
        }
      }
      if (!changed) return this.snapshot.cloud
      this.invalidateContentConfirmation()
      return this.saveDraft({ ...this.snapshot.cloud.draft, folders })
    })
    if (this.port.ongoing) await this.loadProjects()
  }
  /** Clear consent immediately, before a delayed preference save can complete. */
  selectFolder = (
    path: string,
    selected: boolean,
    sourceKind: CloudSourceKind = "claude_jsonl",
  ) => {
    this.invalidateContentConfirmation()
    return this.run("select_folder", () =>
      this.saveDraft({
        ...this.snapshot.cloud.draft,
        folders: this.snapshot.cloud.draft.folders.map((folder) =>
          folder.path === path && cloudSourceKind(folder) === sourceKind
            ? { ...folder, selected }
            : folder,
        ),
      }),
    )
  }
  /** Remove a draft choice, not any saved request, source, or spool entry. */
  removeFolder = (path: string, sourceKind: CloudSourceKind = "claude_jsonl") => {
    this.invalidateContentConfirmation()
    return this.run(null, () =>
      this.saveDraft({
        ...this.snapshot.cloud.draft,
        folders: this.snapshot.cloud.draft.folders.filter(
          (folder) => folder.path !== path || cloudSourceKind(folder) !== sourceKind,
        ),
      }),
    )
  }
  /** Handle an explicit Start or Update click for the rendered context. Loading
   * rows or changing a selection never calls this method. Retry keeps saved IDs.
   */
  startSync = async (context: CloudViewSnapshot = this.snapshot) => {
    if (
      !this.active ||
      context.contextRevision !== this.consentRevision ||
      confirmationContextKey(context) !== confirmationContextKey(this.snapshot)
    )
      return
    if (this.port.ongoing) {
      if (this.snapshot.cloud.destinations !== undefined) return
      await this.startProjectSync()
      return
    }
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
    const desktopCapture = this.snapshot.cloud.captureEnabled === true
    const destinationId = destination.id
    const consent: CaptureConsent = {
      accountId: account,
      destinationId,
      fullContent: true as const,
      attachments: false as const,
      ...(selected.some((folder) => cloudSourceKind(folder) === "opencode_sqlite_v2")
        ? {
            version: "native-full-content-v2" as const,
            selectedSources: selected
              .map((folder) => ({
                sourceKind: cloudSourceKind(folder),
                path: folder.path,
              }))
              .sort(
                (a, b) =>
                  a.sourceKind.localeCompare(b.sourceKind) || a.path.localeCompare(b.path),
              ),
          }
        : {
            version: "native-full-content-v1" as const,
            selectedRoots: selected.map((folder) => folder.path).sort(),
          }),
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
      consentRevision === this.consentRevision &&
      !(
        this.snapshot.cloud.pendingSync?.requestId === requestId &&
        this.snapshot.cloud.pendingSync.result.status === "cancelled"
      )
    this.syncInFlight = true
    const starting: SyncResult = { status: "starting" }
    this.publish({
      screen: "complete",
      busy: false,
      sync: starting,
      error: null,
      sourceListingNeedsRetry: false,
    })
    try {
      if (!existing) {
        await this.port.prepareSync(selected, requestId, consent)
        if (!current()) return
        this.preparedRequestId = requestId
      }
      if (!current()) return
      const sync =
        existing && !desktopCapture
          ? await this.port.reconcile(requestId)
          : await this.port.start(selected, requestId, consent)
      if (current() && (desktopCapture || revision === this.revision))
        this.publish({ sync, busy: false, screen: "complete" })
    } catch (error) {
      if (current() && (desktopCapture || revision === this.revision)) {
        const saved = desktopCapture && this.preparedRequestId === requestId
        const busy = isCloudBusy(error)
        this.publish({
          sync: { status: saved ? "upload_unavailable" : busy ? "unavailable" : "ambiguous" },
          busy: false,
          screen: "complete",
          sourceListingNeedsRetry: saved,
          ...(busy
            ? { error: CLOUD_BUSY_MESSAGE }
            : saved
              ? {
                  error:
                    "Could not finish listing sources. Your request is saved. Retry in Settings > Sessions.",
                }
              : {}),
        })
      }
    } finally {
      this.syncInFlight = false
      // Clear only this operation's progress, not state from a newer desktop event.
      if (this.active && generation === this.generation && this.snapshot.sync === starting)
        this.publish({ sync: { status: "unavailable" } })
    }
  }
  /** Commit the exact current group members through desktop ongoing consent. The
   * completion screen stays available while desktop acceptance is still unknown.
   */
  private async startProjectSync() {
    const port = this.port.ongoing
    const binding = this.projectBinding()
    const projects = this.snapshot.projects
    const selected = this.selectedProjects()
    if (
      !port ||
      !binding ||
      this.snapshot.busy ||
      this.syncInFlight ||
      cloudUploadStatusUnknown(this.snapshot.ongoing) ||
      !this.snapshot.cloud.captureEnabled ||
      !projects?.loaded ||
      projects.loading ||
      projects.error ||
      !selected.length
    )
      return
    const selectedSources = this.projectSources.filter((source) =>
      selected.some(
        (project) =>
          project.sourceKind === source.sourceKind && project.sourcePath === source.path,
      ),
    )
    const generation = this.generation
    const consentRevision = this.consentRevision
    const revision = this.ongoingRevision
    if (this.analytics) noteCloudAction(this.snapshot.screen, "start_sync")
    this.syncInFlight = true
    this.projectSyncStarted = true
    const starting: SyncResult = { status: "starting" }
    this.publish({ screen: "complete", sync: starting, error: null })
    try {
      const status = await port.enable({
        ...binding,
        version: "desktop-project-sync-v1",
        selectedSources,
        projects: selected,
        projectChoices: { ...this.confirmedProjectChoices, ...projects.choices },
        fullContent: true,
        ongoing: true,
        attachments: false,
      })
      if (
        this.active &&
        generation === this.generation &&
        consentRevision === this.consentRevision
      ) {
        if (revision === this.ongoingRevision) this.receiveOngoing(status)
        this.projectChoices = {}
        this.publish({ sync: { status: "idle" }, screen: "complete" })
      }
    } catch (error) {
      if (
        this.active &&
        generation === this.generation &&
        consentRevision === this.consentRevision
      )
        this.publish({
          sync: { status: "ambiguous" },
          error: isCloudBusy(error)
            ? CLOUD_BUSY_MESSAGE
            : "Cloud sync was not confirmed. Check Cloud Settings to retry.",
        })
    } finally {
      this.syncInFlight = false
      if (this.active && generation === this.generation && this.snapshot.sync === starting)
        this.publish({ sync: { status: "unavailable" } })
    }
  }
}
