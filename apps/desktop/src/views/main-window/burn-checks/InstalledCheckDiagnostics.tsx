import { ExternalLink } from "lucide-react"
import { useId, useRef, useState } from "react"

import {
  openInstalledCheckDefinition,
  type InstalledCheckDiagnosticPayload,
  type InstalledCheckDiagnosticSourcePayload,
} from "../../../lib/insightsIpc"
import { pluralize } from "../../../lib/presentation/text"
import { CheckDetailPanel } from "./CheckDetailPanel"

function OpenCheckFile({
  source,
  active,
}: {
  source: InstalledCheckDiagnosticSourcePayload
  active: boolean
}) {
  const opening = useRef(false)
  const [pending, setPending] = useState(false)
  const [error, setError] = useState<string | null>(null)
  return (
    <div className="space-y-2">
      <button
        type="button"
        className="burn-check-action inline-flex items-center gap-1 type-callout"
        disabled={pending || !active}
        aria-busy={pending}
        onClick={async () => {
          if (opening.current || !active) return
          opening.current = true
          setPending(true)
          setError(null)
          try {
            await openInstalledCheckDefinition(source.reference)
          } catch (error) {
            setError(
              typeof error === "string"
                ? error
                : "Could not open the check file. Check that it still exists and that an app can open YAML files, then try again.",
            )
          } finally {
            opening.current = false
            setPending(false)
          }
        }}
      >
        <ExternalLink size={14} className="shrink-0" aria-hidden="true" />
        {pending ? "Opening check file…" : "Open check file"}
      </button>
      {error && (
        <p role="alert" className="type-callout text-label-secondary">
          {error}
        </p>
      )}
    </div>
  )
}

function InstalledCheckDiagnostic({
  diagnostic,
  active,
}: {
  diagnostic: InstalledCheckDiagnosticPayload
  active: boolean
}) {
  const titleId = useId()
  const { source } = diagnostic
  return (
    <section aria-labelledby={titleId}>
      <CheckDetailPanel variant={diagnostic.severity}>
        <div className="space-y-2">
          <h3 id={titleId} className="type-callout font-semibold! text-label">
            {diagnostic.title}
          </h3>
          <p className="whitespace-pre-wrap wrap-anywhere">{diagnostic.message}</p>
          {diagnostic.affectedSessionCount > 0 && (
            <p className="type-caption text-label-tertiary">
              {diagnostic.affectedSessionCount} affected{" "}
              {pluralize(diagnostic.affectedSessionCount, "session", "sessions")}
            </p>
          )}
          {source && (
            <>
              <p className="type-caption text-label-secondary wrap-anywhere">
                {source.path}
                {source.line != null ? ` · Line ${source.line}` : ""}
                {source.column != null ? `, column ${source.column}` : ""}
              </p>
              <OpenCheckFile key={source.reference} source={source} active={active} />
            </>
          )}
          {diagnostic.details.length > 0 && (
            <details>
              <summary className="burn-check-action type-caption">Technical details</summary>
              <dl className="mt-2 space-y-2 type-caption">
                {diagnostic.details.map((detail, index) => (
                  <div key={`${detail.label}:${index}`}>
                    <dt className="text-label-secondary wrap-anywhere">{detail.label}</dt>
                    <dd className="font-mono whitespace-pre-wrap text-label-tertiary wrap-anywhere">
                      {detail.value}
                    </dd>
                  </div>
                ))}
              </dl>
            </details>
          )}
        </div>
      </CheckDetailPanel>
    </section>
  )
}

export function InstalledCheckDiagnostics({
  diagnostics,
  active,
}: {
  diagnostics: readonly InstalledCheckDiagnosticPayload[]
  active: boolean
}) {
  return diagnostics.map((diagnostic) => (
    <InstalledCheckDiagnostic
      key={JSON.stringify([
        diagnostic.code,
        diagnostic.source,
        diagnostic.details,
        diagnostic.title,
        diagnostic.message,
      ])}
      diagnostic={diagnostic}
      active={active}
    />
  ))
}
