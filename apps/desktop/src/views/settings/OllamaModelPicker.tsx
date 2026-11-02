import { useId, useState, useSyncExternalStore } from "react"
import { PushButton } from "../../components/ui/PushButton"
import { OllamaModelSession } from "./OllamaModelSession"
import type { ProviderSettingsSession } from "./ProviderSettingsSession"
import { SettingsField } from "./SettingsSearchRows"
import { modelLabel } from "../../lib/smartCheckProviders"

export function OllamaModelPicker({
  session,
  onChange,
}: {
  session: ProviderSettingsSession
  onChange: (model: string) => void
}) {
  const provider = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
    session.getSnapshot,
  )
  const [lookup] = useState(
    () =>
      new OllamaModelSession(() => {
        const current = session.getSnapshot()
        return current.drafts[current.selectedId]?.draft
      }),
  )
  const state = useSyncExternalStore(lookup.subscribe, lookup.getSnapshot, lookup.getSnapshot)
  const helpId = useId()
  const draft = provider.drafts[provider.selectedId]!.draft
  const connection = draft.connection
  const endpoint =
    connection.endpoint.kind === "provider_default" ? "" : connection.endpoint.value
  const listed = state.models?.some((model) => model.id === connection.model)

  return (
    <div
      ref={(node) => {
        if (node) lookup.bind(draft)
      }}
    >
      <SettingsField searchId="ollamaModel">
        {(id) => (
          <>
            <select
              id={id}
              aria-describedby={helpId}
              className="min-h-[var(--control-height-regular)] w-full min-w-0 rounded-control border border-separator bg-input-fill px-2 py-1 type-body text-label"
              value={connection.model}
              disabled={provider.busy || provider.loading}
              onChange={(event) => onChange(event.target.value)}
            >
              {!connection.model && <option value="">Choose an installed model</option>}
              {connection.model && !listed && (
                <option value={connection.model}>
                  {modelLabel(connection.model)} (current selection)
                </option>
              )}
              {state.models?.map((model) => (
                <option key={model.id} value={model.id}>
                  {model.label}
                </option>
              ))}
            </select>
            <div className="mt-2 space-y-2">
              <PushButton
                disabled={
                  provider.busy || provider.loading || state.loading || !endpoint.trim()
                }
                onClick={() => void lookup.find()}
              >
                {state.loading
                  ? "Finding models…"
                  : state.models
                    ? "Reload models"
                    : "Find models"}
              </PushButton>
              <p id={helpId} className="type-footnote text-label-secondary">
                Choose an installed local model. Test the connection to check model support.
              </p>
              {state.loading && (
                <p role="status" className="type-footnote text-label-secondary">
                  Reading installed models…
                </p>
              )}
              {state.models?.length === 0 && !state.loading && !state.error && (
                <p role="status" className="type-footnote text-label-secondary">
                  No local models found on this server.
                </p>
              )}
              {connection.model && state.models && !listed && (
                <p className="wrap-anywhere type-footnote text-label-secondary">
                  The current model is not in this list. Your selection has not changed.
                </p>
              )}
              {state.error && (
                <p role="alert" className="type-footnote text-system-red-text">
                  {state.error}
                </p>
              )}
              {state.error && state.models && (
                <p className="type-footnote text-label-secondary">
                  The previous list may be out of date.
                </p>
              )}
            </div>
          </>
        )}
      </SettingsField>
    </div>
  )
}
