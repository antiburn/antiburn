import { Check } from "lucide-react"
import { useCallback, useId, useState, useSyncExternalStore } from "react"

import { PushButton } from "../../components/ui/PushButton"
import {
  connectionLabel,
  contextLimitError,
  defaultConnection,
  SMART_CHECK_PROVIDERS,
  type Connection,
  type SmartCheckProvider,
} from "../../lib/smartCheckProviders"
import { type SettingsControlId } from "../../lib/settingsSearchTargets"
import {
  ACTIVE_CONNECTION_UNCONFIRMED,
  providerFieldTarget,
  pendingOperationMessage,
  ProviderSettingsSession,
} from "./ProviderSettingsSession"
import { ConnectionDialog, isConnectionFocusTarget } from "./ConnectionDialog"
import { ConnectionActions } from "./ConnectionActions"
import { OllamaModelPicker } from "./OllamaModelPicker"
import { SettingsField } from "./SettingsSearchRows"

const inputClass =
  "min-h-[var(--control-height-regular)] w-full min-w-0 rounded-control border border-separator bg-input-fill px-2 py-1 type-body text-label"

function shouldRestoreConnectionFocus(opener: Element | null) {
  const focused = document.activeElement
  return focused === opener || !isConnectionFocusTarget(focused)
}

