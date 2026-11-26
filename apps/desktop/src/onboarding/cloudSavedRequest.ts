import {
  CLOUD_BUSY_MESSAGE,
  cloudSourceKind,
  type CloudFolder,
  type CloudPort,
  type CloudSourceKind,
  type SyncResult,
} from "../lib/cloud"
import { personalCloudDestination } from "../lib/cloudDestination"
import { noteCloudAction } from "../lib/onboardingAnalytics"
import {
  isCloudBusy,
  legacyCaptureConsent,
  selectedFoldersKey,
  type CloudViewSnapshot,
} from "./cloudSessionState"

/** Stage picked sources without editing the existing draft or any saved request. */
export function addDraftFolders(
  current: CloudFolder[],
  paths: string[],
  sourceKind: CloudSourceKind,
) {
  const folders = current.map((folder) => ({ ...folder }))
  let changed = false
  for (const path of paths) {
    const existing = folders.find((folder) => matchesFolder(folder, path, sourceKind))
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
  return changed ? folders : undefined
}

/** Edit only the requested provider and path. An absent selection removes the draft row. */
export function editDraftFolder(
  folders: CloudFolder[],
  path: string,
  sourceKind: CloudSourceKind,
  selected?: boolean,
) {
  return selected === undefined
    ? folders.filter((folder) => !matchesFolder(folder, path, sourceKind))
    : folders.map((folder) =>
        matchesFolder(folder, path, sourceKind) ? { ...folder, selected } : folder,
      )
}

function matchesFolder(folder: CloudFolder, path: string, sourceKind: CloudSourceKind) {
  return folder.path === path && cloudSourceKind(folder) === sourceKind
}

/** IDs for one legacy request. The session resets both when the Cloud context changes. */
export interface SavedRequestState {
  id: string | null
  preparedId: string | null
}

/** Run the legacy saved request. The caller admits the rendered Start action and
 * owns the shared sync guard. The request keeps its ID across retries.
 */
export async function startSavedRequest({
  port,
  request,
  getSnapshot,
  generation,
  revision,
  consentRevision,
  syncInFlight,
  setSyncInFlight,
  publish,
  analytics,
}: {
  port: CloudPort
  request: SavedRequestState
  getSnapshot: () => CloudViewSnapshot
  generation: () => number | undefined
  revision: () => number
  consentRevision: () => number
  syncInFlight: boolean
  setSyncInFlight: (value: boolean) => void
  publish: (patch: Partial<CloudViewSnapshot>) => void
  analytics: boolean
}) {
  const { cloud, busy, screen } = getSnapshot()
  const selected = cloud.draft.folders.filter((folder) => folder.selected)
  const selectionKey = selectedFoldersKey(cloud)
  const destination = personalCloudDestination(cloud.destination ?? null)
  if (
    busy ||
    syncInFlight ||
    (!cloud.captureEnabled && screen !== "folders") ||
    !(cloud.captureEnabled || cloud.syncEnabled) ||
    cloud.status !== "signed_in" ||
    !!cloud.setupError ||
    !cloud.identity ||
    !destination ||
    cloud.pendingSync?.result.status === "cancelled" ||
    !selected.length
  )
    return
  if (analytics) noteCloudAction(cloud.captureEnabled ? screen : "folders", "start_sync")
  const startedGeneration = generation()
  const startedRevision = revision()
  const account = cloud.identity.id
  const desktopCapture = cloud.captureEnabled === true
  const destinationId = destination.id
  const consent = legacyCaptureConsent({ accountId: account, destinationId }, selected)
  const requestId = request.id ?? crypto.randomUUID()
  request.id = requestId
  const existing = request.preparedId === requestId
  const startedConsentRevision = consentRevision()
  const sameGeneration = () =>
    startedGeneration !== undefined && startedGeneration === generation()
  const current = () => {
    const { cloud } = getSnapshot()
    return (
      sameGeneration() &&
      request.id === requestId &&
      cloud.status === "signed_in" &&
      !cloud.setupError &&
      cloud.identity?.id === account &&
      personalCloudDestination(cloud.destination ?? null)?.id === destinationId &&
      selectedFoldersKey(cloud) === selectionKey &&
      startedConsentRevision === consentRevision() &&
      !(
        cloud.pendingSync?.requestId === requestId &&
        cloud.pendingSync.result.status === "cancelled"
      )
    )
  }
  setSyncInFlight(true)
  const starting: SyncResult = { status: "starting" }
  publish({
    screen: "complete",
    busy: false,
    sync: starting,
    error: null,
    accountReadFailed: false,
    sourceListingNeedsRetry: false,
  })
  try {
    if (!existing) {
      await port.prepareSync(selected, requestId, consent)
      if (!current()) return
      request.preparedId = requestId
    }
    if (!current()) return
    const sync =
      existing && !desktopCapture
        ? await port.reconcile(requestId)
        : await port.start(selected, requestId, consent)
    if (current() && (desktopCapture || startedRevision === revision()))
      publish({ sync, busy: false, screen: "complete" })
  } catch (error) {
    if (current() && (desktopCapture || startedRevision === revision())) {
      const saved = desktopCapture && request.preparedId === requestId
      const busy = isCloudBusy(error)
      publish({
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
    setSyncInFlight(false)
    // Clear only this operation's progress, not state from a newer desktop event.
    if (sameGeneration() && getSnapshot().sync === starting)
      publish({ sync: { status: "unavailable" } })
  }
}
