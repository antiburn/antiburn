import type { BurnCheckEvidenceItemPayload } from "../../../lib/insightsIpc"
import { EvidenceExcerpt } from "./BurnCheckTargetDetail"

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value)
}

function EvidenceValue({ value }: { value: unknown }) {
  if (value === undefined)
    return <p className="type-callout text-label-tertiary">Unavailable</p>
  if (Array.isArray(value) && value.length > 0)
    return (
      <ol className="space-y-2">
        {value.map((part, index) => (
          <li key={index}>
            <EvidenceValue value={part} />
          </li>
        ))}
      </ol>
    )
  if (isRecord(value) && Object.keys(value).length > 0)
    return (
      <dl className="space-y-2">
        {Object.entries(value).map(([key, part]) => (
          <div key={key} className="min-w-0">
            <dt className="type-caption text-label-secondary wrap-anywhere">{key}</dt>
            <dd>
              <EvidenceValue value={part} />
            </dd>
          </div>
        ))}
      </dl>
    )
  return (
    <pre className="whitespace-pre-wrap wrap-anywhere type-callout text-label">
      {typeof value === "string" ? value : JSON.stringify(value)}
    </pre>
  )
}

export function InstalledCheckEvidence({ item }: { item: BurnCheckEvidenceItemPayload }) {
  let value: unknown
  try {
    value = JSON.parse(item.excerpt)
  } catch {
    return <EvidenceExcerpt item={item} showMetadata={false} collapseLong />
  }
  const assessedInput = item.reference === "assessed-input"
  const inputOmissions = item.reference === "input-omissions"
  const structuredInput = assessedInput || inputOmissions
  const tool = isRecord(value) && isRecord(value.tool) ? value.tool : null
  const result = tool && isRecord(tool.result) ? tool.result : null
  const request = isRecord(value) && typeof value.text === "string" ? value.text : null
  const quality =
    !structuredInput && isRecord(value) && isRecord(value.quality) ? value.quality : null
  return (
    <div className="min-w-0 space-y-2">
      {structuredInput ? (
        <EvidenceValue value={value} />
      ) : tool ? (
        <>
          <p className="type-callout text-label wrap-anywhere">
            {typeof tool.name === "string" ? tool.name : "Action unavailable"}
            {typeof result?.status === "string" ? ` · ${result.status}` : ""}
          </p>
          <section
            aria-label="Action arguments"
            className="rounded-control bg-surface-card px-3 py-2"
          >
            <h4 className="type-caption text-label-secondary">Arguments</h4>
            <EvidenceValue value={tool.arguments} />
          </section>
          <section
            aria-label="Action result"
            className="rounded-control bg-surface-card px-3 py-2"
          >
            <h4 className="type-caption text-label-secondary">Result</h4>
            {typeof result?.content === "string" ? (
              <EvidenceExcerpt
                item={{ ...item, label: "context", excerpt: result.content }}
                showMetadata={false}
                collapseLong
              />
            ) : (
              <EvidenceValue value={result?.content} />
            )}
          </section>
        </>
      ) : request !== null ? (
        <EvidenceExcerpt
          item={{ ...item, excerpt: request }}
          showMetadata={false}
          collapseLong
        />
      ) : (
        <p className="type-callout text-label-secondary">
          Readable source details are unavailable.
        </p>
      )}
      {quality?.source_complete === false && (
        <p className="type-caption text-label-secondary">Source details are partial.</p>
      )}
      {quality?.request_history_complete === false && (
        <p className="type-caption text-label-secondary">
          Earlier user requests may be missing.
        </p>
      )}
      <details>
        <summary className="burn-check-action type-caption">
          {assessedInput
            ? "Raw assessed input"
            : inputOmissions
              ? "Raw input omissions"
              : "Raw source details"}
        </summary>
        <pre className="mt-2 whitespace-pre-wrap wrap-anywhere type-caption text-label-secondary">
          {JSON.stringify(value, null, 2)}
        </pre>
      </details>
      {item.explanation && (
        <p className="type-caption text-label-secondary">{item.explanation}</p>
      )}
      {item.limitation && (
        <p className="type-caption text-label-secondary">{item.limitation}</p>
      )}
    </div>
  )
}