export function CheckProviderSettings({
  control,
  targetRevision,
  legacyKeySaved = false,
  onSelectionChange,
  onPendingChange,
}: {
  control?: string | null | undefined
  targetRevision?: number | undefined
  legacyKeySaved?: boolean
  onSelectionChange?: ((selected: boolean) => void) | undefined
  onPendingChange?: ((pending: boolean) => void) | undefined
}) {
  const [session] = useState(
    () => new ProviderSettingsSession(onSelectionChange, onPendingChange),
  )
  const addId = useId()
  const retryId = useId()
  const state = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
    session.getSnapshot,
  )
  const unavailable = state.loading || !state.saved || state.busy
  const pending = Boolean(state.saved?.pendingOperation)
  const requested = providerFieldTarget(control)
  const recoveryControl =
    requested?.control ?? (control === "decisionProvider" ? control : undefined)
  const profiles = Object.entries(state.saved?.profiles ?? {})
  const selected = state.saved?.activeId
  const editing = state.saved?.profiles[state.selectedId]
  const title =
    state.editor === "add"
      ? "Add connection"
      : `Edit ${editing && state.saved ? connectionLabel(state.selectedId, editing, state.saved) : "saved"} connection`

  const getFallbackFocus = useCallback(() => {
    const retry = document.getElementById(retryId)
    return isConnectionFocusTarget(retry) ? retry : document.getElementById(addId)
  }, [retryId, addId])

  async function remove(id: string, opener: HTMLButtonElement) {
    const removed = await session.removeConnection(id)
    requestAnimationFrame(() => {
      if (session.getSnapshot().editor || !shouldRestoreConnectionFocus(opener)) return
      const target = removed
        ? document.getElementById(addId)
        : session.getSnapshot().saved?.pendingOperation
          ? getFallbackFocus()
          : isConnectionFocusTarget(opener)
            ? opener
            : getFallbackFocus()
      if (isConnectionFocusTarget(target)) target.focus()
    })
  }
  async function retry() {
    const opener = document.activeElement
    const completed = await session.retryPendingOperation()
    requestAnimationFrame(() => {
      if (!shouldRestoreConnectionFocus(opener)) return
      const dialog = document.querySelector<HTMLDialogElement>("dialog[open]")
      const target =
        !completed && isConnectionFocusTarget(opener)
          ? opener
          : dialog
            ? dialog.querySelector<HTMLElement>(
                "input:not(:disabled), select:not(:disabled), button:not(:disabled)",
              )
            : getFallbackFocus()
      if (isConnectionFocusTarget(target)) target.focus()
    })
  }
  async function selectSavedConnection(id: string) {
    const opener = document.activeElement
    await session.useConnection(id)
    requestAnimationFrame(() => {
      const current = session.getSnapshot()
      if (current.editor || !shouldRestoreConnectionFocus(opener)) return
      const target = current.saved?.pendingOperation
        ? getFallbackFocus()
        : current.activeConfirmed && current.saved?.activeId === id
          ? document.getElementById(`${addId}-${id}-actions`)
          : isConnectionFocusTarget(opener)
            ? opener
            : getFallbackFocus()
      if (isConnectionFocusTarget(target)) target.focus()
    })
  }

  return (
    <div
      aria-busy={state.loading || state.busy}
      ref={() => session.revealTarget(control, targetRevision)}
      onClickCapture={(event) => {
        if (state.editor || !(event.target instanceof Element)) return
        const button = event.target.closest<HTMLButtonElement>("button")
        if (button?.getAttribute("aria-haspopup") !== "menu") button?.focus()
      }}
    >
      <div data-settings-control="decisionConnection" tabIndex={-1} className="px-4 py-3">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <h3 className="type-body font-medium! text-label">Connections</h3>
          <PushButton
            id={addId}
            disabled={unavailable || pending}
            onClick={() => session.add()}
          >
            Add connection
          </PushButton>
        </div>
        {state.loading && (
          <p role="status" className="mt-2 type-footnote text-label-secondary">
            Loading connections…
          </p>
        )}
        {state.saved && profiles.length === 0 && !state.loading && (
          <p className="mt-2 type-callout text-label-secondary">
            Add a connection to set up model-based checks.
          </p>
        )}
        {state.saved && state.activeConfirmed && selected === null && profiles.length > 0 && (
          <p className="mt-2 type-footnote text-label-secondary">
            Choose Use on a connection before running checks.
          </p>
        )}
        <ul className="mt-3 grid grid-cols-[minmax(0,1fr)_auto_auto] items-center gap-x-3 divide-y divide-separator">
          {profiles.map(([id, profile]) => {
            const label = connectionLabel(id, profile, state.saved!)
            const active = state.activeConfirmed && selected === id
            const missing =
              (profile.provider === "jev" || profile.provider === "cloudflare") &&
              (!profile.credential ||
                (profile.credential.kind === "legacy_type_safe" &&
                  (!legacyKeySaved || state.legacyKeyForgotten)))
            return (
              <li
                key={id}
                aria-label={label}
                aria-describedby={active ? `${addId}-${id}-selected` : undefined}
                className="col-span-full grid min-w-0 grid-cols-subgrid items-center py-3"
              >
                <div className="min-w-0">
                  <p className="wrap-anywhere type-body text-label">{label}</p>
                  {missing && (
                    <p className="mt-1 type-footnote text-label-secondary">
                      Add a credential to use this connection.
                    </p>
                  )}
                </div>
                <div className="flex min-w-16 items-center justify-center">
                  {active ? (
                    <span
                      id={`${addId}-${id}-selected`}
                      className="flex items-center gap-1 type-footnote text-label-secondary"
                    >
                      <Check size={12} aria-hidden="true" />
                      Selected
                    </span>
                  ) : (
                    <PushButton
                      disabled={unavailable || pending}
                      ariaLabel={`Use ${label}`}
                      onClick={() => void selectSavedConnection(id)}
                    >
                      Use
                    </PushButton>
                  )}
                </div>
                <ConnectionActions
                  id={`${addId}-${id}-actions`}
                  label={label}
                  disabled={unavailable}
                  pending={pending}
                  onEdit={() => session.openEdit(id)}
                  onRemove={(opener) => void remove(id, opener)}
                />
              </li>
            )
          })}
        </ul>
        {!state.editor && (
          <PendingOperationNotice
            session={session}
            retryId={retryId}
            onRetry={() => void retry()}
          />
        )}
        {!state.loading && !state.saved && (
          <div
            className="mt-2 space-y-2"
            data-settings-control={recoveryControl}
            data-settings-target-pending
            tabIndex={-1}
          >
            {state.error && (
              <p role="alert" className="type-footnote text-system-red-text">
                {state.error}
              </p>
            )}
            <PushButton onClick={() => void session.refresh()}>
              Retry loading connections
            </PushButton>
          </div>
        )}
        {state.saved && !state.activeConfirmed && !state.editor && (
          <div className="mt-2 space-y-2">
            <p role="status" className="type-footnote text-label-secondary">
              {ACTIVE_CONNECTION_UNCONFIRMED}
            </p>
            <PushButton disabled={unavailable} onClick={() => void session.refresh()}>
              Retry loading connections
            </PushButton>
          </div>
        )}
        {state.saved && state.error && state.errorLocation === "connections" && (
          <p role="alert" className="mt-2 wrap-anywhere type-footnote text-system-red-text">
            {state.error}
          </p>
        )}
      </div>
      {state.editor && (
        <ConnectionDialog
          title={title}
          onCancel={() => void session.closeEditor()}
          getFallbackFocus={getFallbackFocus}
        >
          <ConnectionForm
            key={state.selectedId}
            session={session}
            control={control}
            legacyKeySaved={legacyKeySaved && !state.legacyKeyForgotten}
            retryId={retryId}
            onRetry={() => void retry()}
          />
        </ConnectionDialog>
      )}
    </div>
  )
}

