import { CloudSession, cloudUploadStatusUnknown } from "../../../../onboarding/CloudSession"
import { CloudAccount } from "../../../settings/CloudAccount"
import { confirm } from "@tauri-apps/plugin-dialog"
import { useCallback, useId, useState, useSyncExternalStore } from "react"

import { pluralize } from "../../../../lib/presentation/text"
import { FolderPermissionNotice } from "../../../../components/repositories/FolderPermissionNotice"
import { Card } from "../../../../components/ui/Card"
import { RangeSlider } from "../../../../components/ui/RangeSlider"
import { SectionGroup } from "../../../../components/ui/SectionGroup"
import { SegmentedControl } from "../../../../components/ui/SegmentedControl"
import { appInfoStore } from "../../../../lib/appInfoStore"
import { openFolderAccessSettings, type AppInfo } from "../../../../lib/ipc"
import { byteLabel } from "../../../../lib/presentation/scanStatusCopy"
import type { LocalRepositoryItem } from "../../../../lib/types/repository"
import { useFolderPermissionFlow } from "../../../../lib/useFolderPermissionFlow"
import { SettingsRow, SettingsToggleRow } from "../../../settings/SettingsSearchRows"
import { useAppSettings, type AppSettingsController } from "../../../settings/useAppSettings"
import {
  MonitoringSection,
  RepositoriesSection,
  ScanFoldersSection,
  ScanningSection,
} from "./SessionSourcesSections"
import { SourcesSession } from "./SourcesSession"

/**
 * The Sessions step's settings: folder access, where antiburn scans,
 * monitoring, the historical scan, the activity window, how
 * much is indexed, and how long session data is kept.
 */

/** Narrowest and widest activity-list windows, mirroring the store's clamp. */
const MIN_DAYS = 1
const MAX_DAYS = 14

function dayLabel(days: number): string {
  return `${days} ${pluralize(days, "day", "days")}`
}

/** The "Show the last N days" row. */
function RecentDaysSection({
  settings,
  update,
}: Pick<AppSettingsController, "settings" | "update">) {
  return (
    <SectionGroup title="Activity">
      <Card>
        <SettingsRow
          searchId="recentDays"
          description={`Sessions and the popover show activity from the last ${dayLabel(
            settings.activityWindowDays,
          )}. Active sessions are always included. This changes the list, not storage; indexed sessions outside the window remain on this machine.`}
          trailing={
            <span className="type-body tabular-nums text-label-secondary">
              {dayLabel(settings.activityWindowDays)}
            </span>
          }
        >
          <RangeSlider
            className="mt-2 w-full"
            value={settings.activityWindowDays}
            min={MIN_DAYS}
            max={MAX_DAYS}
            ariaLabel="Days of activity to show"
            ariaValueText={dayLabel(settings.activityWindowDays)}
            onChange={(days) => void update({ activityWindowDays: days })}
          />
        </SettingsRow>
      </Card>
    </SectionGroup>
  )
}

/** The "Indexed sessions" row. */
function IndexedSessionsSection({ info }: { info: AppInfo | null }) {
  return (
    <SectionGroup title="Local storage">
      <Card>
        <SettingsRow
          searchId="indexedSessions"
          description="What's currently stored in the sqlite db. Your agents' own files are never touched."
          trailing={
            <span className="type-body tabular-nums text-label-secondary">
              {info
                ? `${info.indexedSessions} sessions · ${byteLabel(info.databaseBytes)}`
                : "—"}
            </span>
          }
        />
      </Card>
    </SectionGroup>
  )
}

type RetentionValue = "30" | "90" | "-1"

const RETENTION_OPTIONS: ReadonlyArray<{ value: RetentionValue; label: string }> = [
  { value: "30", label: "30 days" },
  { value: "90", label: "90 days" },
  { value: "-1", label: "Forever" },
]

function retentionLength(days: number): number {
  return days === -1 ? Number.POSITIVE_INFINITY : days
}

