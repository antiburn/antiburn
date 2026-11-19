import {
  emptyCloudSnapshot,
  CLOUD_BUSY_MESSAGE,
  desktopCloudPort,
  cloudSourceKind,
  cloudProjectChoices,
  mergeCloudProjectGroups,
  type CloudDraft,
  type CloudPort,
  type CloudScreen,
  type CloudSnapshot,
  type CloudSourceKind,
  type SyncResult,
  type CloudProjectGroup,
  type CloudProjectChoices,
  type DesktopCloudProjectGroup,
  type CloudProjectSelection,
  type CloudOngoingStatus,
  type CloudDestinationBinding,
  type CloudProjectCursor,
  type CloudSourceSelection,
  type CloudWorkspace,
  type CloudWorkspaceRecovery,
} from "../lib/cloud"
import { noteCloudAction, noteCloudScreenViewed } from "../lib/onboardingAnalytics"
import { personalCloudDestination } from "../lib/cloudDestination"
import {
  accountViewPatch,
  backfillTarget,
  cloudContextChange,
  cloudUploadStatusUnknown,
  confirmationContextKey,
  hasSavedProjectPermission,
  isCloudBusy,
  legacyCaptureConsent,
  matchesUploadPolicy,
  mergeProjectSources,
  ongoingDestination,
  projectBinding,
  projectConfirmationKey,
  projectSelectionKey,
  projectSyncChoices,
  projectViewGroups,
  retryTarget,
  reviewableProjectSelections,
  selectedFoldersKey,
  selectedProjects,
  supportedProjectSelections,
  teamFoldersToConfirm,
  teamStatusPatch,
  uploadActionError,
  uploadPolicy,
  workspaceRecoveryPatch,
  type CloudViewSnapshot,
} from "./cloudSessionState"

