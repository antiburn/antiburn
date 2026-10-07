import { FolderPlus, RefreshCw, X } from "lucide-react"
import { useCallback, useSyncExternalStore, type ReactNode } from "react"

import { LocalRepositoryList } from "../../../../components/repositories/LocalRepositoryList"
import { ToggleListIntro } from "../../../../components/ui/ToggleList"
import { Card } from "../../../../components/ui/Card"
import { PushButton } from "../../../../components/ui/PushButton"
import { SectionGroup } from "../../../../components/ui/SectionGroup"
import { StatusText } from "../../../../components/ui/StatusText"
import { cancelScan, scanHistory, scanNow } from "../../../../lib/ipc"
import { olderSessionsStatus } from "../../../../lib/presentation/scanStatusCopy"
import { scanStatusStore, withKnownAgents } from "../../../../lib/scanStatusStore"
import type { LocalRepositoryItem } from "../../../../lib/types/repository"
import { scanStatusLabel } from "../../../popover/ScanStatusBar"
import {
  StepSettingsRow,
  StepSettingsSectionGroup,
  StepSettingsToggleRow,
} from "./StepSettingsSearchRows"
import type { AppSettingsController } from "../../../settings/useAppSettings"

/**
 * The Sessions step's source sections: Monitoring, Scanning, Scan folders,
 * and Repositories. `SessionsStepSettings` owns the one `SourcesSession`
 * that Scan folders and Repositories read from, and renders these sections
 * between its own Activity and Remote hosts sections.
 */

/** The "Keep looking for new sessions" row. */
export function MonitoringSection({
  settings,
  update,
}: Pick<AppSettingsController, "settings" | "update">) {
  return (
    <SectionGroup title="Monitoring">
      <Card>
        <StepSettingsToggleRow
          searchId="monitoring"
          description="This option allows antiburn to read your session files in the background while you work. If you turn it off then they will only be read while the app or menu bar dropdown are open."
          checked={!settings.discoveryPaused}
          onChange={(next) => void update({ discoveryPaused: !next })}
        />
      </Card>
    </SectionGroup>
  )
}

/**
 * The two scans: the routine one over the last 30 days, and the historical
 * one over older sessions, as far back as retention allows. Each row says
 * what its own scan did.
 */
export function ScanningSection({ discoveryPaused }: { discoveryPaused: boolean }) {
  const scanStatus = useSyncExternalStore(
    scanStatusStore.subscribe,
    scanStatusStore.getSnapshot,
  )
  const running = scanStatus?.running === true
  const history = scanStatus?.history
  const historyPassRunning = running && history?.passRunning === true

  const handleRescan = useCallback(async () => {
    const status = await scanNow().catch(() => null)
    if (status) scanStatusStore.set(withKnownAgents(status))
  }, [])

  const handleScanOlder = useCallback(async () => {
    const status = await scanHistory().catch(() => null)
    if (status) scanStatusStore.set(withKnownAgents(status))
  }, [])

  const handleStopOlder = useCallback(async () => {
    const status = await cancelScan().catch(() => null)
    if (status) scanStatusStore.set(withKnownAgents(status))
  }, [])

  // While the historical pass runs, the recent row reports its own last pass.
  const recentStatus = scanStatusLabel(
    historyPassRunning && scanStatus ? { ...scanStatus, running: false } : scanStatus,
    discoveryPaused,
  )

  return (
    <SectionGroup title="Scanning">
      <Card>
        <StepSettingsRow
          searchId="sourceScanning"
          trailing={
            <PushButton
              className="gap-1.5"
              disabled={running}
              onClick={() => void handleRescan()}
            >
              <RefreshCw size={12} aria-hidden="true" />
              {running && !historyPassRunning ? "Scanning…" : "Rescan"}
            </PushButton>
          }
        >
          <ScanRowText status={recentStatus}>
            Sessions from the last 30 days. antiburn checks these every 5 minutes and when files
            change.
          </ScanRowText>
        </StepSettingsRow>
        <StepSettingsRow
          searchId="historicalScan"
          trailing={
            historyPassRunning ? (
              <PushButton onClick={() => void handleStopOlder()}>Stop</PushButton>
            ) : (
              <PushButton
                disabled={history?.state === "none"}
                onClick={() => void handleScanOlder()}
              >
                Scan now
              </PushButton>
            )
          }
        >
          <ScanRowText status={olderSessionsStatus(history, discoveryPaused)}>
            Sessions older than 30 days, as far back as Keep session data allows. Read once
            after setup.
          </ScanRowText>
        </StepSettingsRow>
      </Card>
    </SectionGroup>
  )
}

