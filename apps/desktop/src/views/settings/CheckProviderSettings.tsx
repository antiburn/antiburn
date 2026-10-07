import { useState, useSyncExternalStore } from "react"
import { ChevronDown } from "lucide-react"
import { Card } from "../../components/ui/Card"
import { PushButton } from "../../components/ui/PushButton"
import { SectionGroup } from "../../components/ui/SectionGroup"
import {
  SMART_CHECK_PROVIDERS,
  type CapabilitySource,
  type Connection,
  type ModelCapabilities,
} from "../../lib/smartCheckProviders"
import { ProviderSettingsSession } from "./ProviderSettingsSession"
import { StepSettingsRow as SettingsRow } from "../main-window/overview/stepSettings/StepSettingsSearchRows"

const inputClass =
  "mt-2 min-h-[var(--control-height-regular)] w-full rounded-control border border-separator bg-input-fill px-3 type-body text-label"
const sourceLabels: Record<CapabilitySource, string> = {
  provider_metadata: "Provider metadata",
  runtime_metadata: "Loaded model metadata",
  documented_default: "Documented default",
  manual: "Manual limit",
  unknown: "Unknown",
}

function CapabilityDetails({ capabilities }: { capabilities: ModelCapabilities }) {
  const limits = [
    ["Total input tokens", capabilities.total_input_tokens],
    ["State plus longest question tokens", capabilities.state_and_longest_question_tokens],
    ["Loaded context tokens", capabilities.runtime_context_tokens],
    ["Request body bytes", capabilities.request_body_bytes],
    ["Questions per request", capabilities.questions_per_request],
  ] as const
  return (
    <div className="mt-3 space-y-1 type-footnote text-label-secondary">
      {limits.map(([label, limit]) => (
        <p key={label}>
          {label}: {limit.value?.toLocaleString() ?? "Unknown"} · {sourceLabels[limit.source]}
        </p>
      ))}
      <p>Rendering reserve: {capabilities.rendering_reserve_tokens.toLocaleString()} tokens</p>
      <p>
        Token counting:{" "}
        {capabilities.tokenizer?.kind === "exact" ? "Exact tokenizer" : "Conservative estimate"}
      </p>
      {capabilities.model_revision && <p>Model revision: {capabilities.model_revision}</p>}
    </div>
  )
}

