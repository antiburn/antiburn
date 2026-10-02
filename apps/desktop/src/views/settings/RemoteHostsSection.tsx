import { Tooltip } from "../../components/presentation/Tooltip"
import { StatusText } from "../../components/ui/StatusText"
import { ToggleSwitch } from "../../components/ui/ToggleSwitch"
import * as DropdownMenu from "@radix-ui/react-dropdown-menu"
import { Ellipsis, ExternalLink, Monitor, Plus } from "lucide-react"
import { useRef, useState } from "react"
import { createPortal } from "react-dom"

import { Card } from "../../components/ui/Card"
import { PushButton } from "../../components/ui/PushButton"
import {
  openRemoteHelperDownloads,
  REMOTE_HOST_LIMIT,
  remoteHostLabel,
  type RemoteHost,
  type RemoteHostPreflight,
  type RemoteHostsSnapshot,
  type RemoteSyncIntervalSecs,
} from "../../lib/remoteHosts"
import { agentDisplayName } from "../../lib/presentation/agents"
import { relativeTime } from "../../lib/presentation/relativeTime"
import { SettingsRow, SettingsSectionGroup } from "./SettingsSearchRows"
import type { SourcesSession } from "./SourcesSession"
import { makeDialogBackgroundInert, trapDialogFocus } from "./settingsDialog"
import { openMainWindowRemoteHost } from "../../lib/mainWindowIpc"
import "./remote-hosts.css"

const SYNC_OPTIONS: readonly { value: RemoteSyncIntervalSecs; label: string }[] = [
  { value: 60, label: "Every minute" },
  { value: 300, label: "Every 5 minutes" },
  { value: 900, label: "Every 15 minutes" },
  { value: 1800, label: "Every 30 minutes" },
  { value: 3600, label: "Every hour" },
  { value: 0, label: "Manual only" },
]

const ACTION_TARGET = "relative before:absolute before:inset-x-0 before:-inset-y-2.5"

function lastSyncLabel(epoch: number | null): string {
  return epoch == null
    ? "Never synced"
    : `Synced ${relativeTime(new Date(epoch * 1000).toISOString())}`
}

function SetupInstructions({
  appVersion,
  preflight,
  onOpenDownloads,
}: {
  appVersion: string
  preflight: RemoteHostPreflight
  onOpenDownloads: () => void
}) {
  const [chosenArchitecture, setChosenArchitecture] = useState<"x86_64" | "aarch64" | null>(
    null,
  )
  const architecture = preflight.architecture ?? chosenArchitecture
  const target =
    architecture === "aarch64" ? "aarch64-unknown-linux-musl" : "x86_64-unknown-linux-musl"
  const archive = `antiburn-remote-${appVersion}-${target}.tar.gz`
  const directory = archive.slice(0, -".tar.gz".length)
  return (
    <div className="mt-4 rounded-control bg-surface-secondary p-3">
      <p className="type-callout text-label">Install the remote helper</p>
      <p className="mt-1 type-footnote text-label-secondary">
        This small command-line executable runs only when Antiburn connects. It does not install
        a desktop app or persistent service.
      </p>
      {preflight.architecture == null ? (
        <fieldset className="mt-3">
          <legend className="type-footnote text-label-secondary">
            Choose the Linux host architecture
          </legend>
          <div className="mt-2 flex gap-4 type-footnote text-label">
            <label className="flex items-center gap-1.5">
              <input
                type="radio"
                name="remote-helper-architecture"
                value="x86_64"
                checked={chosenArchitecture === "x86_64"}
                onChange={() => setChosenArchitecture("x86_64")}
              />
              x64
            </label>
            <label className="flex items-center gap-1.5">
              <input
                type="radio"
                name="remote-helper-architecture"
                value="aarch64"
                checked={chosenArchitecture === "aarch64"}
                onChange={() => setChosenArchitecture("aarch64")}
              />
              ARM64
            </label>
          </div>
        </fieldset>
      ) : null}
      {architecture ? (
        <>
          <p className="mt-3 type-footnote text-label-secondary">
            Download <span className="font-mono text-label">{archive}</span> and{" "}
            <span className="font-mono text-label">SHA256SUMS</span> from the official Releases
            page. In the download directory, verify and install it with:
          </p>
          <pre className="mt-2 overflow-x-auto rounded-control bg-surface-card p-2 type-footnote text-label">
            {`sha256sum --check --ignore-missing SHA256SUMS\ntar -xzf ${archive}\nmkdir -p ~/.local/bin\ninstall -m 755 ${directory}/antiburn-remote ~/.local/bin/antiburn-remote`}
          </pre>
        </>
      ) : (
        <p className="mt-3 type-footnote text-label-secondary">
          Select x64 or ARM64 to see the exact download and verified install commands.
        </p>
      )}
      <button
        type="button"
        onClick={onOpenDownloads}
        className="mt-2 inline-flex items-center gap-1 type-footnote text-accent hover:underline"
      >
        Open official Releases <ExternalLink size={11} aria-hidden="true" />
      </button>
    </div>
  )
}