/**
 * The "Keep session data" retention row, bare (no section wrapper): this
 * step's own settings give it a card of its own, next to this step's other
 * local-data rows.
 */
function RetentionSection({
  settings,
  update,
  loaded,
}: Pick<AppSettingsController, "settings" | "update" | "loaded">) {
  async function handleRetentionChange(value: RetentionValue) {
    const days = Number(value)
    if (retentionLength(days) < retentionLength(settings.sessionDataRetentionDays)) {
      const period = days === 30 ? "30 days" : "90 days"
      const proceed = await confirm(
        `This immediately removes saved sessions and analysis whose last activity is older than ${period}. Original transcript files are unchanged. If the originals are no longer available, this may remove your only saved history.`,
        {
          title: `Keep session data for ${period}?`,
          kind: "warning",
          okLabel: "Change retention",
        },
      )
      if (!proceed) return
    }
    await update({ sessionDataRetentionDays: days })
  }

  return (
    <SettingsRow
      searchId="retention"
      description="Choose how long antiburn keeps saved sessions and analysis on this computer. A shorter period removes older history. Original transcript files are unchanged."
      trailing={
        <SegmentedControl
          options={RETENTION_OPTIONS}
          value={String(settings.sessionDataRetentionDays) as RetentionValue}
          ariaLabel="Session data retention"
          onChange={(value) => void handleRetentionChange(value)}
          disabled={!loaded}
        />
      }
    />
  )
}

