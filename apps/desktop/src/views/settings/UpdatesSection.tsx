import {
  AlertTriangle,
  Check as CheckGlyph,
  Download,
  LoaderCircle,
  RotateCw,
} from "lucide-react"
import { useSyncExternalStore } from "react"

import { Card } from "../../components/ui/Card"
import { PushButton } from "../../components/ui/PushButton"
import { Row } from "../../components/ui/Row"
import { SectionGroup } from "../../components/ui/SectionGroup"
import { StatusText } from "../../components/ui/StatusText"
import { ToggleRow } from "../../components/ui/ToggleRow"
import { createExternalStore } from "../../lib/externalStore"
import { relativeTime } from "../../lib/presentation/relativeTime"
import {
  checkForUpdates,
  getUpdateStatus,
  installUpdate,
  onUpdateStatus,
  restartToUpdate,
  startUpdateSimulation,
  type AppInfo,
  type UpdateStatusPayload,
} from "../../lib/ipc"
import type { AppSettingsController } from "./useAppSettings"

/**
 * The Updates section of the About pane.
 *
 * A section rather than a sidebar pane of its own: software update belongs
 * with the build it updates, which is what About is. The masthead above
 * already names the version, so the row here is the action and its status,
 * not a restatement.
 *
 * The updater plugin is the **only** network-capable surface in the whole
 * application, and `info.updatesSupported` is the shell's answer about whether
 * it actually registered — not a compile-time guess. Everything here hangs off
 * that one flag:
 *
 * - **Supported.** The automatic-check switch is shown and is real: the shell
 *   runs the schedule, and the results arrive here as events, so the line under
 *   the switch reports what the last automatic check actually found.
 * - **Unsupported.** The switch is not rendered at all. A build that cannot
 *   check for updates has no automatic behaviour to configure, and a disabled
 *   switch over a preference nothing reads is the exact "control that does
 *   nothing" this section must not ship. The section says why instead.
 */

type CheckState =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "current" }
  | { kind: "available"; version: string }
  | { kind: "downloading"; version: string; downloaded: number; total: number | null }
  | { kind: "installing"; version: string }
  | { kind: "installed"; version: string }
  | { kind: "failed"; message: string; operation: "check" | "install"; version: string | null }

export interface UpdatesSectionProps extends AppSettingsController {
  /** Absent until the shell answers; `null` outside the shell entirely. */
  info: AppInfo | null
}

/** Fold a shell-reported check outcome into the section's own state. */
function stateFromEvent(status: UpdateStatusPayload): CheckState {
  switch (status.kind) {
    case "available":
      return { kind: "available", version: status.version ?? "" }
    case "current":
      return { kind: "current" }
    case "downloading":
      return {
        kind: "downloading",
        version: status.version ?? "",
        downloaded: status.downloadedBytes ?? 0,
        total: status.totalBytes,
      }
    case "installing":
      return { kind: "installing", version: status.version ?? "" }
    case "installed":
      return { kind: "installed", version: status.version ?? "" }
    case "failed":
      return {
        kind: "failed",
        message: status.message ?? "The update could not be completed",
        operation: status.failureOperation ?? "check",
        version: status.version,
      }
    default:
      return { kind: "idle" }
  }
}