function RemoteHostEditor({
  host,
  session,
  appVersion,
  onClose,
}: {
  host: RemoteHost | null
  session: SourcesSession
  appVersion: string
  onClose: () => void
}) {
  const [displayName, setDisplayName] = useState(host?.displayName ?? "")
  const [sshAlias, setSshAlias] = useState(host?.sshAlias ?? "")
  const [preflight, setPreflight] = useState<RemoteHostPreflight | null>(null)
  const [preflightAlias, setPreflightAlias] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [message, setMessage] = useState<string | null>(null)
  const name = displayName.trim()
  const alias = sshAlias.trim()
  const currentPreflight = preflightAlias === alias ? preflight : null
  const dirty = name !== (host?.displayName ?? "") || alias !== (host?.sshAlias ?? "")
  const aliasChanged = alias !== (host?.sshAlias ?? "")

  function requestClose() {
    if (dirty) {
      setMessage("Use Cancel to discard these changes.")
      return
    }
    onClose()
  }

  async function check() {
    if (!alias) {
      setMessage("Enter an SSH host alias.")
      return null
    }
    const checkedAlias = alias
    setPreflightAlias(checkedAlias)
    setBusy(true)
    setMessage(null)
    try {
      const result = await session.checkRemoteHost(checkedAlias)
      setPreflight(result)
      if (result.status !== "ready") setMessage(result.message || "Could not use this host.")
      return result
    } catch {
      setMessage("Could not check this host. Verify the SSH alias and try again.")
      return null
    } finally {
      setBusy(false)
    }
  }

  async function save() {
    if (!alias) {
      setMessage("Enter an SSH host alias.")
      return
    }
    setBusy(true)
    setMessage(null)
    try {
      if (host && !aliasChanged) {
        await session.updateRemoteHost(host.id, host.sshAlias, name || null)
        onClose()
        return
      }
      const result =
        currentPreflight?.status === "ready"
          ? currentPreflight
          : await session.checkRemoteHost(alias)
      setPreflightAlias(alias)
      setPreflight(result)
      if (result.status !== "ready") {
        setMessage(result.message || "Could not use this host.")
        return
      }
      if (host) await session.updateRemoteHost(host.id, alias, name || null)
      else await session.addRemoteHost(alias, name || null)
      onClose()
    } catch {
      setMessage("Could not save this host. Check the details and try again.")
    } finally {
      setBusy(false)
    }
  }

  const ready = currentPreflight?.status === "ready"
  const helperMissing = currentPreflight?.status === "helperMissing"
  const checking = !host && !ready
  const actionLabel = busy
    ? checking
      ? "Checking…"
      : "Saving…"
    : checking
      ? helperMissing
        ? "Check again"
        : "Check connection"
      : host
        ? "Save changes"
        : "Add host"
  const titleId = `remote-host-${host ? "edit" : "add"}-title`
  return createPortal(
    <div
      ref={makeDialogBackgroundInert}
      className="fixed inset-0 z-50 flex items-center justify-center bg-surface-window/80 p-6 backdrop-blur-sm"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) requestClose()
      }}
    >
      <section
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-busy={busy}
        onKeyDown={(event) => trapDialogFocus(event, requestClose)}
        className="max-h-[calc(100vh-3rem)] w-full max-w-md overflow-y-auto rounded-control border border-separator bg-surface-overlay p-5 text-label shadow-raised"
      >
        <h3 id={titleId} className="type-title-3 text-label">
          {host ? "Edit remote host" : "Add remote host"}
        </h3>
        <p className="mt-2 type-footnote text-label-secondary">
          Currently supports Linux hosts with Claude Code and Codex sessions. Use an existing
          SSH alias configured for key authentication.
        </p>
        <label className="mt-4 block type-footnote text-label-secondary">
          Name <span className="text-label-tertiary">(optional)</span>
          <input
            autoFocus
            disabled={busy}
            value={displayName}
            onChange={(event) => setDisplayName(event.target.value)}
            className="mt-1 h-[var(--control-height-regular)] w-full rounded-control border border-separator bg-input-fill px-2 type-body text-label"
            placeholder="Build server"
          />
        </label>
        <label className="mt-3 block type-footnote text-label-secondary">
          SSH host alias
          <input
            disabled={busy}
            value={sshAlias}
            onChange={(event) => {
              setSshAlias(event.target.value)
              setPreflight(null)
              setPreflightAlias(null)
              setMessage(null)
            }}
            className="mt-1 h-[var(--control-height-regular)] w-full rounded-control border border-separator bg-input-fill px-2 font-mono type-body text-label"
            placeholder="my-linux-host"
            aria-invalid={!!message && !alias}
          />
        </label>
        {ready && currentPreflight ? (
          <p role="status" className="mt-3 type-footnote text-label-secondary">
            Linux {currentPreflight.architecture === "aarch64" ? "ARM64" : "x64"}
            {currentPreflight.helperVersion
              ? ` · Helper ${currentPreflight.helperVersion}`
              : ""}{" "}
            · Found{" "}
            {currentPreflight.supportedAgents.map(agentDisplayName).join(" and ") ||
              "supported agents"}
          </p>
        ) : null}
        {helperMissing && currentPreflight ? (
          <SetupInstructions
            appVersion={appVersion}
            preflight={currentPreflight}
            onOpenDownloads={() => void openRemoteHelperDownloads()}
          />
        ) : null}
        {message ? (
          <p role="alert" className="mt-3 type-footnote text-system-red-text">
            {message}
          </p>
        ) : null}
        <div className="mt-5 flex justify-end gap-2">
          <PushButton onClick={onClose} disabled={busy} className={ACTION_TARGET}>
            Cancel
          </PushButton>
          <PushButton
            onClick={() => void (checking ? check() : save())}
            disabled={busy || !alias || (!checking && !dirty)}
            variant="primary"
            className={ACTION_TARGET}
          >
            <span className="inline-grid">
              <span aria-hidden="true" className="invisible col-start-1 row-start-1">
                {host ? "Save changes" : "Check connection"}
              </span>
              <span className="col-start-1 row-start-1">{actionLabel}</span>
            </span>
          </PushButton>
        </div>
      </section>
    </div>,
    document.body,
  )
}