export function SessionsStepSettings({
  sharedCloudSession,
}: { sharedCloudSession?: CloudSession } = {}) {
  const { settings, update, loaded } = useAppSettings()
  const [cloudSession] = useState(
    () => sharedCloudSession ?? new CloudSession(undefined, false, true),
  )
  const cloud = useSyncExternalStore(
    sharedCloudSession ? cloudSession.subscribeSnapshot : cloudSession.subscribe,
    cloudSession.getSnapshot,
    cloudSession.getSnapshot,
  )
  const cloudUnavailable = !cloud.cloud.authEnabled || !cloud.cloud.captureEnabled
  const cloudFolderRowId = useId()
  const canSetUpCloud =
    cloudSession.projectSyncSupported &&
    cloud.cloud.status === "signed_in" &&
    cloud.cloud.destination?.kind === "personal"
  const hasSavedFolders = [cloud.ongoing, ...Object.values(cloud.teamStatuses ?? {})].some(
    (policy) =>
      policy?.consentVersion === "desktop-project-sync-v1" && policy.projects.length > 0,
  )
  const deviceUploadsEnabled = cloud.deviceUploadEnabled === true
  const cloudStorageUnknown = cloudUploadStatusUnknown(cloud.ongoing)
  const uploadControlUnavailable =
    cloudStorageUnknown ||
    cloudUnavailable ||
    (hasSavedFolders ? cloud.deviceUploadEnabled === undefined : !canSetUpCloud)
  const uploadControlBusy =
    cloud.busy && !(cloud.canPauseBackfill && hasSavedFolders && deviceUploadsEnabled)
  const info = useSyncExternalStore(appInfoStore.subscribe, appInfoStore.getSnapshot)
  const [session] = useState(() => new SourcesSession())
  const { repositories, scanRoots, permissions, scanning } = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
  )

  // Granting is the one path that can add repositories the reader is waiting
  // for, so each grant refreshes the list rather than making them wait for the
  // whole queue.
  const permissionFlow = useFolderPermissionFlow(permissions.deferred, () => {
    void session.refresh()
  })
  const [rechecking, setRechecking] = useState(false)

  const handleRecheck = useCallback(async () => {
    setRechecking(true)
    await session.recheck()
    setRechecking(false)
  }, [session])

  const handleCopyDiagnostics = useCallback(() => session.copyDiagnostics(), [session])

  const handleToggle = useCallback(
    (item: LocalRepositoryItem, enabled: boolean) => session.toggleRepository(item, enabled),
    [session],
  )

  const handleLocate = useCallback(() => session.locate(), [session])

  const handleRemoveRoot = useCallback((path: string) => session.removeRoot(path), [session])

  const sections = (
    <>
      {info?.cloudEnabled && (
        <SectionGroup title="Antiburn Cloud">
          <Card divided={false}>
            <CloudAccount session={cloudSession} value={cloud} folderRowId={cloudFolderRowId}>
              <SettingsToggleRow
                searchId="cloudUpload"
                description={
                  !hasSavedFolders && canSetUpCloud
                    ? "Choose folders and confirm before uploading."
                    : hasSavedFolders && !deviceUploadsEnabled && !uploadControlUnavailable
                      ? "Uploads are paused on this device."
                      : undefined
                }
                checked={hasSavedFolders && deviceUploadsEnabled}
                disabled={
                  !loaded || !cloud.loaded || uploadControlBusy || uploadControlUnavailable
                }
                dimmed={cloudUnavailable}
                disabledTooltip={
                  !cloud.loaded
                    ? "Checking Cloud availability…"
                    : cloudStorageUnknown
                      ? "Check disk space and file permissions, then check saved uploads."
                      : cloudUnavailable
                        ? "Uploads are unavailable in this build. You can turn this on when Antiburn Cloud becomes available."
                        : !hasSavedFolders && !canSetUpCloud
                          ? "Sign in to set up uploads."
                          : undefined
                }
                onChange={(next) => {
                  if (
                    loaded &&
                    cloud.loaded &&
                    !uploadControlBusy &&
                    !uploadControlUnavailable
                  ) {
                    if (hasSavedFolders) {
                      void cloudSession.controlSync(!next)
                    } else if (next && canSetUpCloud) {
                      const row = document.getElementById(cloudFolderRowId)
                      const firstChoice = row?.querySelector<HTMLElement>(
                        "select:not(:disabled), button[role=switch]:not(:disabled)",
                      )
                      const focusTarget = firstChoice ?? row
                      focusTarget?.focus()
                    }
                  }
                }}
              />
            </CloudAccount>
          </Card>
        </SectionGroup>
      )}

      <RepositoriesSection
        repositories={repositories}
        scanning={scanning}
        onToggleRepository={(item, enabled) => void handleToggle(item, enabled)}
        onLocate={() => void handleLocate()}
        includeNonRepoFolders={settings.includeNonRepoFolders}
        onIncludeNonRepoFoldersChange={(next) => void update({ includeNonRepoFolders: next })}
      />
      <ScanFoldersSection
        scanRoots={scanRoots}
        onAddFolder={() => void handleLocate()}
        onRemoveFolder={(path) => void handleRemoveRoot(path)}
      />

      <MonitoringSection settings={settings} update={update} />
      <RecentDaysSection settings={settings} update={update} />

      <SectionGroup title="Local data">
        <Card>
          <RetentionSection settings={settings} update={update} loaded={loaded} />
        </Card>
      </SectionGroup>

      <ScanningSection discoveryPaused={settings.discoveryPaused} />

      <IndexedSessionsSection info={info} />
    </>
  )

  return (
    <>
      {permissions.supported && permissions.deferred.length > 0 ? (
        <FolderPermissionNotice
          deferred={permissions.deferred}
          phase={permissionFlow.phase}
          current={permissionFlow.current}
          position={permissionFlow.position}
          total={permissionFlow.total}
          recordedDenials={permissionFlow.recordedDenials}
          onRequest={permissionFlow.start}
          onOpenSettings={() => void openFolderAccessSettings()}
          onRecheck={() => void handleRecheck()}
          onCopyDiagnostics={() => void handleCopyDiagnostics()}
          rechecking={rechecking}
        />
      ) : null}

      {sections}
    </>
  )
}