function formatBytes(bytes: number): string {
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

function DownloadProgress({ downloaded, total }: { downloaded: number; total: number | null }) {
  const percent = total ? Math.min(100, Math.round((downloaded / total) * 100)) : null
  return (
    <div className="mt-2">
      <div
        role="progressbar"
        aria-label="Downloading update"
        aria-valuemin={0}
        aria-valuemax={total ?? undefined}
        aria-valuenow={total ? downloaded : undefined}
        aria-valuetext={
          total ? `${formatBytes(downloaded)} of ${formatBytes(total)}` : undefined
        }
        className="ui-progress"
      >
        <div
          className="ui-progress-indicator"
          data-state={percent === null ? "indeterminate" : "determinate"}
          style={percent === null ? undefined : { width: `${percent}%` }}
        />
      </div>
    </div>
  )
}

function CheckStatus({ state }: { state: CheckState }) {
  switch (state.kind) {
    case "checking":
      return (
        <StatusText icon={LoaderCircle} iconClassName="animate-spin" tone="secondary">
          Checking…
        </StatusText>
      )
    case "current":
      return (
        <StatusText icon={CheckGlyph} iconStrokeWidth={2.5} tone="secondary">
          Up to date
        </StatusText>
      )
    case "available":
      return (
        <StatusText icon={Download} tone="primary">
          Version {state.version} is available
        </StatusText>
      )
    case "downloading":
      return (
        <div>
          <StatusText icon={Download} tone="primary">
            Downloading version {state.version}
            {state.total
              ? ` · ${formatBytes(state.downloaded)} of ${formatBytes(state.total)}`
              : ""}
          </StatusText>
          <DownloadProgress downloaded={state.downloaded} total={state.total} />
        </div>
      )
    case "installing":
      return (
        <StatusText icon={LoaderCircle} iconClassName="animate-spin" tone="primary">
          Verifying and installing version {state.version}…
        </StatusText>
      )
    case "installed":
      return (
        <StatusText icon={CheckGlyph} iconStrokeWidth={2.5} tone="primary">
          Version {state.version} is installed. Restart to use it.
        </StatusText>
      )
    case "failed":
      return (
        <StatusText icon={AlertTriangle} tone="secondary">
          {state.message}
        </StatusText>
      )
    default:
      return null
  }
}

type UpdatesSnapshot = {
  state: CheckState
  /** When the shell last checked on its own, as it reported it. */
  lastAutomatic: string | null
  latestRevision: number
}

// Module-level: the shell's automatic-check schedule and a manual check both
// land here regardless of how many settings windows are open to see them.
const updateStatusStore = createExternalStore<UpdatesSnapshot>({
  initial: { state: { kind: "idle" }, lastAutomatic: null, latestRevision: 0 },
  subscribe: async (set) => {
    const stop = await onUpdateStatus((status) => {
      setStatus(status, set)
    })
    const status = await getUpdateStatus()
    if (status) setStatus(status, set)
    return stop
  },
})

function setStatus(
  status: UpdateStatusPayload,
  set: (snapshot: UpdatesSnapshot) => void = updateStatusStore.set,
) {
  const current = updateStatusStore.getSnapshot()
  if (status.revision <= current.latestRevision) return
  set({
    state: stateFromEvent(status),
    lastAutomatic: status.automatic ? status.checkedAt : current.lastAutomatic,
    latestRevision: status.revision,
  })
}

export function UpdatesSection({ settings, update, info }: UpdatesSectionProps) {
  const { state, lastAutomatic } = useSyncExternalStore(
    updateStatusStore.subscribe,
    updateStatusStore.getSnapshot,
  )
  const supported = info?.updatesSupported ?? false
  const debugBuild = info?.debugBuild ?? false

  const setState = (next: CheckState) =>
    updateStatusStore.set({ ...updateStatusStore.getSnapshot(), state: next })

  const runCheck = async () => {
    setState({ kind: "checking" })
    try {
      setStatus(await checkForUpdates())
    } catch (error) {
      setState({
        kind: "failed",
        message: error instanceof Error ? error.message : "The check could not be completed",
        operation: "check",
        version: null,
      })
    }
  }

  const runInstall = async (version: string) => {
    setState({ kind: "downloading", version, downloaded: 0, total: null })
    try {
      setStatus(await installUpdate(version))
    } catch (error) {
      setState({
        kind: "failed",
        message: error instanceof Error ? error.message : "The update could not be installed",
        operation: "install",
        version,
      })
    }
  }

  const runSimulation = async () => {
    setState({ kind: "checking" })
    try {
      setStatus(await startUpdateSimulation())
    } catch (error) {
      setState({
        kind: "failed",
        message: error instanceof Error ? error.message : "The simulation could not start",
        operation: "check",
        version: null,
      })
    }
  }

  const busy =
    state.kind === "checking" || state.kind === "downloading" || state.kind === "installing"
  const simulationActive = debugBuild && state.kind !== "idle"
  const action = (() => {
    if (state.kind === "available") {
      return (
        <PushButton variant="primary" onClick={() => void runInstall(state.version)}>
          Install
        </PushButton>
      )
    }
    if (state.kind === "installed") {
      return (
        <PushButton
          variant="primary"
          onClick={() => void restartToUpdate()}
          trailingIcon={RotateCw}
        >
          Restart to update
        </PushButton>
      )
    }
    if (state.kind === "downloading") {
      return (
        <PushButton onClick={() => void runCheck()} disabled>
          Downloading…
        </PushButton>
      )
    }
    if (state.kind === "installing") {
      return (
        <PushButton onClick={() => void runCheck()} disabled>
          Installing…
        </PushButton>
      )
    }
    if (state.kind === "failed" && state.operation === "install" && state.version) {
      const version = state.version
      return <PushButton onClick={() => void runInstall(version)}>Try install again</PushButton>
    }
    if (!supported) {
      return (
        <PushButton onClick={() => void runCheck()} disabled>
          Check for updates
        </PushButton>
      )
    }
    return (
      <PushButton onClick={() => void runCheck()} disabled={busy}>
        {state.kind === "checking" ? "Checking…" : "Check for updates"}
      </PushButton>
    )
  })()

  return (
    <SectionGroup title="Updates">
      <Card>
        <Row
          label="Software update"
          description={
            supported
              ? undefined
              : simulationActive
                ? "This local simulation does not download or modify the application."
                : "In-app updates are unavailable in this build. Use a signed macOS or Windows release, or the Linux AppImage."
          }
          trailing={action}
        >
          <div
            className={(supported || simulationActive) && state.kind !== "idle" ? "mt-1.5" : ""}
            role="status"
            aria-live="polite"
            aria-atomic="true"
          >
            {(supported || simulationActive) &&
              state.kind !== "idle" &&
              state.kind !== "downloading" && <CheckStatus state={state} />}
          </div>
          {(supported || simulationActive) && state.kind === "downloading" && (
            <div className="mt-1.5">
              <CheckStatus state={state} />
            </div>
          )}
        </Row>
        {debugBuild && (
          <Row
            label="Updater simulator"
            description="Runs the update interface with fixed local data and no application changes."
            trailing={
              <PushButton onClick={() => void runSimulation()} disabled={busy}>
                {simulationActive ? "Restart simulation" : "Start simulation"}
              </PushButton>
            }
          />
        )}
        {supported ? (
          <ToggleRow
            label="Check for updates automatically"
            description={
              lastAutomatic
                ? `A moment after launch and every six hours. antiburn contacts the release feed for this check and nothing else; last checked ${relativeTime(lastAutomatic)}.`
                : "A moment after launch and every six hours. antiburn contacts the release feed for this check and nothing else — nothing about your sessions is ever sent."
            }
            checked={settings.autoUpdate}
            onChange={(next) => void update({ autoUpdate: next })}
          />
        ) : (
          <Row
            label="Automatic updates"
            // Not a disabled switch: there is no automatic behaviour in this
            // build to turn on or off, so there is nothing to render a
            // control for.
            description="This build has no working in-app updater, so it never contacts the release feed."
          />
        )}
      </Card>
    </SectionGroup>
  )
}