function RemoveRemoteHostDialog({
  host,
  session,
  onClose,
}: {
  host: RemoteHost
  session: SourcesSession
  onClose: () => void
}) {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState(false)
  return createPortal(
    <div
      ref={makeDialogBackgroundInert}
      className="fixed inset-0 z-50 flex items-center justify-center bg-surface-window/80 p-6 backdrop-blur-sm"
    >
      <section
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="remove-remote-host-title"
        onKeyDown={(event) => trapDialogFocus(event, onClose)}
        className="w-full max-w-md rounded-control border border-separator bg-surface-overlay p-5 text-label shadow-raised"
      >
        <h3 id="remove-remote-host-title" className="type-title-3">
          Remove {remoteHostLabel(host)}?
        </h3>
        <p className="mt-2 type-body text-label-secondary">
          This removes this computer’s cached transcripts and analysis from this Mac. Originals
          on the remote computer stay unchanged.
        </p>
        {error ? (
          <p role="alert" className="mt-3 type-footnote text-system-red-text">
            Could not remove this host. Try again.
          </p>
        ) : null}
        <div className="mt-5 flex justify-end gap-2">
          <PushButton onClick={onClose} disabled={busy} autoFocus>
            Cancel
          </PushButton>
          <button
            type="button"
            disabled={busy}
            onClick={() => {
              setBusy(true)
              setError(false)
              void session
                .removeRemoteHost(host.id)
                .then(onClose)
                .catch(() => {
                  setBusy(false)
                  setError(true)
                })
            }}
            className="ui-push-button border-transparent bg-system-red text-white disabled:opacity-50"
          >
            {busy ? "Removing…" : "Remove"}
          </button>
        </div>
      </section>
    </div>,
    document.body,
  )
}