function PendingOperationNotice({
  session,
  retryId,
  onRetry,
}: {
  session: ProviderSettingsSession
  retryId: string
  onRetry: () => void
}) {
  const state = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
    session.getSnapshot,
  )
  const operation = state.saved?.pendingOperation
  if (!operation) return null
  return (
    <div className="mt-3 space-y-2">
      <p role="status" className="type-footnote text-label-secondary">
        {pendingOperationMessage(operation, Boolean(state.editor))}
      </p>
      <PushButton id={retryId} disabled={state.busy || state.loading} onClick={onRetry}>
        {state.action === "retry" ? "Retrying…" : "Retry connection change"}
      </PushButton>
    </div>
  )
}

function ConnectionForm({
  session,
  control,
  legacyKeySaved,
  retryId,
  onRetry,
}: {
  session: ProviderSettingsSession
  control?: string | null | undefined
  legacyKeySaved: boolean
  retryId: string
  onRetry: () => void
}) {
  const state = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
    session.getSnapshot,
  )
  const current = state.drafts[state.selectedId]
  const connection = current?.draft.connection ?? defaultConnection("jev")
  const saved = state.saved?.profiles[state.selectedId]
  const savedCredential =
    connection.credential?.kind === "legacy_type_safe"
      ? legacyKeySaved
      : Boolean(saved?.credential)
  const credential = current?.draft.credential ?? ""
  const [replacing, setReplacing] = useState(false)
  const helpId = useId()
  const needsCredential = connection.provider === "jev" || connection.provider === "cloudflare"
  const setupError = contextLimitError(connection)
  const unavailable = state.busy || state.loading
  const disabled =
    unavailable ||
    Boolean(state.saved?.pendingOperation) ||
    !connection.model.trim() ||
    (connection.endpoint.kind !== "provider_default" && !connection.endpoint.value.trim()) ||
    (needsCredential && !credential.trim() && !savedCredential) ||
    Boolean(setupError)

  function edit(update: Partial<Connection>) {
    session.edit({ ...connection, ...update }, credential || null)
  }
  function textField(
    searchId: SettingsControlId,
    value: string,
    onChange: (value: string) => void,
    secret = false,
  ) {
    const showInput =
      !secret || !savedCredential || replacing || control === searchId || Boolean(value)
    return (
      <SettingsField key={searchId} searchId={searchId}>
        {(id) => (
          <>
            {showInput ? (
              <input
                id={id}
                autoFocus={secret && replacing}
                aria-describedby={secret && savedCredential ? helpId : undefined}
                type={secret ? "password" : "text"}
                autoComplete="off"
                spellCheck={false}
                className={inputClass}
                value={value}
                disabled={unavailable}
                onChange={(event) => onChange(event.target.value)}
              />
            ) : (
              <div className="flex flex-wrap items-center gap-2 type-callout text-label-secondary">
                <span>Saved</span>
                <PushButton disabled={unavailable} onClick={() => setReplacing(true)}>
                  Change
                </PushButton>
              </div>
            )}
            {secret && savedCredential && showInput && (
              <p id={helpId} className="type-footnote text-label-secondary">
                Leave empty to keep the saved credential.
              </p>
            )}
            {secret &&
              saved?.credential &&
              (saved.credential.kind !== "legacy_type_safe" || !state.legacyKeyForgotten) && (
                <div className="type-footnote text-label-secondary">
                  <button
                    type="button"
                    className="text-label-secondary underline disabled:opacity-50"
                    disabled={unavailable || Boolean(state.saved?.pendingOperation)}
                    onClick={() => void session.run("remove")}
                  >
                    {saved.credential.kind === "legacy_type_safe"
                      ? "Forget shared TypeSafe key"
                      : "Forget saved credential"}
                  </button>
                </div>
              )}
            {secret &&
              connection.provider === "jev" &&
              state.editor === "add" &&
              legacyKeySaved && (
                <div className="space-y-2 type-footnote text-label-secondary">
                  <p>A shared TypeSafe key is saved on this device.</p>
                  <div className="flex flex-wrap gap-2">
                    <PushButton
                      disabled={
                        unavailable || connection.credential?.kind === "legacy_type_safe"
                      }
                      onClick={() => session.useLegacyKey()}
                    >
                      {connection.credential?.kind === "legacy_type_safe"
                        ? "Saved key selected"
                        : "Use saved key"}
                    </PushButton>
                    <button
                      type="button"
                      className="underline disabled:opacity-50"
                      disabled={unavailable || Boolean(state.saved?.pendingOperation)}
                      onClick={() => void session.forgetLegacyKey()}
                    >
                      Forget shared TypeSafe key
                    </button>
                  </div>
                </div>
              )}
          </>
        )}
      </SettingsField>
    )
  }
  const endpoint = (id: SettingsControlId) =>
    textField(
      id,
      connection.endpoint.kind === "provider_default" ? "" : connection.endpoint.value,
      (value) => {
        if (connection.endpoint.kind !== "provider_default")
          edit({ endpoint: { ...connection.endpoint, value } })
      },
    )
  const key = (id: SettingsControlId) =>
    textField(id, credential, (value) => session.edit(connection, value || null), true)

  return (
    <div aria-busy={unavailable} className="space-y-5 px-4 pt-2 pb-4">
      <div className="space-y-4">
        {state.editor === "add" && (
          <SettingsField searchId="decisionProvider">
            {(id) => (
              <select
                id={id}
                autoFocus
                className={inputClass}
                value={connection.provider}
                disabled={unavailable}
                onChange={(event) =>
                  void session.changeAddProvider(event.target.value as SmartCheckProvider)
                }
              >
                {Object.entries(SMART_CHECK_PROVIDERS).map(([id, provider]) => (
                  <option key={id} value={id}>
                    {provider.label}
                  </option>
                ))}
              </select>
            )}
          </SettingsField>
        )}
        {connection.provider === "jev" && key("typeSafeApiKey")}
        {connection.provider === "ollama" && (
          <>
            {endpoint("ollamaUrl")}
            <OllamaModelPicker session={session} onChange={(model) => edit({ model })} />
            {key("ollamaApiKey")}
          </>
        )}
        {connection.provider === "cloudflare" && (
          <>
            {endpoint("cloudflareAccountId")}
            <SettingsField searchId="cloudflareModel">
              {(id) => (
                <select
                  id={id}
                  className={inputClass}
                  value={connection.model}
                  disabled={unavailable}
                  onChange={(event) => edit({ model: event.target.value })}
                >
                  <option value="clef">Clef</option>
                  <option value="clef-flash">Clef Flash</option>
                </select>
              )}
            </SettingsField>
            {key("cloudflareApiToken")}
          </>
        )}
        {connection.provider === "custom" && (
          <>
            {endpoint("customUrl")}
            {textField("customModel", connection.model, (model) => edit({ model }))}
            <SettingsField searchId="customResponseMode">
              {(id) => (
                <>
                  <select
                    id={id}
                    className={inputClass}
                    value={connection.responseMode}
                    disabled={unavailable}
                    onChange={(event) =>
                      edit({ responseMode: event.target.value as Connection["responseMode"] })
                    }
                  >
                    <option value="direct">Direct System One</option>
                    <option value="cloudflare_envelope">Cloudflare envelope</option>
                  </select>
                  <p className="type-footnote text-label-secondary">
                    Requires a supported System One endpoint. Chat APIs are not supported.
                  </p>
                </>
              )}
            </SettingsField>
            {key("customApiKey")}
          </>
        )}
      </div>
      <div className="space-y-2">
        <PendingOperationNotice session={session} retryId={retryId} onRetry={onRetry} />
        {current?.tested && (
          <p role="status" className="type-footnote text-system-green">
            Connection test passed.
          </p>
        )}
        {state.status && state.status !== ACTIVE_CONNECTION_UNCONFIRMED && (
          <p role="status" className="type-footnote text-label-secondary">
            {state.status}
          </p>
        )}
        {state.error && state.errorLocation === "editor" && (
          <p role="alert" className="wrap-anywhere type-footnote text-system-red-text">
            {state.error}
          </p>
        )}
        {!state.activeConfirmed && (
          <div className="space-y-2">
            <p role="status" className="type-footnote text-label-secondary">
              {ACTIVE_CONNECTION_UNCONFIRMED}
            </p>
            <PushButton
              disabled={state.busy || state.loading}
              onClick={() => void session.refresh()}
            >
              Retry loading connections
            </PushButton>
          </div>
        )}
        {setupError && (
          <p className="wrap-anywhere type-footnote text-label-secondary">{setupError}</p>
        )}
      </div>
      <div className="space-y-2">
        <div className="flex flex-wrap items-center gap-2">
          <PushButton disabled={disabled} onClick={() => void session.run("test")}>
            {state.action === "test" ? "Testing…" : "Test connection"}
          </PushButton>
          <div className="ml-auto flex flex-wrap justify-end gap-2">
            <PushButton disabled={unavailable} onClick={() => void session.closeEditor()}>
              Cancel
            </PushButton>
            <PushButton
              variant="primary"
              disabled={disabled}
              onClick={() => void session.run("save")}
            >
              {state.action === "save"
                ? "Saving…"
                : state.editor === "add"
                  ? "Add connection"
                  : "Save"}
            </PushButton>
          </div>
        </div>
        <p className="type-footnote text-label-secondary">
          Tests use sample data. Provider charges may apply.
        </p>
      </div>
    </div>
  )
}