export function CheckProviderSettings({
  control,
  targetRevision = 0,
  legacyKeySaved = false,
}: {
  control?: string | null | undefined
  targetRevision?: number | undefined
  legacyKeySaved?: boolean
}) {
  const [session] = useState(() => new ProviderSettingsSession())
  const state = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
    session.getSnapshot,
  )
  const [disclosure, setDisclosure] = useState({ open: false, dismissedRevision: -1 })
  const searchOpen =
    control === "smartCheckLimits" && disclosure.dismissedRevision !== targetRevision
  const open = disclosure.open || searchOpen
  const current = state.drafts[state.selectedId]
  const connection = current?.draft.connection
  const provider = connection?.provider ?? "jev"
  const savedConnection = state.saved?.profiles[state.selectedId]
  const hasSavedCredential =
    savedConnection?.credential?.kind === "legacy_type_safe"
      ? legacyKeySaved
      : Boolean(savedConnection?.credential)
  const credential = current?.draft.credential ?? ""
  const metadata = SMART_CHECK_PROVIDERS[provider]
  function edit(update: Partial<Connection>) {
    if (connection)
      session.edit(
        { ...connection, ...update, revision: connection.revision + 1, modelRevision: null },
        current.draft.credential,
      )
  }
  function limits(
    field: "totalInputTokens" | "stateAndLongestQuestionTokens" | "runtimeContextTokens",
    text: string,
  ) {
    edit({
      contextOverride: {
        totalInputTokens: null,
        stateAndLongestQuestionTokens: null,
        runtimeContextTokens: null,
        ...connection?.contextOverride,
        [field]: text === "" ? null : Number(text),
      },
    })
  }
  const manualInvalid =
    connection?.contextOverride &&
    Object.values(connection.contextOverride).some(
      (value) => value !== null && (!Number.isInteger(value) || value < 1 || value > 65536),
    )
  const requiredMissing =
    !connection ||
    !connection.model.trim() ||
    (connection.endpoint.kind !== "provider_default" && !connection.endpoint.value.trim())
  const disabled = state.busy || Boolean(manualInvalid) || requiredMissing
  const needsCredential =
    (provider === "jev" || provider === "cloudflare") &&
    !credential.trim() &&
    !hasSavedCredential

  return (
    <SectionGroup title="Decision model">
      <Card>
        <SettingsRow searchId="smartCheckProvider" label="Provider">
          <div className="space-y-3">
            <select
              aria-label="Smart check provider"
              className={inputClass}
              value={provider}
              disabled={state.busy || !state.saved}
              onChange={(event) => {
                const value = event.target.value
                if (
                  value === "jev" ||
                  value === "ollama" ||
                  value === "cloudflare" ||
                  value === "custom"
                )
                  session.selectProvider(value)
              }}
            >
              {Object.entries(SMART_CHECK_PROVIDERS).map(([id, entry]) => (
                <option key={id} value={id}>
                  {entry.label}
                </option>
              ))}
            </select>
            {state.saved && Object.keys(state.saved.profiles).length > 1 && (
              <label className="block type-body text-label">
                Saved connections
                <select
                  aria-label="Saved connections"
                  className={inputClass}
                  value={savedConnection ? state.selectedId : ""}
                  disabled={state.busy}
                  onChange={(event) => session.select(event.target.value)}
                >
                  <option value="" disabled>
                    Unsaved connection
                  </option>
                  {Object.entries(state.saved.profiles).map(([id, profile]) => (
                    <option key={id} value={id}>
                      {SMART_CHECK_PROVIDERS[profile.provider].label} · {profile.model} · {id}
                      {id === state.saved?.activeId ? " · In use" : ""}
                    </option>
                  ))}
                </select>
              </label>
            )}
            {connection && provider !== "jev" && (
              <>
                <label className="block type-body text-label">
                  {metadata.endpointLabel}
                  <input
                    aria-label={metadata.endpointLabel ?? undefined}
                    className={inputClass}
                    disabled={state.busy}
                    value={
                      connection.endpoint.kind === "provider_default"
                        ? ""
                        : connection.endpoint.value
                    }
                    onChange={(event) =>
                      connection.endpoint.kind !== "provider_default" &&
                      edit({
                        endpoint: { ...connection.endpoint, value: event.target.value },
                      })
                    }
                  />
                </label>
                <p className="type-footnote text-label-secondary">
                  {provider === "ollama"
                    ? "Adds /v1/systemone to this URL."
                    : provider === "custom"
                      ? "System One endpoint; uses this exact URL."
                      : "Your Workers AI account ID."}
                </p>
                <label className="block type-body text-label">
                  Model
                  {provider === "cloudflare" ? (
                    <select
                      aria-label="Provider model"
                      className={inputClass}
                      value={connection.model}
                      disabled={state.busy}
                      onChange={(event) => edit({ model: event.target.value })}
                    >
                      <option value="clef">clef</option>
                      <option value="clef-flash">clef-flash</option>
                    </select>
                  ) : (
                    <input
                      aria-label="Provider model"
                      className={inputClass}
                      value={connection.model}
                      disabled={state.busy}
                      onChange={(event) => edit({ model: event.target.value })}
                    />
                  )}
                </label>
                {provider === "custom" && (
                  <label className="block type-body text-label">
                    Response mode
                    <select
                      aria-label="Response mode"
                      className={inputClass}
                      value={connection.responseMode}
                      disabled={state.busy}
                      onChange={(event) =>
                        edit({
                          responseMode:
                            event.target.value === "direct" ? "direct" : "cloudflare_envelope",
                        })
                      }
                    >
                      <option value="direct">Direct System One</option>
                      <option value="cloudflare_envelope">Cloudflare envelope</option>
                    </select>
                  </label>
                )}
              </>
            )}
            {connection && provider === "jev" && (
              <label className="block type-body text-label">
                Model
                <input
                  aria-label="Provider model"
                  className={inputClass}
                  value={connection.model}
                  readOnly
                  disabled={state.busy}
                />
              </label>
            )}
          </div>
        </SettingsRow>
        <SettingsRow
          searchId="typeSafeApiKey"
          label={
            provider === "jev"
              ? "API key"
              : provider === "cloudflare"
                ? "API token"
                : "Credential (optional)"
          }
          description={
            hasSavedCredential
              ? "Leave empty to use the saved credential, or enter a replacement."
              : undefined
          }
        >
          <input
            aria-label={
              provider === "jev"
                ? "TypeSafe API key"
                : provider === "cloudflare"
                  ? "Cloudflare API token"
                  : "Provider credential (optional)"
            }
            type="password"
            autoComplete="off"
            spellCheck={false}
            value={credential}
            disabled={state.busy || !connection}
            placeholder={
              hasSavedCredential ? "Saved credential; enter a replacement" : undefined
            }
            onChange={(event) =>
              connection && session.edit(connection, event.target.value || null)
            }
            className={inputClass}
          />
          <div className="mt-3 flex flex-wrap gap-2">
            <PushButton disabled={disabled} onClick={() => void session.run("test")}>
              Test connection
            </PushButton>
            <PushButton
              variant="primary"
              disabled={disabled || needsCredential}
              onClick={() => void session.run("save")}
            >
              Save connection
            </PushButton>
            {savedConnection && state.saved?.activeId !== state.selectedId && (
              <PushButton disabled={state.busy} onClick={() => void session.run("switch")}>
                Use saved connection
              </PushButton>
            )}
            {hasSavedCredential && (
              <PushButton disabled={state.busy} onClick={() => void session.run("remove")}>
                Remove credential
              </PushButton>
            )}
          </div>
          <p className="mt-2 type-footnote text-label-secondary">
            Tests use synthetic input. Provider charges may apply.
          </p>
          {current?.tested && (
            <p role="status" className="mt-2 type-footnote text-system-green">
              Connection test passed.
            </p>
          )}
          {state.busy && (
            <p role="status" className="mt-2 type-footnote text-label-secondary">
              Working…
            </p>
          )}
          {state.status && !current?.tested && (
            <p role="status" className="mt-2 type-footnote text-label-secondary">
              {state.status}
            </p>
          )}
          {state.error && (
            <p role="alert" className="mt-2 type-footnote text-system-red-text">
              {state.error}
            </p>
          )}
        </SettingsRow>
        <SettingsRow
          searchId="smartCheckLimits"
          trailing={
            <button
              type="button"
              aria-label="Model limits"
              aria-expanded={open}
              aria-controls="check-model-limits"
              className="flex h-6 w-6 items-center justify-center rounded-control text-label-secondary"
              onClick={() => setDisclosure({ open: !open, dismissedRevision: targetRevision })}
            >
              <ChevronDown size={14} aria-hidden className={open ? "rotate-180" : undefined} />
            </button>
          }
        >
          {open && (
            <div id="check-model-limits">
              <p className="mt-2 type-footnote text-label-secondary">
                Manual limits can lower the model's request bounds.
              </p>
              <PushButton
                className="mt-2"
                disabled={disabled}
                onClick={() => void session.run("refresh")}
              >
                Refresh model limits
              </PushButton>
              {connection && provider !== "jev" && (
                <div className="mt-3 space-y-2">
                  {(
                    [
                      ["totalInputTokens", "Manual total input tokens"],
                      [
                        "stateAndLongestQuestionTokens",
                        "Manual state plus longest question tokens",
                      ],
                      ["runtimeContextTokens", "Manual loaded context tokens"],
                    ] as const
                  ).map(([field, label]) => (
                    <label key={field} className="block type-body text-label">
                      {label}
                      <input
                        aria-label={label}
                        type="number"
                        min={1}
                        max={65536}
                        step={1}
                        disabled={state.busy}
                        value={connection.contextOverride?.[field] ?? ""}
                        onChange={(event) => limits(field, event.target.value)}
                        className={inputClass}
                      />
                    </label>
                  ))}
                  {manualInvalid && (
                    <p role="alert" className="type-footnote text-system-red-text">
                      Use whole token limits from 1 to 65,536, or leave them empty.
                    </p>
                  )}
                </div>
              )}
              {current?.capabilities && (
                <CapabilityDetails capabilities={current.capabilities} />
              )}
            </div>
          )}
        </SettingsRow>
      </Card>
      <p className="mt-3 type-footnote text-label-secondary">
        Checks send selected session content and skill descriptions to the provider in use,
        excluding private thinking. Data stays local only when inference runs on this device.
      </p>
    </SectionGroup>
  )
}