function RemoteHostRow({
  host,
  isSyncing,
  session,
  onEdit,
  onRemove,
}: {
  host: RemoteHost
  isSyncing: boolean
  session: SourcesSession
  onEdit: (trigger: HTMLElement) => void
  onRemove: (trigger: HTMLElement) => void
}) {
  const [savingSync, setSavingSync] = useState(false)
  const [syncSettingError, setSyncSettingError] = useState(false)
  const [starting, setStarting] = useState(false)
  const [scanError, setScanError] = useState(false)
  const [navigationError, setNavigationError] = useState(false)
  const scanning = host.automaticSyncEnabled && (starting || isSyncing)
  const failed = host.status === "error"
  const actionsTrigger = useRef<HTMLButtonElement>(null)
  return (
    <div className="remote-host-row px-4 py-3">
      <Monitor
        size={14}
        className="remote-host-icon shrink-0 text-label-secondary"
        aria-hidden="true"
      />
      <p className="remote-host-name min-w-0 truncate type-body text-label">
        {remoteHostLabel(host)}
      </p>
      <p className="remote-host-metadata flex flex-wrap items-center gap-x-1 type-footnote tabular-nums text-label-secondary">
        {remoteHostLabel(host) !== host.sshAlias ? (
          <span className="remote-host-alias inline-flex h-[var(--space-lg)] max-w-full shrink-0 items-center justify-center rounded-full bg-surface-tertiary/40 px-1 font-mono font-medium! type-caption text-label-tertiary">
            <span className="truncate" title={host.sshAlias}>
              {host.sshAlias}
            </span>
          </span>
        ) : null}
        <button
          type="button"
          className="remote-host-sessions relative text-label underline decoration-label/40 underline-offset-2 disabled:text-label-secondary disabled:no-underline"
          aria-label={`Browse sessions from ${remoteHostLabel(host)} in the current date range`}
          title="Browse this host’s sessions. Your Sessions date range still applies."
          disabled={host.cachedSessionCount === 0}
          onClick={async () => {
            setNavigationError(false)
            try {
              await openMainWindowRemoteHost(host.id)
            } catch {
              setNavigationError(true)
            }
          }}
        >
          {host.cachedSessionCount} {host.cachedSessionCount === 1 ? "session" : "sessions"}
        </button>
        <span aria-hidden="true">·</span>
        {host.automaticSyncEnabled ? (
          <StatusText tone="secondary">
            {lastSyncLabel(host.lastSuccessfulSyncEpoch)}
          </StatusText>
        ) : (
          <Tooltip label={lastSyncLabel(host.lastSuccessfulSyncEpoch)}>
            <span tabIndex={0}>
              Sync off
              <span className="sr-only">. {lastSyncLabel(host.lastSuccessfulSyncEpoch)}</span>
            </span>
          </Tooltip>
        )}
      </p>
      <div className="remote-host-actions">
        <PushButton
          className={"remote-host-sync justify-center"}
          onClick={async () => {
            setStarting(true)
            setScanError(false)
            try {
              await session.scanRemoteHost(host.id)
            } catch {
              setScanError(true)
            } finally {
              setStarting(false)
            }
          }}
          disabled={scanning || !host.automaticSyncEnabled || savingSync}
        >
          {scanning ? "Syncing…" : failed || scanError ? "Retry" : "Sync now"}
        </PushButton>
        <DropdownMenu.Root modal={false}>
          <DropdownMenu.Trigger asChild>
            <button
              ref={actionsTrigger}
              type="button"
              aria-label={`More actions for ${remoteHostLabel(host)}`}
              className="ui-push-button remote-host-overflow relative flex items-center justify-center"
            >
              <Ellipsis size={14} aria-hidden="true" />
            </button>
          </DropdownMenu.Trigger>
          <DropdownMenu.Portal>
            <DropdownMenu.Content className="ui-menu min-w-32" align="end" sideOffset={4}>
              <DropdownMenu.Item
                className="ui-menu-item"
                onSelect={() => {
                  if (actionsTrigger.current) onEdit(actionsTrigger.current)
                }}
              >
                Edit
              </DropdownMenu.Item>
              <DropdownMenu.Item
                className="ui-menu-item text-system-red-text"
                onSelect={() => {
                  if (actionsTrigger.current) onRemove(actionsTrigger.current)
                }}
              >
                Remove
              </DropdownMenu.Item>
            </DropdownMenu.Content>
          </DropdownMenu.Portal>
        </DropdownMenu.Root>
        <label
          className="remote-host-toggle"
          title="Turn off syncing for this host without removing saved sessions. Turn it on to sync again."
        >
          <ToggleSwitch
            aria-label={`Sync for ${remoteHostLabel(host)}`}
            checked={host.automaticSyncEnabled}
            disabled={savingSync}
            onCheckedChange={async (enabled) => {
              setSavingSync(true)
              setSyncSettingError(false)
              try {
                await session.setRemoteHostSyncEnabled(host.id, enabled)
              } catch {
                setSyncSettingError(true)
              } finally {
                setSavingSync(false)
              }
            }}
          />
        </label>
      </div>
      {syncSettingError ? (
        <p role="alert" className="remote-host-message type-footnote text-system-red-text">
          Couldn’t change syncing. Your previous setting is unchanged. Try again.
        </p>
      ) : null}
      {scanError ? (
        <p role="alert" className="remote-host-message type-footnote text-system-red-text">
          Couldn’t start sync. Your saved sessions are still available. Try again.
        </p>
      ) : null}
      {navigationError ? (
        <p role="alert" className="remote-host-message type-footnote text-system-red-text">
          Couldn’t open Sessions. Try again.
        </p>
      ) : null}
      {failed ? (
        <p role="status" className="remote-host-message type-footnote text-system-red-text">
          Last sync failed{host.lastError?.message ? ` · ${host.lastError.message}` : ""}
        </p>
      ) : null}
    </div>
  )
}

