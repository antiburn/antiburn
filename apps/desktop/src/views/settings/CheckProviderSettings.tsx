import { useState, useSyncExternalStore } from "react"

import { Card } from "../../components/ui/Card"
import { Disclosure } from "../../components/ui/Disclosure"
import { PushButton } from "../../components/ui/PushButton"
import { SectionGroup } from "../../components/ui/SectionGroup"
import { defaultConnection } from "../../lib/smartCheckProviders"
import type { CheckAvailability } from "../../lib/checkAvailability"
import { ProviderSettingsSession } from "./ProviderSettingsSession"
import { SettingsRow } from "./SettingsSearchRows"

const inputClass =
  "mt-2 min-h-[var(--control-height-regular)] w-full rounded-control border border-separator bg-input-fill px-3 type-body text-label"

export function CheckProviderSettings({
  legacyKeySaved = false,
  usage,
}: {
  control?: string | null | undefined
  targetRevision?: number | undefined
  legacyKeySaved?: boolean
  usage?: CheckAvailability["usage"]
}) {
  const [session] = useState(() => new ProviderSettingsSession())
  const state = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
    session.getSnapshot,
  )
  const [usageOpen, setUsageOpen] = useState(false)
  const current = state.drafts[state.selectedId]
  const connection = current?.draft.connection ?? defaultConnection("jev")
  const savedConnection = state.saved?.profiles[state.selectedId]
  const hasSavedCredential =
    savedConnection?.credential?.kind === "legacy_type_safe"
      ? legacyKeySaved
      : Boolean(savedConnection?.credential)
  const credential = current?.draft.credential ?? ""
  const unavailable = state.loading || !state.saved || state.busy
  const disabled =
    unavailable || !connection.model.trim() || (!credential.trim() && !hasSavedCredential)

  return (
    <SectionGroup title="Decision model">
      <Card>
        <SettingsRow
          searchId="typeSafeApiKey"
          label="Jev connection"
          description="Model-based Checks use Jev to review selected session content."
        >
          <label className="block type-footnote text-label-secondary">
            Model
            <input
              aria-label="Jev model"
              className={inputClass}
              value={connection.model}
              readOnly
              disabled={unavailable}
            />
          </label>
          <label className="mt-3 block type-footnote text-label-secondary">
            API key
            <input
              aria-label="TypeSafe API key"
              type="password"
              autoComplete="off"
              spellCheck={false}
              value={credential}
              disabled={unavailable}
              placeholder={
                hasSavedCredential ? "Saved credential; enter a replacement" : undefined
              }
              onChange={(event) => session.edit(connection, event.target.value || null)}
              className={inputClass}
            />
          </label>
          {hasSavedCredential && (
            <p className="mt-1 type-footnote text-label-secondary">
              Leave empty to use the saved credential, or enter a replacement.
            </p>
          )}
          <div className="mt-3 flex flex-wrap gap-2">
            <PushButton disabled={disabled} onClick={() => void session.run("test")}>
              {state.action === "test" ? "Testing connection…" : "Test connection"}
            </PushButton>
            <PushButton
              variant="primary"
              disabled={disabled}
              onClick={() => void session.run("save")}
            >
              {state.action === "save" ? "Saving connection…" : "Save Jev connection"}
            </PushButton>
            {hasSavedCredential && (
              <PushButton disabled={unavailable} onClick={() => void session.run("remove")}>
                {state.action === "remove" ? "Removing API key…" : "Remove API key"}
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
          {state.loading && (
            <p role="status" className="mt-2 type-footnote text-label-secondary">
              Loading Jev connection…
            </p>
          )}
          {state.status && (
            <p role="status" className="mt-2 type-footnote text-label-secondary">
              {state.status}
            </p>
          )}
          {state.error && (
            <p role="alert" className="mt-2 type-footnote text-system-red-text">
              {state.error}
            </p>
          )}
          {!state.loading && !state.saved && (
            <PushButton onClick={() => void session.refresh()}>
              Retry loading connection
            </PushButton>
          )}
        </SettingsRow>
        {usage && (
          <div className="px-3">
            <Disclosure label="Model usage" open={usageOpen} onOpenChange={setUsageOpen}>
              <p className="type-footnote text-label-secondary">
                Recorded model-based check totals on this device. Clear Local Data resets these
                totals.
              </p>
              <p className="mt-2 type-footnote text-label-secondary">
                {usage.confirmedCalls > 0 || usage.inputTokens > 0 || usage.outputTokens > 0
                  ? `${usage.inputTokens.toLocaleString()} input tokens · ${usage.outputTokens.toLocaleString()} output tokens · ${usage.confirmedCalls.toLocaleString()} confirmed requests`
                  : "No confirmed model requests yet."}
              </p>
              <p className="mt-1 type-footnote text-label-secondary">
                {usage.estimatedUsd === null
                  ? "Cost estimate unavailable."
                  : `${usage.estimatedUsd} estimated cost. This is not a bill.`}
              </p>
              <p className="mt-1 type-footnote text-label-secondary">
                {usage.cacheHits.toLocaleString()} cached results reused.
              </p>
              {usage.unknownOutcomes > 0 && (
                <p className="mt-1 type-footnote text-label-secondary">
                  {usage.unknownOutcomes.toLocaleString()} request outcomes are unknown and are
                  not included in the estimate.
                </p>
              )}
            </Disclosure>
          </div>
        )}
      </Card>
      <p className="mt-3 type-footnote text-label-secondary">
        Model-based Checks send selected activity, arguments, and available user requests to
        Jev, including paths or file content in those fields. They exclude private thinking and
        do not send full transcripts. Provider charges may apply.
      </p>
    </SectionGroup>
  )
}
