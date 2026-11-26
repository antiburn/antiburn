import {
  emptyCloudSnapshot,
  CLOUD_BUSY_MESSAGE,
  desktopCloudPort,
  cloudProjectChoices,
  type CloudAccessUpdate,
  type CloudDraft,
  type CloudPort,
  type CloudScreen,
  type CloudSnapshot,
  type SyncResult,
  type CloudProjectGroup,
  type CloudProjectChoices,
  type DesktopCloudProjectGroup,
  type CloudProjectSelection,
  type CloudOngoingStatus,
  type CloudDestinationBinding,
  type CloudWorkspace,
  type CloudWorkspaceRecovery,
} from "../lib/cloud"
import { noteCloudAction, noteCloudScreenViewed } from "../lib/onboardingAnalytics"
import { personalCloudDestination } from "../lib/cloudDestination"
import { advanceFolderMoveScope, createFolderMove } from "./cloudFolderMove"
import {
  isCurrentProjectFolderRoute,
  projectFolderRoutes,
  projectSyncConsent,
} from "./cloudProjectRoutes"
import type { ProjectPages } from "./cloudProjectPages"
import { CloudProjectReader } from "./cloudProjectReader"
import { cloudReadScopeMessage, retryCloudRead } from "./cloudReadRetry"
import {
  cloudFolderReadChanged,
  savedUploadStatusBecameUnavailable,
  shouldRecheckCloudFolders,
} from "./cloudProjectScope"
import {
  accountViewPatch,
  backfillTarget,
  cloudContextChange,
  cloudUploadStatusUnknown,
  confirmationContextKey,
  confirmsSavedWrite,
  expectedFolderRoute,
  checkOnboardingFolderRoutes,
  type RequestedFolderRoute,
  folderSaveError,
  hasSavedProjectPermission,
  isCloudBusy,
  matchesUploadPolicy,
  ongoingDestination,
  projectBinding,
  projectConfirmationKey,
  projectSelectionKey,
  projectSyncChoices,
  retryTarget,
  reviewableProjectSelections,
  selectedProjects,
  supportedProjectSelections,
  teamFoldersToConfirm,
  teamStatusPatch,
  uploadPolicy,
  workspaceRecoveryPatch,
  type CloudViewSnapshot,
} from "./cloudSessionState"
import { createSyncControl } from "./cloudSyncControl"
import { createTeamFolderConfirmation } from "./cloudTeamFolders"
export {
  canBackfillUploads,
  canRetryPersonalUploads,
  cloudOngoingStatusMessage,
  cloudStatusMessage,
  cloudUploadStatusUnknown,
  teamFoldersToConfirm,
  type CloudViewSnapshot,
} from "./cloudSessionState"