export function RemoteHostsSection({
  remote,
  session,
  appVersion,
}: {
  remote: RemoteHostsSnapshot
  session: SourcesSession
  appVersion: string
}) {
  const [editor, setEditor] = useState<{
    host: RemoteHost | "new"
    restoreFocus: HTMLElement
  } | null>(null)
  const [removing, setRemoving] = useState<{
    host: RemoteHost
    restoreFocus: HTMLElement
  } | null>(null)
  const atLimit = remote.hosts.length >= REMOTE_HOST_LIMIT
  const [savingInterval, setSavingInterval] = useState(false)
  const [intervalError, setIntervalError] = useState(false)

  function restoreFocus(target: HTMLElement) {
    queueMicrotask(() => {
      const fallback = document.getElementById("add-remote-host")
      ;(target.isConnected ? target : fallback)?.focus()
    })
  }

  function closeEditor() {
    if (!editor) return
    const target = editor.restoreFocus
    setEditor(null)
    restoreFocus(target)
  }

  function closeRemoveDialog() {
    if (!removing) return
    const target = removing.restoreFocus
    setRemoving(null)
    restoreFocus(target)
  }
  return (
    <>
      <SettingsSectionGroup searchId="sourceRemoteHosts">
        <Card className="remote-hosts-card">
          <div>
            <p className="px-4 pt-3 type-footnote text-label-secondary">
              Sync sessions from your other computers over SSH and browse them alongside your
              local sessions. Synced sessions are saved on this computer so you can view them
              offline.
            </p>
            <SettingsRow
              searchId="sourceAutomaticSync"
              className="remote-sync-row"
              description={
                remote.sync.intervalSecs === 0
                  ? "Use Sync now on a host to update its sessions."
                  : "Enabled hosts sync automatically while Antiburn is running."
              }
              trailing={
                <select
                  aria-label="Sync frequency"
                  value={remote.sync.intervalSecs}
                  disabled={savingInterval}
                  aria-describedby={intervalError ? "remote-sync-save-error" : undefined}
                  onChange={async (event) => {
                    const seconds = Number(event.target.value) as RemoteSyncIntervalSecs
                    setSavingInterval(true)
                    setIntervalError(false)
                    try {
                      await session.setRemoteSyncInterval(seconds)
                    } catch {
                      setIntervalError(true)
                    } finally {
                      setSavingInterval(false)
                    }
                  }}
                  className="h-[var(--control-height-regular)] rounded-control border border-separator bg-input-fill px-2 type-footnote text-label"
                >
                  {SYNC_OPTIONS.map((option) => (
                    <option key={option.value} value={option.value}>
                      {option.label}
                    </option>
                  ))}
                </select>
              }
            />
            {intervalError ? (
              <p
                id="remote-sync-save-error"
                role="alert"
                className="px-4 pb-3 type-footnote text-system-red-text"
              >
                Couldn’t save the sync interval. Still set to{" "}
                {SYNC_OPTIONS.find(
                  (option) => option.value === remote.sync.intervalSecs,
                )?.label.toLowerCase()}
                . Try again.
              </p>
            ) : null}
          </div>
          {remote.loading && !remote.loaded ? (
            <p role="status" className="px-4 py-3 type-footnote text-label-secondary">
              Loading remote hosts…
            </p>
          ) : remote.hosts.length === 0 ? (
            <p className="px-4 py-3 type-footnote text-label-secondary">
              No remote hosts added.
            </p>
          ) : (
            remote.hosts.map((host) => (
              <RemoteHostRow
                key={host.id}
                host={host}
                isSyncing={remote.sync.active?.hostId === host.id}
                session={session}
                onEdit={(trigger) => {
                  setEditor({ host, restoreFocus: trigger })
                }}
                onRemove={(trigger) => {
                  setRemoving({ host, restoreFocus: trigger })
                }}
              />
            ))
          )}
          <div className="flex flex-wrap items-center gap-3 px-4 py-3">
            <PushButton
              onClick={() => {
                const trigger = document.activeElement
                if (!(trigger instanceof HTMLElement)) return
                setEditor({ host: "new", restoreFocus: trigger })
              }}
              disabled={atLimit}
              className={`${ACTION_TARGET} shrink-0 gap-1.5`}
              ariaLabel={atLimit ? "Eight-host limit reached" : "Add host"}
              id="add-remote-host"
            >
              <Plus size={12} aria-hidden="true" /> Add host
            </PushButton>
            {atLimit ? (
              <p className="type-footnote text-label-tertiary">Eight-host limit reached.</p>
            ) : null}
          </div>
        </Card>
      </SettingsSectionGroup>
      {editor ? (
        <RemoteHostEditor
          key={editor.host === "new" ? "new" : editor.host.id}
          host={editor.host === "new" ? null : editor.host}
          session={session}
          appVersion={appVersion}
          onClose={closeEditor}
        />
      ) : null}
      {removing ? (
        <RemoveRemoteHostDialog
          host={removing.host}
          session={session}
          onClose={closeRemoveDialog}
        />
      ) : null}
    </>
  )
}