function ScanRowText({ children, status }: { children: ReactNode; status: string }) {
  return (
    <div className="mt-0.5 flex justify-between gap-x-2 type-footnote text-pretty text-label-secondary">
      <span>{children}</span>
      <span className="shrink-0">{status}</span>
    </div>
  )
}

/** The extra scan-folder list, beyond the agent stores and usual code directories. */
export function ScanFoldersSection({
  scanRoots,
  onAddFolder,
  onRemoveFolder,
}: {
  scanRoots: string[]
  onAddFolder: () => void
  onRemoveFolder: (path: string) => void
}) {
  return (
    <StepSettingsSectionGroup
      searchId="sourceFolders"
      trailing={
        <StatusText tone="secondary">
          {scanRoots.length === 0
            ? "Defaults only"
            : `${scanRoots.length} extra ${scanRoots.length === 1 ? "folder" : "folders"}`}
        </StatusText>
      }
    >
      <Card>
        <div className="space-y-2 px-4 py-3">
          <p className="type-footnote text-label-secondary">
            Agent session stores and the usual code directories are searched automatically. Add
            a folder only if you keep repositories somewhere else.
          </p>
          {scanRoots.length > 0 && (
            <ul className="space-y-1">
              {scanRoots.map((root) => (
                <li key={root} className="flex items-center gap-2">
                  <span
                    dir="rtl"
                    title={root}
                    className="min-w-0 flex-1 truncate text-left type-footnote text-label"
                  >
                    <bdi>{root}</bdi>
                  </span>
                  <button
                    type="button"
                    onClick={() => onRemoveFolder(root)}
                    aria-label={`Stop scanning ${root}`}
                    className="shrink-0 rounded p-0.5 text-label-tertiary hover:bg-surface-hover hover:text-label-secondary"
                  >
                    <X size={12} strokeWidth={2.5} aria-hidden="true" />
                  </button>
                </li>
              ))}
            </ul>
          )}
          <PushButton className="gap-1.5" onClick={onAddFolder}>
            <FolderPlus size={12} aria-hidden="true" />
            Add a folder…
          </PushButton>
        </div>
      </Card>
    </StepSettingsSectionGroup>
  )
}

/** The repository list, plus the "include folders without git" switch. */
export function RepositoriesSection({
  repositories,
  scanning,
  onToggleRepository,
  onLocate,
  includeNonRepoFolders,
  onIncludeNonRepoFoldersChange,
}: {
  repositories: LocalRepositoryItem[]
  scanning: boolean
  onToggleRepository: (item: LocalRepositoryItem, enabled: boolean) => void
  onLocate: () => void
  includeNonRepoFolders: boolean
  onIncludeNonRepoFoldersChange: (next: boolean) => void
}) {
  return (
    <StepSettingsSectionGroup searchId="sourceRepositories">
      <Card>
        <ToggleListIntro>
          Switching off a repository deletes the sessions read from it and stops reading new
          ones.
        </ToggleListIntro>
        <div className="h-[280px]">
          <LocalRepositoryList
            repositories={repositories}
            loading={scanning}
            onToggleRepository={onToggleRepository}
            onLocate={onLocate}
          />
        </div>
      </Card>
      <Card>
        <StepSettingsToggleRow
          searchId="sourceNonRepoFolders"
          description="Counts sessions started in a folder that isn't a repository. Burn checks still need a repository."
          checked={includeNonRepoFolders}
          onChange={onIncludeNonRepoFoldersChange}
        />
      </Card>
    </StepSettingsSectionGroup>
  )
}