/** Use the same browser sign-in label in onboarding and Settings. */
export const cloudGithubSignInLabel = "Sign in with GitHub"

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
  }
  private listeners = new Set<() => void>()
  private stop: (() => void) | undefined
  private stopOngoing: (() => void) | undefined
  private stopWorkspace: (() => void) | undefined
  private stopLocalTools: (() => void) | undefined
  private localToolsLoading = false
  private localToolsEventGeneration = 0
  private localToolsStatusRevision = -1
  private accountRead: AbortController | undefined
  private workspaceRead: AbortController | undefined
  private projectPages: ProjectPages | undefined
  private readonly projectReader: CloudProjectReader
  private projectChoices: CloudProjectChoices = {}
  private projectTargets: Record<string, string> = {}
  private confirmedProjectChoices: CloudProjectChoices = {}
  private needsChoiceReview = false
  private ongoingRevision = 0
  private projectSyncState: "pending" | "started" | undefined
  private onboardingSavedRoutes: RequestedFolderRoute[] = []
  private timer: ReturnType<typeof setTimeout> | undefined
  private identityTimer = false
  private generation = 0
  private revision = 0
  private active = false
  private visible = true
  private viewed: CloudScreen | null = null
  private syncInFlight = false
  private consentRevision = 0
  private folderMoveScope = { authority: 0, account: 0 }
  private readonly port: CloudPort
  private readonly analytics: boolean
  private readonly holdIdentity: boolean
  /** Keep onboarding paging manual. Settings uses automatic paging. */
  constructor(
    port: CloudPort = desktopCloudPort,
    analytics = true,
    holdIdentity = false,
    projectPaging: "manual" | "automatic" = "manual",
  ) {
    this.port = port
    this.analytics = analytics
    this.holdIdentity = holdIdentity
    this.projectReader = new CloudProjectReader(port, {
      automaticPaging: projectPaging === "automatic",
      getSnapshot: this.getSnapshot,
      accountRevision: () => this.revision,
      policyRevision: () => this.ongoingRevision,
      receiveAccount: (cloud) => this.receive(cloud),
      receivePolicy: (status) => this.receiveOngoing(status),
      receiveWorkspace: (recovery) => this.receiveWorkspaceRecovery(recovery),
      isActive: () => this.active,
      isVisible: () => this.visible,
      getPages: () => this.projectPages,
      needsChoiceReview: () => this.needsChoiceReview,
      publishProjects: (projects) => {
        if (projects.scopeInvalid && !this.snapshot.projects?.scopeInvalid)
          this.invalidateContentConfirmation()
        this.publish({ projects })
      },
      publishGroups: (pages, workspaces, state) =>
        this.publishProjectGroups(pages, workspaces, state),
    })
  }
  /** Return the stable snapshot expected by useSyncExternalStore. */
  getSnapshot = () => this.snapshot
  /** Observe snapshots without starting reads. The host owns the active subscription. */
  subscribeSnapshot = (listener: () => void) => {
    this.snapshotListeners.add(listener)
    return () => {
      this.snapshotListeners.delete(listener)
    }
  }
  private snapshotListeners = new Set<() => void>()

  /** Start observation for the first reader; the last unsubscribe fences late replies. */
  subscribe = (listener: () => void) => {
    this.listeners.add(listener)
    if (!this.active) {
      this.active = true
      document.addEventListener("visibilitychange", this.visibility)
      window.addEventListener("focus", this.refreshIncompleteProjects)
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
        this.stopWorkspace?.()
        this.stopWorkspace = undefined
        this.stopLocalTools?.()
        this.stopLocalTools = undefined
        this.localToolsLoading = false
        this.projectReader.abort()
        this.accountRead?.abort()
        this.workspaceRead?.abort()
        clearTimeout(this.timer)
        document.removeEventListener("visibilitychange", this.visibility)
        window.removeEventListener("focus", this.refreshIncompleteProjects)
        this.viewed = null
      }
    }
  }
  /** Suspend timers and screen analytics while the host surface is hidden. */
  setVisible = (visible: boolean) => {
    if (this.visible === visible) return
    this.visible = visible
    if (!visible && this.projectReader.hasRead) this.projectReader.suspend()
    else if (visible) this.refreshIncompleteProjects()
    this.visibility()
  }
  private visibility = () => {
    if (!this.visible || document.hidden) this.viewed = null
    this.schedule()
    this.recordView()
  }
  private refreshIncompleteProjects = () => {
    const { busy, projects } = this.snapshot
    if (!this.active || !this.visible || busy) return
    if (this.projectReader.queueFailedPageRecheck()) return
    if (
      !this.projectReader.hasRead &&
      (this.projectReader.hasPendingAutomaticPages ||
        shouldRecheckCloudFolders(this.snapshot) ||
        !!projects?.refreshError ||
        (!!projects && !projects.loaded && !projects.scopeInvalid))
    )
      void this.loadProjects(this.projectReader.hasPendingAutomaticPages)
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
    const wasBusy = this.snapshot.busy
    this.snapshot = { ...this.snapshot, ...patch, contextRevision: this.consentRevision }
    this.listeners.forEach((listener) => listener())
    this.snapshotListeners.forEach((listener) => listener())
    this.recordView()
    this.schedule()
    // A cancelled folder request can release its worker before the save finishes.
    if (wasBusy && !this.snapshot.busy) void this.projectReader.resumeAfterWrite()
  }
  /** Desktop state owns bindings. Clear consent on authority/root changes and let
   * completed desktop work clear a transient admission warning, not a real failure.
   * Setup failures fence pending discovery and keep cached choices available.
   */
  private receive(value: CloudSnapshot) {
    this.revision++
    const previous = this.snapshot.cloud
    const { resetProjects, contextChanged, sourcesChanged } = cloudContextChange(
      previous,
      value,
    )
    const setupFailed = !!value.setupError && !this.snapshot.cloud.setupError
    if (resetProjects || sourcesChanged || setupFailed) this.projectReader.abort()
    if (resetProjects || sourcesChanged) this.onboardingSavedRoutes = []
    if (resetProjects) {
      if (this.snapshot.cloud.identity) this.projectReader.forgetCachedPages()
      this.projectPages = undefined
      this.projectChoices = {}
      this.projectTargets = {}
      this.confirmedProjectChoices = {}
      this.needsChoiceReview = false
      this.projectSyncState = undefined
    }
    const patch = accountViewPatch(this.snapshot, value, {
      resetProjects,
      contextChanged,
      projectSyncState: this.projectSyncState,
    })
    if (!resetProjects && (sourcesChanged || setupFailed) && this.snapshot.projects)
      patch.projects = {
        ...this.snapshot.projects,
        loaded: sourcesChanged ? false : this.snapshot.projects.loaded,
        loading: false,
        retrying: false,
        retryStage: undefined,
        paging: undefined,
        hasMore: sourcesChanged ? false : this.snapshot.projects.hasMore,
      }
    if (contextChanged) {
      this.folderMoveScope = advanceFolderMoveScope(this.folderMoveScope, previous, value)
      this.consentRevision++
    }
    this.publish(patch)
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
      !this.canOpenSetup() &&
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
    const previousError = this.snapshot.error
    const current = () =>
      this.active && generation === this.generation && revision === this.revision
    try {
      const value = await this.port.read()
      if (current()) this.receive(value)
    } catch (error) {
      if (current() && !this.snapshot.busy && this.snapshot.error === previousError)
        this.publish({
          accountReadFailed: true,
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
  /** Attach events before independent account and tool reads. */
  load = async () => {
    const generation = this.generation
    this.accountRead?.abort()
    const read = (this.accountRead = new AbortController())
    const { signal } = read
    const currentLoad = () =>
      this.active && generation === this.generation && this.accountRead === read
    const accountTimeout = new Error("Cloud account read timed out.")
    const timeout = setTimeout(() => read.abort(accountTimeout), 30_000)
    this.publish({ busy: true, canPauseBackfill: false, error: null, accountReadFailed: false })
    void this.loadLocalTools()
    let readRevision: number | undefined
    let accountReadFinished = false
    try {
      if (!this.stop) {
        const stop = await this.port.watch((value) => {
          if (this.active && generation === this.generation) {
            const previous = this.snapshot.cloud
            // A recovery event cancels backoff even while an account read is pending.
            if (previous.setupError && !value.setupError) this.projectReader.abort()
            this.receive(value)
            if (cloudFolderReadChanged(previous, this.snapshot)) void this.loadProjects()
          }
        })
        if (!currentLoad()) {
          stop()
          return
        }
        this.stop = stop
      }
      if (!this.stopOngoing) {
        const stop = await this.port.ongoing.watch((status) => {
          if (this.active && generation === this.generation) this.receiveOngoing(status, true)
        })
        if (!currentLoad()) {
          stop()
          return
        }
        this.stopOngoing = stop
      }
      if (this.port.watchWorkspace && !this.stopWorkspace) {
        const stop = await this.port.watchWorkspace((connection) => {
          if (this.active && generation === this.generation) this.receiveWorkspace(connection)
        })
        if (!currentLoad()) {
          stop()
          return
        }
        this.stopWorkspace = stop
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
      readRevision = this.revision
      const value = await retryCloudRead(
        () => this.projectReader.request(this.port.read, signal),
        signal,
        (value) => value.status === "signed_in" && !value.destination,
      )
      if (signal.aborted || !currentLoad()) return
      clearTimeout(timeout)
      accountReadFinished = true
      if (readRevision === this.revision) this.receive(value)
      this.publish({ busy: false })
      if (this.snapshot.cloud.identity) await this.loadProjects()
    } catch (error) {
      if ((signal.aborted && signal.reason !== accountTimeout) || !currentLoad()) return
      if (readRevision !== undefined && readRevision !== this.revision) {
        this.publish({ busy: false })
        if (this.snapshot.cloud.status === "signed_in") await this.loadProjects()
        return
      }
      this.publish({
        busy: false,
        loaded: true,
        accountReadFailed: !accountReadFinished,
        error: isCloudBusy(error)
          ? CLOUD_BUSY_MESSAGE
          : "Could not load Cloud. Try again or continue locally.",
      })
    } finally {
      clearTimeout(timeout)
      if (this.accountRead === read) this.accountRead = undefined
    }
  }
  /** Apply legacy cleanup only to the active account. It does not change consent. */
  private receiveWorkspace(recovery: CloudWorkspaceRecovery) {
    const { accountId, workspaceId } = recovery
    const binding = this.projectBinding(true)
    if (binding?.accountId !== accountId || binding.destinationId === workspaceId) return
    this.receiveWorkspaceRecovery(recovery)
  }
  /** Merge recovery from a status read or a newer cleanup event. */
  private receiveWorkspaceRecovery(recovery: CloudWorkspaceRecovery) {
    const patch = workspaceRecoveryPatch(this.snapshot, recovery)
    if (patch) this.publish(patch)
  }
  /** Accept only the observed account's policy. No status event grants new consent. */
  private receiveOngoing(status: CloudOngoingStatus, includeTeams = false) {
    const destination = ongoingDestination(this.snapshot.cloud, status)
    const device =
      status.deviceUploadEnabled === undefined
        ? {}
        : { deviceUploadEnabled: status.deviceUploadEnabled }
    if (destination === "device") {
      if (status.deviceUploadEnabled !== undefined) {
        this.ongoingRevision++
        this.publish(device)
      }
      return
    }
    const recheck = includeTeams && savedUploadStatusBecameUnavailable(this.snapshot, status)
    if (destination === "team" && status.destinationId) {
      this.publish({
        ...device,
        ...teamStatusPatch(this.snapshot, status.destinationId, status),
      })
      if (recheck) void this.loadProjects()
      return
    }
    if (destination !== "personal") return
    this.ongoingRevision++
    if (includeTeams) {
      for (const team of status.teamStatuses ?? []) this.receiveOngoing(team)
    }
    if (cloudUploadStatusUnknown(status)) {
      this.invalidateContentConfirmation()
      this.publish({ ...device, ongoing: status })
      if (recheck) void this.loadProjects()
      return
    }
    this.confirmedProjectChoices = status.projectChoices ?? {}
    this.needsChoiceReview =
      status.consentVersion === "desktop-project-sync-v1" &&
      Object.keys(this.confirmedProjectChoices).length === 0
    const projects = this.snapshot.projects
    if (projects?.loaded && !projects.loading) {
      const choices = cloudProjectChoices(
        projects.groups,
        this.confirmedProjectChoices,
        this.projectChoices,
        this.defaultProjectSelection(projects.workspaces),
      )
      const selectionKey = this.projectSelectionKey(projects.groups, choices)
      if (selectionKey !== projects.selectionKey) this.invalidateContentConfirmation()
      this.publish({
        ...device,
        ongoing: status,
        projects: {
          ...projects,
          choices,
          selectionKey,
          needsChoiceReview: this.needsChoiceReview,
        },
      })
    } else this.publish({ ...device, ongoing: status })
  }
  private projectBinding(allowSetupError = false): CloudDestinationBinding | null {
    return projectBinding(this.snapshot.cloud, allowSetupError)
  }
  /** Keep destination choices explicit when a team policy already exists. */
  private defaultProjectSelection(workspaces?: readonly CloudWorkspace[]) {
    return (
      !this.needsChoiceReview &&
      (workspaces?.length ?? 0) <= 1 &&
      Object.keys(this.snapshot.teamStatuses ?? {}).length === 0
    )
  }
  private supportedProjectSelections(group: DesktopCloudProjectGroup): CloudProjectSelection[] {
    return supportedProjectSelections(group, this.projectPages?.sources ?? [])
  }
  private selectedProjects(
    choices: CloudProjectChoices = this.snapshot.projects?.choices ?? {},
  ): CloudProjectSelection[] {
    return selectedProjects(
      this.projectPages?.groups ?? [],
      this.projectPages?.sources ?? [],
      choices,
    )
  }
  private projectSelectionKey(
    groups: CloudProjectGroup[],
    choices: CloudProjectChoices,
    workspaces = this.snapshot.projects?.workspaces,
  ) {
    return projectSelectionKey(
      groups,
      choices,
      this.selectedProjects(choices),
      this.projectTargets,
      workspaces,
    )
  }
  private projectConfirmationKey(group: DesktopCloudProjectGroup) {
    return projectConfirmationKey(group, this.supportedProjectSelections(group))
  }
  /** Display account-bound folder evidence. Desktop checks additions before saving. */
  private publishProjectGroups(
    pages: ProjectPages,
    workspaces: readonly CloudWorkspace[] | undefined,
    state?: "provisional" | "refreshing" | "paging",
  ) {
    const provisional = state === "provisional"
    if (!provisional) this.projectPages = pages
    const { groups, hasMore } = this.projectReader.viewProjects(pages, provisional)
    const choices = provisional
      ? Object.fromEntries(groups.map(({ id }) => [id, false]))
      : cloudProjectChoices(
          groups,
          this.confirmedProjectChoices,
          this.projectChoices,
          this.defaultProjectSelection(workspaces),
        )
    const selectionKey = provisional
      ? ""
      : this.projectSelectionKey(groups, choices, workspaces ?? [])
    if (this.snapshot.projects?.selectionKey !== selectionKey)
      this.invalidateContentConfirmation()
    this.publish({
      projects: {
        groups,
        choices,
        workspaces,
        targets: provisional ? undefined : this.projectTargets,
        loaded: !provisional,
        loading: provisional,
        retrying: state === "refreshing",
        paging:
          state === "paging" || (state === "refreshing" && hasMore) ? "pending" : undefined,
        partial: pages.partial,
        hasMore: state === "paging" || hasMore,
        scopeInvalid: provisional && !!this.snapshot.projects?.scopeInvalid,
        needsChoiceReview: this.needsChoiceReview,
        error: null,
        selectionKey,
      },
    })
  }
  /** Read folder evidence without saving draft choices. */
  loadProjects = (more = false, retryInBackground = true) =>
    this.projectReader.load(more, retryInBackground)
  /** Check folder access without blocking the rest of the account pane. */
  retryFolderScope = () => this.projectReader.retryScope()
  /** Extend the visible draft only after an explicit page request. */
  loadMoreProjects = () => this.loadProjects(true)
  /** Stage a local group choice for the current account. Only Start saves consent. */
  selectProject = (id: string, selected: boolean, target?: string, settings = false) => {
    const projects = this.snapshot.projects
    const group = projects?.groups.find((item) => item.id === id)
    const binding = this.projectBinding()
    if (
      !binding ||
      !projects ||
      !this.projectReader.canUseChoices ||
      this.snapshot.busy ||
      !group ||
      (selected &&
        (!group.captureSupported ||
          this.hasSavedProject(id, settings) ||
          (target &&
            target !== binding.destinationId &&
            !projects.workspaces?.some(({ id, kind }) => id === target && kind === "team"))))
    )
      return
    this.invalidateContentConfirmation()
    this.projectTargets = { ...this.projectTargets }
    delete this.projectTargets[id]
    if (selected && target) this.projectTargets[id] = target
    this.projectChoices = { ...this.projectChoices, [id]: selected }
    const choices = { ...projects.choices, [id]: selected }
    this.publish({
      projects: {
        ...projects,
        choices,
        targets: this.projectTargets,
        selectionKey: this.projectSelectionKey(projects.groups, choices),
      },
    })
  }

  /** Match saved repository IDs or exact folder grants, without comparing origin URLs. */
  projectHasSavedPermission = (
    id: string,
    policy: CloudOngoingStatus | null | undefined = this.snapshot.ongoing,
    exact = false,
  ) => {
    return hasSavedProjectPermission(
      this.projectPages?.groups.find((item) => item.id === id),
      policy,
      exact,
    )
  }

  private hasSavedProject(id: string, exact = false) {
    return (
      this.projectHasSavedPermission(id, undefined, exact) ||
      Object.values(this.snapshot.teamStatuses ?? {}).some((policy) =>
        this.projectHasSavedPermission(id, policy, exact),
      )
    )
  }

  /** Source recovery may recheck only an already saved Personal binding. */
  projectHasReviewableSource = (id: string) => {
    const group = this.projectPages?.groups.find((item) => item.id === id)
    return !!group && this.reviewableProjectSelections(group, this.snapshot.ongoing).length > 0
  }

  private reviewableProjectSelections(
    group: DesktopCloudProjectGroup,
    policy: CloudOngoingStatus | null | undefined,
  ) {
    return reviewableProjectSelections(
      this.supportedProjectSelections(group),
      policy,
      group.repositoryWide ? group.id : undefined,
    )
  }

  /** Save the explicit Settings folder choice without a second prompt. */
  setSavedProject = async (
    context: CloudViewSnapshot,
    id: string,
    selected: boolean,
    team?: string,
    reviewSavedSource = false,
  ) => {
    const port = this.port.ongoing
    const anchor = this.projectBinding(!selected)
    const binding = anchor && { ...anchor, destinationId: team ?? anchor.destinationId }
    const group = this.projectPages?.groups.find((item) => item.id === id)
    const policyOf = (value: CloudViewSnapshot) =>
      team ? value.teamStatuses?.[team] : value.ongoing
    const policy = policyOf(this.snapshot)
    const policyRevision = policyOf(context)?.policyRevision
    const savedFolder = policy?.savedFolders?.find((folder) => folder.groupId === id)
    if (
      !this.isCurrentConfirmation(context) ||
      !port ||
      !binding ||
      (!group && !savedFolder) ||
      !policyRevision ||
      policyRevision !== policy?.policyRevision ||
      (team && selected) ||
      this.snapshot.busy ||
      this.syncInFlight ||
      (selected && !this.projectReader.canUseChoices) ||
      policy?.consentVersion !== "desktop-project-sync-v1" ||
      (!team && !policy.storageKnown) ||
      (reviewSavedSource && (!policy.sourceConfirmationRequired || !selected || !!team)) ||
      (selected && this.hasSavedProject(id, true) && !reviewSavedSource)
    )
      return
    let projects: CloudProjectSelection[] = []
    if (selected) {
      if (
        !group ||
        context.projects?.groups.find((item) => item.id === id)?.confirmationKey !==
          this.projectConfirmationKey(group)
      )
        return
      projects = reviewSavedSource
        ? this.reviewableProjectSelections(group, policy)
        : this.supportedProjectSelections(group)
      if (!projects.length) return
    }
    const folderPaths = group
      ? selected
        ? group.folderPaths
        : [...new Set([group.path, ...group.folderPaths])]
      : savedFolder
        ? [savedFolder.path]
        : []
    const generation = this.generation
    const revision = this.ongoingRevision
    const authority = this.folderMoveScope.authority
    this.publish({ busy: true, uploadActionError: null })
    try {
      const read = selected ? this.projectReader.finishRead() : undefined
      if (read) await read
      if (
        !this.isCurrentConfirmation(context) ||
        (selected && !this.projectReader.canUseChoices) ||
        policyOf(this.snapshot)?.policyRevision !== policyRevision
      )
        return
      const status = await port.setProject({
        ...binding,
        groupId: id,
        selected,
        projects,
        folderPaths,
        expectedPolicyRevision: policyRevision,
      })
      if (
        this.active &&
        generation === this.generation &&
        authority === this.folderMoveScope.authority &&
        (team
          ? policyOf(this.snapshot)?.policyRevision === policyRevision
          : revision === this.ongoingRevision)
      )
        this.receiveOngoing(status)
      if (
        reviewSavedSource &&
        this.active &&
        generation === this.generation &&
        authority === this.folderMoveScope.authority
      )
        return confirmsSavedWrite(
          policyOf(this.snapshot),
          status,
          binding,
          expectedFolderRoute(group ? [group] : [], projects),
        )
    } catch {
      if (
        this.active &&
        generation === this.generation &&
        authority === this.folderMoveScope.authority &&
        this.snapshot.cloud.identity?.id === binding.accountId &&
        this.snapshot.cloud.destination?.id === anchor?.destinationId
      )
        this.publish({
          uploadActionError: reviewSavedSource
            ? "Restore saved sources or turn off their folders."
            : "Could not update the folder. Try again.",
        })
    } finally {
      if (this.active && generation === this.generation) this.publish({ busy: false })
    }
  }
  /** Move one saved folder group after explicit Settings Save. */
  moveSavedProject = createFolderMove({
    getPort: () => this.port.ongoing,
    getSnapshot: this.getSnapshot,
    getDiscovery: () => ({ pages: this.projectPages, choices: this.confirmedProjectChoices }),
    canStart: (context) => !this.syncInFlight && this.isCurrentConfirmation(context),
    scope: () => ({ ...this.folderMoveScope, generation: this.active ? this.generation : -1 }),
    setInFlight: (busy) => (this.syncInFlight = busy),
    publish: (patch) => this.publish(patch),
    receiveStatus: (status) => this.receiveOngoing(status),
  })
  /** Pause or resume uploads on this device without retrying interrupted work. */
  controlSync = (paused: boolean, context = this.snapshot) =>
    this.changeSyncControl(
      (port) =>
        paused
          ? port.setEnabled(false)
          : port.setEnabled(true, projectBinding(context.cloud, false) ?? undefined),
      { pause: paused },
    ).then((result) => result === "requested")

  private confirmTeamFolders = createTeamFolderConfirmation({
    getPort: () => this.port.ongoing,
    getSnapshot: this.getSnapshot,
    getDraft: () => ({
      pages: this.projectPages,
      choices: this.projectChoices,
      targets: this.projectTargets,
    }),
    generation: () => (this.active ? this.generation : undefined),
    authority: () => this.folderMoveScope.authority,
    canStart: (context) => !this.syncInFlight && this.isCurrentConfirmation(context),
    canUseChoices: () => this.projectReader.canUseChoices,
    hasSavedProject: (id) => this.hasSavedProject(id, true),
    finishRead: () => this.projectReader.finishRead(),
    publish: (patch) => this.publish(patch),
    receive: (status) => this.receiveOngoing(status),
    clearCommittedChoices: (ids, draft) => this.clearCommittedChoices(ids, draft),
  })

  /** Retry only the Personal policy shown when the user selected Retry. */
  retrySync = async (context: CloudViewSnapshot) => {
    const target = retryTarget(context, this.snapshot, this.projectBinding(true))
    if (target)
      await this.changeSyncControl((port) => port.retry(target.binding, target.revision))
  }

  /** Retry the exact Team chosen in Settings. Never fall back to Personal. */
  retryTeamSync = async (context: CloudViewSnapshot, destinationId: string) => {
    const target = retryTarget(context, this.snapshot, this.projectBinding(true), destinationId)
    const policy = this.snapshot.teamStatuses?.[destinationId]
    if (target && policy && !this.syncInFlight)
      await this.changeSyncControl((port) => port.retry(target.binding, target.revision), {
        team: policy,
      })
  }

  /** Ask desktop to scan the exact saved policy shown in Settings. */
  private backfillSync = async (context: CloudViewSnapshot, teamId?: string) => {
    const backfill = this.port.ongoing.backfill
    const target = backfillTarget(context, this.snapshot, this.projectBinding(true), teamId)
    if (!backfill || !target || this.syncInFlight) return "stopped" as const
    return this.changeSyncControl(() => backfill(target.binding, target.revision), {
      team: target.team,
      backfillCurrent: () =>
        matchesUploadPolicy(
          uploadPolicy(this.snapshot, teamId),
          target.binding,
          target.revision,
        ),
    })
  }

  /** Request a full scan of each eligible saved workspace without changing folder access. */
  syncNow = async (context: CloudViewSnapshot) => {
    const generation = this.generation
    const anchor = projectBinding(context.cloud, false)
    if (!anchor) return false
    let requested = false
    let scanError: string | undefined
    for (const teamId of [undefined, ...Object.keys(context.teamStatuses ?? {})]) {
      if (!this.active || generation !== this.generation) return false
      const current = this.snapshot
      if (!backfillTarget(context, current, this.projectBinding(), teamId)) continue
      const result = await this.backfillSync(context, teamId)
      if (result === "stopped") return false
      if (result === "requested") requested = true
      else scanError ??= result.error
    }
    if (scanError && this.active && generation === this.generation && !this.snapshot.busy)
      this.publish({ uploadActionError: scanError })
    return requested && !scanError
  }

  /** Clear only the error that the user chose to review. */
  clearUploadActionError = (message: string | null | undefined) => {
    if (message && this.snapshot.uploadActionError === message)
      this.publish({ uploadActionError: null })
  }

  private changeSyncControl = createSyncControl({
    getPort: () => this.port.ongoing,
    getSnapshot: this.getSnapshot,
    generation: () => (this.active ? this.generation : undefined),
    policyRevision: () => this.ongoingRevision,
    publish: (patch) => this.publish(patch),
    receive: (status) => this.receiveOngoing(status, true),
  })
  /** Serialize surface actions and reject replies superseded by desktop events. */
  private async run(
    action: Parameters<typeof noteCloudAction>[1] | null,
    task: () => Promise<CloudSnapshot>,
    failureMessage: (error: unknown) => string | null = () =>
      "Cloud could not complete that action. Try again.",
  ) {
    if (this.snapshot.busy) return false
    if (this.analytics && action) noteCloudAction(this.snapshot.screen, action)
    const generation = this.generation
    this.publish({ busy: true, error: null, accountReadFailed: false })
    const revision = this.revision
    try {
      const value = await task()
      if (!this.active || generation !== this.generation) return false
      const fresh = revision === this.revision
      if (fresh) this.receive(value)
      this.publish({ busy: false })
      const consistent = fresh || JSON.stringify(this.snapshot.cloud) === JSON.stringify(value)
      return consistent && value.status !== "credential_failed"
    } catch (error) {
      if (this.active && generation === this.generation)
        this.publish({
          busy: false,
          error: isCloudBusy(error) ? CLOUD_BUSY_MESSAGE : failureMessage(error),
        })
      return false
    }
  }
  /** Start browser approval, optionally for an action missing from the active grant. */
  begin = (update?: CloudAccessUpdate) =>
    this.run(update ? null : this.snapshot.cloud.authEnabled ? "sign_in" : "learn_more", () =>
      this.port.begin(update),
    )
  retry = () =>
    !this.snapshot.cloud.authEnabled ? this.begin() : this.run("retry", this.port.begin)
  /** Record explicit browser cancellation without cancelling saved source requests. */
  cancel = () => this.run("cancel", this.port.cancel)
  /** Cancel while navigating away without adding a second action event. */
  cancelSilently = () => this.run(null, this.port.cancel)
  /** Close the account connection and let desktop code clean up old Team grants. */
  signOut = () => {
    this.folderMoveScope.authority++
    this.invalidateContentConfirmation()
    this.projectReader.suspend()
    return this.run("sign_out", this.port.signOut)
  }
  /** Run only after the UI's separate reset confirmation; keep the remote warning. */
  resetLocal = () => this.run(null, this.port.resetLocal)
  /** Clear an old Team connection without changing the account or folder consent. */
  resetWorkspace = async (workspaceId: string) => {
    const binding = this.projectBinding(true)
    const connection = this.snapshot.workspaceConnections?.[workspaceId]
    const reset = this.port.resetWorkspace
    if (
      !this.active ||
      !binding ||
      connection?.accountId !== binding.accountId ||
      !connection.localResetAvailable ||
      !reset ||
      this.snapshot.busy
    )
      return false
    const generation = this.generation
    const current = () =>
      this.active &&
      generation === this.generation &&
      this.projectBinding(true)?.accountId === binding.accountId &&
      this.projectBinding(true)?.destinationId === binding.destinationId
    this.publish({ busy: true, error: null, accountReadFailed: false })
    try {
      const status = await reset(binding, workspaceId)
      if (!current()) return false
      if (status !== "signed_out") {
        this.publish({ error: "Could not clear the old Team connection. Try again." })
        return false
      }
      if (this.snapshot.workspaceConnections?.[workspaceId] === connection)
        this.receiveWorkspace({
          ...connection,
          localResetAvailable: false,
          remoteRevocationUnconfirmed: true,
        })
      return true
    } catch (error) {
      if (current())
        this.publish({
          error: isCloudBusy(error)
            ? CLOUD_BUSY_MESSAGE
            : "Could not clear the old Team connection. Try again.",
        })
      return false
    } finally {
      if (this.active && generation === this.generation) this.publish({ busy: false })
    }
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
    await this.run(action, () =>
      this.saveDraft({ ...this.snapshot.cloud.draft, screen }, false),
    )
    if (screen === "folders" && !this.snapshot.projects?.loaded) await this.loadProjects()
  }
  /** Open the configured dashboard root without sending device proof. */
  openDashboard = () =>
    this.port.openDashboard?.() ??
    Promise.reject(new Error("The Cloud dashboard is unavailable."))

  /** Simulated ports may omit the browser setup handoff. */
  canOpenSetup = () => !!this.port.openSetup

  /** The browser owns email verification and workspace admission. */
  openSetup = () =>
    this.port.openSetup?.() ?? Promise.reject(new Error("Cloud setup is unavailable."))

  /** Require a fresh destination list for the same device grant before showing folders. */
  continueFromDashboard = async () => {
    const { cloud } = this.snapshot
    const binding = this.projectBinding()
    if (!binding || this.snapshot.screen !== "identity") return
    const generation = this.generation
    await this.run(
      "continue",
      async () => {
        this.projectReader.suspend()
        // The native check can rotate the grant before reading destinations.
        // Wait for its HTTP deadlines and the current read in the account lane.
        const read = (this.workspaceRead = new AbortController())
        const workspaces = await this.projectReader
          .request(() => this.port.workspaces?.(binding) ?? Promise.resolve(null), read.signal)
          .finally(() => {
            if (this.workspaceRead === read) this.workspaceRead = undefined
          })
        const { cloud: current, screen } = this.snapshot
        if (
          !this.active ||
          this.generation !== generation ||
          screen !== "identity" ||
          current.status !== "signed_in" ||
          current.identity?.id !== binding.accountId ||
          current.destination?.id !== binding.destinationId ||
          current.grantId !== cloud.grantId ||
          current.grantExpiresAt !== cloud.grantExpiresAt ||
          !workspaces?.some(
            ({ id, kind }) => id === binding.destinationId && kind === "personal",
          )
        )
          throw new Error("Could not verify Cloud workspaces.")
        if (this.projectPages && this.projectReader.canUseChoices)
          this.publishProjectGroups(this.projectPages, workspaces)
        return this.saveDraft({ ...current.draft, screen: "folders" }, false)
      },
      (error) =>
        this.snapshot.cloud.status === "reauth_required"
          ? null
          : (cloudReadScopeMessage(error) ?? "Could not continue to Cloud folders. Try again."),
    )
    const view = this.getSnapshot()
    if (view.screen === "folders" && !view.projects?.loaded) await this.loadProjects()
  }
  /** Confirm the rendered folder choices only after an explicit Start click. */
  startSync = async (context: CloudViewSnapshot = this.snapshot) => {
    if (!this.isCurrentConfirmation(context)) return
    return this.startProjectSync()
  }
  private isCurrentConfirmation(context: CloudViewSnapshot) {
    return (
      this.active &&
      context.contextRevision === this.consentRevision &&
      confirmationContextKey(context) === confirmationContextKey(this.snapshot)
    )
  }

  /** Save new choices and renew a stale saved selection only for the same Team. */
  startPendingProjects = async (
    context: CloudViewSnapshot,
    pendingOnly = true,
    confirmTeam?: string,
    reviewSavedSource?: string,
  ) => {
    if (!this.isCurrentConfirmation(context)) return
    if (confirmTeam) return this.confirmTeamFolders(context, confirmTeam)
    if (reviewSavedSource)
      return this.setSavedProject(context, reviewSavedSource, true, undefined, true)
    return this.startProjectSync(pendingOnly, true)
  }

  private projectPolicy(destinationId: string) {
    return destinationId === this.snapshot.cloud.destination?.id
      ? this.snapshot.ongoing
      : this.snapshot.teamStatuses?.[destinationId]
  }

  /** Clear only choices submitted by this operation. Keep later edits staged. */
  private clearCommittedChoices(ids: string[], draft: CloudProjectChoices) {
    const projects = this.snapshot.projects
    if (!projects || draft !== this.projectChoices) return
    const choicesDraft = { ...this.projectChoices }
    const targets = { ...this.projectTargets }
    for (const id of ids) {
      delete choicesDraft[id]
      delete targets[id]
    }
    this.projectChoices = choicesDraft
    this.projectTargets = targets
    this.invalidateContentConfirmation()
    const choices = cloudProjectChoices(
      projects.groups,
      this.confirmedProjectChoices,
      choicesDraft,
      this.defaultProjectSelection(projects.workspaces),
    )
    this.publish({
      projects: {
        ...projects,
        choices,
        targets,
        selectionKey: this.projectSelectionKey(projects.groups, choices),
      },
    })
  }

  /** Commit the exact current group members through desktop ongoing consent. The
   * completion screen stays available while desktop acceptance is still unknown.
   */
  private async startProjectSync(pendingOnly = false, settings = false) {
    const port = this.port.ongoing
    const binding = this.projectBinding()
    const projects = this.snapshot.projects
    const { choicesForStart, explicitOnly } = projectSyncChoices(
      this.snapshot,
      this.projectTargets,
      pendingOnly,
      (id) => this.hasSavedProject(id, settings),
    )
    const selected = this.selectedProjects(choicesForStart)
    if (
      !port ||
      !binding ||
      this.snapshot.busy ||
      this.syncInFlight ||
      cloudUploadStatusUnknown(this.snapshot.ongoing) ||
      !this.snapshot.cloud.captureEnabled ||
      !projects ||
      !this.projectReader.canUseChoices ||
      !selected.length
    )
      return
    const groups = projects.groups.filter(
      (group) => group.captureSupported && choicesForStart[group.id],
    )
    const destinations = [
      ...new Set(groups.map(({ id }) => this.projectTargets[id] ?? binding.destinationId)),
    ]
    if (!settings) {
      const singleWorkspaceChoices =
        destinations.length === 1 && !explicitOnly
          ? Object.fromEntries(
              Object.entries(choicesForStart).filter(
                ([id]) =>
                  (this.projectTargets[id] ?? binding.destinationId) === destinations[0],
              ),
            )
          : undefined
      return this.startFolderRoutes(port, binding, groups, destinations, singleWorkspaceChoices)
    }
    if (destinations.length > 1) {
      this.publish({
        uploadActionError:
          "Enable folders for one workspace at a time. No folder choices were saved.",
      })
      return
    }
    const destinationId = destinations[0]
    if (!destinationId) return
    const isTeam = destinationId !== binding.destinationId
    if (isTeam && teamFoldersToConfirm(this.snapshot, destinationId)) return
    if (
      isTeam &&
      !projects.workspaces?.some(({ id, kind }) => id === destinationId && kind === "team")
    ) {
      this.publish({
        uploadActionError: "A selected team is unavailable. Check the folder choices.",
      })
      return
    }
    const target = { ...binding, destinationId }
    const generation = this.generation
    const authority = this.folderMoveScope.authority
    const consentRevision = this.consentRevision
    const stillBound = () =>
      this.active &&
      generation === this.generation &&
      authority === this.folderMoveScope.authority &&
      this.projectBinding()?.accountId === binding.accountId &&
      this.projectBinding()?.destinationId === binding.destinationId
    const current = () => stillBound() && consentRevision === this.consentRevision
    this.syncInFlight = true
    const starting: SyncResult = { status: "starting" }
    try {
      if (!current() || groups.some((group) => this.hasSavedProject(group.id, settings))) return
      if (this.analytics) noteCloudAction(this.snapshot.screen, "start_sync")
      if (!pendingOnly) this.projectSyncState = "started"
      this.publish({
        ...(!pendingOnly ? { screen: "complete" as const } : {}),
        busy: pendingOnly,
        sync: starting,
        uploadActionError: null,
      })
      const read = this.projectReader.finishRead()
      if (read) await read
      if (!current() || !this.projectReader.canUseChoices) return
      const policy = this.projectPolicy(destinationId)
      let confirmed: boolean
      if (policy?.consentVersion === "desktop-project-sync-v1") {
        confirmed =
          (await this.addPendingProjects(port, target, groups, stillBound, settings)) === true
      } else {
        const draft = this.projectChoices
        const expected = expectedFolderRoute(this.projectPages?.groups ?? [], selected, groups)
        const choices = explicitOnly
          ? Object.fromEntries(groups.map(({ id }) => [id, true]))
          : Object.fromEntries(
              Object.entries(choicesForStart).filter(
                ([id]) => (this.projectTargets[id] ?? binding.destinationId) === destinationId,
              ),
            )
        const status = await port.enable(
          projectSyncConsent(target, this.projectPages?.sources ?? [], selected, {
            ...(destinationId === binding.destinationId ? this.confirmedProjectChoices : {}),
            ...choices,
          }),
        )
        if (!stillBound()) return
        const observed = this.projectPolicy(destinationId)
        if (observed === policy) {
          if (!current()) return
          this.receiveOngoing(status)
        }
        if (isTeam) this.publish({ lastTeamSync: this.projectPolicy(destinationId) ?? null })
        confirmed = confirmsSavedWrite(
          this.projectPolicy(destinationId),
          status,
          target,
          expected,
        )
        if (confirmed) this.clearCommittedChoices(Object.keys(choices), draft)
      }
      if (!stillBound()) return false
      if (!confirmed) {
        this.publish({
          sync: { status: "ambiguous" },
          uploadActionError: folderSaveError(undefined),
        })
        return false
      }
      if (this.snapshot.sync === starting) this.publish({ sync: { status: "idle" } })
      return true
    } catch (error) {
      const message = error instanceof Error ? error.message : error
      if (stillBound() && (pendingOnly || current()))
        this.publish({
          sync: { status: "ambiguous" },
          uploadActionError: folderSaveError(message),
        })
    } finally {
      this.syncInFlight = false
      if (this.active && generation === this.generation)
        this.publish({
          ...(pendingOnly ? { busy: false } : {}),
          ...(this.snapshot.sync === starting ? { sync: { status: "unavailable" } } : {}),
        })
    }
  }

  /** Use the existing workspace commands for one explicit onboarding choice.
   * A failed route stays staged. Accepted routes are not submitted again.
   */
  private async startFolderRoutes(
    port: NonNullable<CloudPort["ongoing"]>,
    binding: CloudDestinationBinding,
    groups: CloudProjectGroup[],
    destinations: string[],
    singleWorkspaceChoices?: CloudProjectChoices,
  ) {
    const destinationFor = (id: string) => this.projectTargets[id] ?? binding.destinationId
    const routes = projectFolderRoutes(this.snapshot, groups, destinations, destinationFor)
    if (!routes) return
    const generation = this.generation
    const authority = this.folderMoveScope.authority
    const consentRevision = this.consentRevision
    const current = () =>
      this.active &&
      generation === this.generation &&
      authority === this.folderMoveScope.authority &&
      this.snapshot.cloud.captureEnabled === true &&
      this.projectBinding()?.accountId === binding.accountId &&
      this.projectBinding()?.destinationId === binding.destinationId
    const requested: RequestedFolderRoute[] = []
    const checkRoutes = (savedRoutes = this.onboardingSavedRoutes) =>
      checkOnboardingFolderRoutes(requested, savedRoutes, (id) => this.projectPolicy(id))
    let needsReadback = false
    this.syncInFlight = true
    this.projectSyncState = "pending"
    const starting: SyncResult = { status: "starting" }
    this.publish({ busy: true, error: null, accountReadFailed: false, sync: starting })
    if (this.analytics) noteCloudAction(this.snapshot.screen, "start_sync")
    try {
      const read = this.projectReader.finishRead()
      if (read) await read
      if (consentRevision !== this.consentRevision) return
      for (const route of routes) {
        if (!current()) return
        const { id, groups, workspace } = route
        const target = { ...binding, destinationId: id }
        const ids = groups.map((group) => group.id)
        const choices = Object.fromEntries(ids.map((id) => [id, true]))
        const selected = selectedProjects(
          this.projectPages?.groups.filter((group) => ids.includes(group.id)) ?? [],
          this.projectPages?.sources ?? [],
          choices,
        )
        const expected = expectedFolderRoute(this.projectPages?.groups ?? [], selected, groups)
        requested.push({ target, name: workspace?.name ?? "Personal", expected })
        try {
          if (
            !this.projectReader.canUseChoices ||
            !isCurrentProjectFolderRoute(this.snapshot, route, destinationFor, (id) =>
              this.hasSavedProject(id),
            )
          )
            throw new Error("Folder choices or workspace availability changed.")
          const policy = this.projectPolicy(id)
          if (policy?.consentVersion === "desktop-project-sync-v1") {
            await this.addPendingProjects(port, target, groups, current, false)
          } else {
            const draft = this.projectChoices
            const status = await port.enable(
              projectSyncConsent(target, this.projectPages?.sources ?? [], selected, {
                ...singleWorkspaceChoices,
                ...choices,
              }),
            )
            if (!current()) return
            const observed = this.projectPolicy(id)
            if (observed === policy) this.receiveOngoing(status)
            if (!confirmsSavedWrite(this.projectPolicy(id), status, target, expected))
              throw new Error("Cloud did not confirm these folders.")
            this.clearCommittedChoices(
              singleWorkspaceChoices ? Object.keys(singleWorkspaceChoices) : ids,
              draft,
            )
          }
          if (!current()) return
          if (id !== binding.destinationId)
            this.publish({ lastTeamSync: this.projectPolicy(id) ?? null })
        } catch {
          if (!current()) return
          needsReadback = true
        }
      }
      if (!current()) return
      let result = checkRoutes()
      if (needsReadback || result.error) {
        await this.loadProjects(false, false)
        if (!current()) return
        result = checkRoutes(result.savedRoutes)
      }
      this.onboardingSavedRoutes = result.error ? result.savedRoutes : []
      if (result.error) return this.publish({ screen: "folders", error: result.error })
      this.projectSyncState = "started"
      this.publish({ screen: "complete", sync: { status: "idle" } })
    } finally {
      this.syncInFlight = false
      if (this.active && generation === this.generation)
        this.publish({
          busy: false,
          ...(this.snapshot.sync === starting ? { sync: { status: "idle" } } : {}),
        })
    }
  }

  /** Add groups one at a time so a failed group cannot replace any saved policy. */
  private async addPendingProjects(
    port: NonNullable<CloudPort["ongoing"]>,
    binding: CloudDestinationBinding,
    groups: CloudProjectGroup[],
    stillBound: () => boolean,
    settings: boolean,
  ) {
    for (const group of groups) {
      if (!stillBound()) return
      const member = this.projectPages?.groups.find(({ id }) => id === group.id)
      const policy = this.projectPolicy(binding.destinationId)
      if (
        !member ||
        group.confirmationKey !== this.projectConfirmationKey(member) ||
        (this.projectTargets[group.id] ??
          (settings ? undefined : this.snapshot.cloud.destination?.id)) !==
          binding.destinationId ||
        this.hasSavedProject(group.id, settings) ||
        !policy?.policyRevision ||
        (binding.destinationId === this.snapshot.cloud.destination?.id && !policy.storageKnown)
      )
        throw new Error("The saved workspace or folder changed.")
      const draft = this.projectChoices
      const selected = this.supportedProjectSelections(member)
      const status = await port.setProject({
        ...binding,
        groupId: group.id,
        selected: true,
        projects: selected,
        folderPaths: member.folderPaths,
        expectedPolicyRevision: policy.policyRevision,
      })
      if (!stillBound()) return
      if (this.projectPolicy(binding.destinationId) === policy) this.receiveOngoing(status)
      const confirmed = confirmsSavedWrite(
        this.projectPolicy(binding.destinationId),
        status,
        binding,
        expectedFolderRoute([member], selected),
      )
      if (!confirmed) {
        if (settings) return false
        throw new Error("Cloud did not confirm this folder.")
      }
      this.clearCommittedChoices([group.id], draft)
    }
    return true
  }
}