export {
  canBackfillUploads,
  canRetryPersonalUploads,
  cloudOngoingStatusMessage,
  cloudStatusMessage,
  cloudUploadStatusUnknown,
  teamFoldersToConfirm,
  type CloudViewSnapshot,
} from "./cloudSessionState"

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
  private stopWorkspace: (() => void) | undefined
  private stopLocalTools: (() => void) | undefined
  private localToolsLoading = false
  private localToolsEventGeneration = 0
  private localToolsStatusRevision = -1
  private projectGeneration = 0
  private desktopProjects: DesktopCloudProjectGroup[] = []
  private projectSources: CloudSourceSelection[] = []
  private projectCursor: CloudProjectCursor | null = null
  private projectChoices: CloudProjectChoices = {}
  private projectTargets: Record<string, string> = {}
  private confirmedProjectChoices: CloudProjectChoices = {}
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
        this.stopWorkspace?.()
        this.stopWorkspace = undefined
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
    const { resetProjects, contextChanged } = cloudContextChange(this.snapshot.cloud, value)
    if (resetProjects) {
      this.projectGeneration++
      this.desktopProjects = []
      this.projectSources = []
      this.projectCursor = null
      this.projectChoices = {}
      this.projectTargets = {}
      this.confirmedProjectChoices = {}
      this.needsChoiceReview = false
      this.projectSyncStarted = false
    } else if (value.setupError && value.setupError !== this.snapshot.cloud.setupError) {
      this.projectGeneration++
    }
    const patch = accountViewPatch(this.snapshot, value, {
      resetProjects,
      contextChanged,
      ongoing: !!this.port.ongoing,
      projectSyncStarted: this.projectSyncStarted,
      preparedRequestId: this.preparedRequestId,
    })
    if (contextChanged) {
      this.requestId = null
      this.preparedRequestId = null
      this.consentRevision++
    }
    if (!this.port.ongoing && value.identity && value.pendingSync) {
      this.requestId = value.pendingSync.requestId
      this.preparedRequestId = value.pendingSync.requestId
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
  load = async () => {
    const generation = this.generation
    this.publish({ busy: true, error: null })
    void this.loadLocalTools()
    try {
      if (!this.stop) {
        const stop = await this.port.watch((value) => {
          if (this.active && generation === this.generation) {
            const setupRecovered = !!this.snapshot.cloud.setupError && !value.setupError
            this.receive(value)
            if (
              this.port.ongoing &&
              this.projectBinding() &&
              !this.snapshot.busy &&
              (!this.snapshot.projects || setupRecovered)
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
          if (this.active && generation === this.generation) this.receiveOngoing(status, true)
        })
        if (!this.active || generation !== this.generation) {
          stop()
          return
        }
        this.stopOngoing = stop
      }
      if (this.port.watchWorkspace && !this.stopWorkspace) {
        const stop = await this.port.watchWorkspace((connection) => {
          if (this.active && generation === this.generation) this.receiveWorkspace(connection)
        })
        if (!this.active || generation !== this.generation) {
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
      if (this.port.ongoing && this.snapshot.cloud.identity) await this.loadProjects()
    } catch (error) {
      if (this.active && generation === this.generation)
        this.publish({
          busy: false,
          loaded: true,
          error: isCloudBusy(error)
            ? CLOUD_BUSY_MESSAGE
            : "Could not load Cloud. Try again or continue locally.",
        })
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
    if (destination === "team" && status.destinationId) {
      this.publish({
        ...device,
        ...teamStatusPatch(this.snapshot, status.destinationId, status),
      })
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
    return supportedProjectSelections(group, this.projectSources)
  }
  private selectedProjects(
    choices: CloudProjectChoices = this.snapshot.projects?.choices ?? {},
  ): CloudProjectSelection[] {
    return selectedProjects(this.desktopProjects, this.projectSources, choices)
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
  /** Read one bounded page without saving initial ON choices. Desktop code
   * owns source evidence and worktree grouping; failed pages never become empty success.
   */
  loadProjects = async (more = false) => {
    const port = this.port.ongoing
    const binding = this.projectBinding(!more)
    if (!port || !binding || this.snapshot.projects?.loading || (more && !this.projectCursor))
      return
    const generation = ++this.projectGeneration
    const current = () =>
      this.active &&
      generation === this.projectGeneration &&
      JSON.stringify(this.projectBinding(!more)) === JSON.stringify(binding)
    const existing = this.snapshot.projects
    this.publish({
      projects: {
        groups: existing?.groups ?? [],
        choices: existing?.choices ?? {},
        workspaces: existing?.workspaces,
        targets: existing?.targets,
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
      let workspaces = existing?.workspaces
      if (!more) {
        const revision = this.ongoingRevision
        const previousTeams = this.snapshot.teamStatuses
        const previousConnections = this.snapshot.workspaceConnections
        const status = await port.status()
        if (!current()) return
        if (
          status?.accountId === binding.accountId &&
          status.destinationId === binding.destinationId
        )
          for (const team of status.teamStatuses ?? []) {
            const id = team.destinationId
            if (!id || id === binding.destinationId || team.accountId !== binding.accountId)
              continue
            if (previousTeams?.[id] === this.snapshot.teamStatuses?.[id])
              this.receiveOngoing(team)
            if (previousConnections?.[id] === this.snapshot.workspaceConnections?.[id])
              this.receiveWorkspaceRecovery({
                accountId: team.accountId,
                workspaceId: id,
                workspaceName: team.workspaceName,
                localResetAvailable: team.localResetAvailable,
                remoteRevocationUnconfirmed: team.remoteRevocationUnconfirmed,
              })
          }
        if (status && revision === this.ongoingRevision) this.receiveOngoing(status)
        if (this.snapshot.cloud.setupError) {
          this.publish({ projects: { ...this.snapshot.projects!, loading: false } })
          return
        }
        if (this.port.workspaces) {
          workspaces = (await this.port.workspaces(binding)) ?? undefined
          if (!current()) return
          if (
            workspaces &&
            !workspaces.some(
              ({ id, kind }) => id === binding.destinationId && kind === "personal",
            )
          )
            throw new Error("The signed-in workspace is not available for selection.")
        }
      }
      const cursor = more ? this.projectCursor : null
      const page = await port.groups(binding, cursor)
      if (!current()) return
      if (page.nextCursor && JSON.stringify(page.nextCursor) === JSON.stringify(cursor))
        throw new Error("Project discovery repeated a page.")
      const groups = mergeCloudProjectGroups(more ? this.desktopProjects : [], page.groups)
      this.desktopProjects = groups
      this.projectSources = mergeProjectSources(
        more ? this.projectSources : [],
        page.sourceRoots,
      )
      this.projectCursor = page.nextCursor
      const viewGroups = projectViewGroups(groups, this.projectSources)
      const choices = cloudProjectChoices(
        viewGroups,
        this.confirmedProjectChoices,
        this.projectChoices,
        this.defaultProjectSelection(workspaces),
      )
      const selectionKey = this.projectSelectionKey(viewGroups, choices, workspaces ?? [])
      if (existing?.selectionKey !== selectionKey) this.invalidateContentConfirmation()
      this.publish({
        projects: {
          groups: viewGroups,
          choices,
          workspaces,
          targets: this.projectTargets,
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
        })
    }
  }
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
      projects.loading ||
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

  /** Keep saved local grants visible when discovery loses a folder. Show grants
   * above or inside a group by directory boundary, never by a partial name.
   */
  projectHasSavedPermission = (
    id: string,
    policy: Pick<CloudOngoingStatus, "projects"> | null | undefined = this.snapshot.ongoing,
    exact = false,
  ) => {
    return hasSavedProjectPermission(
      this.desktopProjects.find((item) => item.id === id),
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

  /** Source recovery may recheck only an exact, already saved Personal binding. */
  projectHasReviewableSource = (id: string) => {
    const group = this.desktopProjects.find((item) => item.id === id)
    return !!group && this.reviewableProjectSelections(group, this.snapshot.ongoing).length > 0
  }

  private reviewableProjectSelections(
    group: DesktopCloudProjectGroup,
    policy: CloudOngoingStatus | null | undefined,
  ) {
    return reviewableProjectSelections(this.supportedProjectSelections(group), policy)
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
    const group = this.desktopProjects.find((item) => item.id === id)
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
      (!team && this.snapshot.projects?.loading) ||
      (selected && !!this.snapshot.projects?.error) ||
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
    this.publish({ busy: true, uploadActionError: null })
    try {
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
        (team
          ? policyOf(this.snapshot)?.policyRevision === policyRevision
          : revision === this.ongoingRevision)
      )
        this.receiveOngoing(status)
    } catch {
      if (
        this.active &&
        generation === this.generation &&
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
  /** Pause or resume uploads on this device without retrying interrupted work. */
  controlSync = (paused: boolean) => this.changeSyncControl((port) => port.setEnabled(!paused))

  /** Renew the exact saved Team selection and add the explicit same-Team draft. */
  private async confirmTeamFolders(context: CloudViewSnapshot, destinationId: string) {
    const binding = this.projectBinding()
    const displayed = teamFoldersToConfirm(context, destinationId)
    const current = teamFoldersToConfirm(this.snapshot, destinationId)
    const port = this.port.ongoing
    if (
      !binding ||
      binding.destinationId === destinationId ||
      !displayed ||
      displayed.revision !== current?.revision ||
      displayed.paths.length !== current.paths.length ||
      displayed.paths.some((path, index) => path !== current.paths[index]) ||
      !port?.confirmTeamFolders ||
      this.snapshot.busy ||
      this.syncInFlight
    )
      return
    const additions: Record<string, CloudProjectSelection[]> = {}
    for (const [id, target] of Object.entries(this.projectTargets)) {
      const group = this.desktopProjects.find((item) => item.id === id)
      if (
        target !== destinationId ||
        this.hasSavedProject(id, true) ||
        !group ||
        !context.projects?.loaded ||
        context.projects.loading ||
        context.projects.error ||
        context.projects.groups.find((item) => item.id === id)?.confirmationKey !==
          this.projectConfirmationKey(group)
      )
        return
      additions[id] = this.supportedProjectSelections(group)
      if (!additions[id].length) return
    }
    const addedGroups = Object.keys(additions)
    const draft = this.projectChoices
    const generation = this.generation
    const stillBound = () =>
      this.active &&
      generation === this.generation &&
      this.projectBinding()?.accountId === binding.accountId &&
      this.projectBinding()?.destinationId === binding.destinationId
    if (!stillBound()) return
    this.publish({ busy: true, uploadActionError: null })
    try {
      const status = await port.confirmTeamFolders({
        ...binding,
        destinationId,
        expectedPolicyRevision: displayed.revision,
        folderPaths: displayed.paths,
        ...(addedGroups.length ? { additions } : {}),
      })
      if (
        stillBound() &&
        status.accountId === binding.accountId &&
        status.destinationId === destinationId
      ) {
        if (this.snapshot.teamStatuses?.[destinationId]?.policyRevision === displayed.revision)
          this.receiveOngoing(status)
        if (addedGroups.length) this.clearCommittedChoices(addedGroups, draft)
      }
    } catch (error) {
      if (stillBound())
        this.publish({
          uploadActionError: isCloudBusy(error)
            ? CLOUD_BUSY_MESSAGE
            : "Could not save folders. Try again.",
        })
    } finally {
      if (this.active && generation === this.generation) this.publish({ busy: false })
    }
  }

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
      await this.changeSyncControl(
        (port) => port.retry(target.binding, target.revision),
        policy,
      )
  }

  /** Ask desktop to scan the exact saved policy shown in Settings. */
  backfillSync = async (context: CloudViewSnapshot, teamId?: string) => {
    const backfill = this.port.ongoing?.backfill
    const target = backfillTarget(context, this.snapshot, this.projectBinding(true), teamId)
    if (!backfill || !target || this.syncInFlight) return false
    return this.changeSyncControl(
      () => backfill(target.binding, target.revision),
      target.team,
      () =>
        matchesUploadPolicy(
          uploadPolicy(this.snapshot, teamId),
          target.binding,
          target.revision,
        ),
    )
  }

  private async changeSyncControl(
    action: (port: NonNullable<CloudPort["ongoing"]>) => Promise<CloudOngoingStatus | void>,
    team?: CloudOngoingStatus,
    backfillCurrent?: () => boolean,
  ) {
    const port = this.port.ongoing
    if (!this.active || !port || this.snapshot.busy) return false
    const generation = this.generation
    const revision = this.ongoingRevision
    const current = () =>
      this.active &&
      generation === this.generation &&
      (!backfillCurrent || backfillCurrent()) &&
      (!team || team.accountId === this.projectBinding(true)?.accountId)
    this.publish({ busy: true, uploadActionError: null })
    try {
      const status = await action(port)
      if (
        status &&
        current() &&
        (team
          ? !!team.destinationId &&
            status.destinationId === team.destinationId &&
            status.accountId === team.accountId &&
            this.snapshot.teamStatuses?.[team.destinationId] === team
          : revision === this.ongoingRevision)
      )
        this.receiveOngoing(status, true)
      return current()
    } catch (error) {
      if (current())
        this.publish({
          uploadActionError: uploadActionError(error, !!backfillCurrent, !!team),
        })
      return false
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
    } catch (error) {
      if (this.active && generation === this.generation)
        this.publish({
          busy: false,
          error: isCloudBusy(error)
            ? CLOUD_BUSY_MESSAGE
            : "Cloud could not complete that action. Try again.",
        })
      return false
    }
  }
  /** Start explicit browser approval; authentication alone never starts capture. */
  begin = () =>
    this.run(this.snapshot.cloud.authEnabled ? "sign_in" : "learn_more", this.port.begin)
  /** Cancel the previous browser attempt before opening another; keep desktop cleanup gates. */
  retry = () =>
    !this.snapshot.cloud.authEnabled
      ? this.begin()
      : this.run("retry", async () => {
          const cancelled = await this.port.cancel()
          if (cancelled.status === "credential_failed") return cancelled
          return this.port.begin()
        })
  /** Record explicit browser cancellation without cancelling saved source requests. */
  cancel = () => this.run("cancel", this.port.cancel)
  /** Cancel while navigating away without adding a second action event. */
  cancelSilently = () => this.run(null, this.port.cancel)
  /** Close the account connection and let desktop code clean up old Team grants. */
  signOut = () => {
    this.invalidateContentConfirmation()
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
    this.publish({ busy: true, error: null })
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
    if (!this.isCurrentConfirmation(context)) return
    if (this.port.ongoing) return this.startProjectSync()
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
    const consent = legacyCaptureConsent({ accountId: account, destinationId }, selected)
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
    if (confirmTeam) {
      await this.confirmTeamFolders(context, confirmTeam)
    } else if (reviewSavedSource) {
      await this.setSavedProject(context, reviewSavedSource, true, undefined, true)
    } else {
      await this.startProjectSync(pendingOnly, true)
    }
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
      !projects?.loaded ||
      projects.loading ||
      projects.error ||
      !selected.length
    )
      return
    const groups = projects.groups.filter(
      (group) => group.captureSupported && choicesForStart[group.id],
    )
    const destinations = [
      ...new Set(groups.map(({ id }) => this.projectTargets[id] ?? binding.destinationId)),
    ]
    const errorField = settings ? "uploadActionError" : "error"
    if (destinations.length > 1) {
      this.publish({
        [errorField]:
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
        [errorField]: "A selected team is unavailable. Check the folder choices.",
      })
      return
    }
    const target = { ...binding, destinationId }
    const generation = this.generation
    const consentRevision = this.consentRevision
    const stillBound = () =>
      this.active &&
      generation === this.generation &&
      this.projectBinding()?.accountId === binding.accountId &&
      this.projectBinding()?.destinationId === binding.destinationId
    const current = () => stillBound() && consentRevision === this.consentRevision
    this.syncInFlight = true
    const starting: SyncResult = { status: "starting" }
    try {
      if (!current() || groups.some((group) => this.hasSavedProject(group.id, settings))) return
      if (this.analytics) noteCloudAction(this.snapshot.screen, "start_sync")
      if (!pendingOnly) this.projectSyncStarted = true
      this.publish({
        ...(!pendingOnly ? { screen: "complete" as const } : {}),
        busy: pendingOnly,
        sync: starting,
        [errorField]: null,
      })
      const policy = this.projectPolicy(destinationId)
      if (policy?.consentVersion === "desktop-project-sync-v1") {
        await this.addPendingProjects(port, target, groups, stillBound, settings)
      } else {
        const draft = this.projectChoices
        const choices = explicitOnly
          ? Object.fromEntries(groups.map(({ id }) => [id, true]))
          : Object.fromEntries(
              Object.entries(choicesForStart).filter(
                ([id]) => (this.projectTargets[id] ?? binding.destinationId) === destinationId,
              ),
            )
        const status = await port.enable({
          ...target,
          version: "desktop-project-sync-v1",
          selectedSources: this.projectSources.filter((source) =>
            selected.some(
              (project) =>
                project.sourceKind === source.sourceKind && project.sourcePath === source.path,
            ),
          ),
          projects: selected,
          projectChoices: {
            ...(destinationId === binding.destinationId ? this.confirmedProjectChoices : {}),
            ...choices,
          },
          fullContent: true,
          ongoing: true,
          attachments: false,
        })
        if (!stillBound()) return
        const observed = this.projectPolicy(destinationId)
        if (observed === policy) {
          if (!current()) return
          this.receiveOngoing(status)
        }
        if (isTeam) this.publish({ lastTeamSync: this.projectPolicy(destinationId) ?? null })
        this.clearCommittedChoices(Object.keys(choices), draft)
      }
      if (stillBound() && this.snapshot.sync === starting)
        this.publish({ sync: { status: "idle" } })
    } catch (error) {
      const message = error instanceof Error ? error.message : error
      if (stillBound() && (pendingOnly || current()))
        this.publish({
          sync: { status: "ambiguous" },
          [errorField]: isCloudBusy(error)
            ? CLOUD_BUSY_MESSAGE
            : isTeam &&
                message ===
                  "Turn off all folders under the old Team policy before approving new uploads."
              ? "Refresh your Cloud folders, then save your Team choices again."
              : settings
                ? "Could not save the folder choices. Try again."
                : "Cloud sync was not confirmed. Check Cloud Settings to retry.",
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
      const member = this.desktopProjects.find(({ id }) => id === group.id)
      const policy = this.projectPolicy(binding.destinationId)
      if (
        !member ||
        group.confirmationKey !== this.projectConfirmationKey(member) ||
        this.projectTargets[group.id] !== binding.destinationId ||
        this.hasSavedProject(group.id, settings) ||
        !policy?.policyRevision ||
        (binding.destinationId === this.snapshot.cloud.destination?.id && !policy.storageKnown)
      )
        throw new Error("The saved workspace or folder changed.")
      const draft = this.projectChoices
      const status = await port.setProject({
        ...binding,
        groupId: group.id,
        selected: true,
        projects: this.supportedProjectSelections(member),
        folderPaths: member.folderPaths,
        expectedPolicyRevision: policy.policyRevision,
      })
      if (!stillBound()) return
      if (this.projectPolicy(binding.destinationId) === policy) this.receiveOngoing(status)
      this.clearCommittedChoices([group.id], draft)
    }
  }
}
