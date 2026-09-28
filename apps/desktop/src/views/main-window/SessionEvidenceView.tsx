import { useState, useSyncExternalStore } from "react"

import type {
  EvidenceReference,
  SessionEvidenceContextItem,
} from "../../lib/sessionEvidenceIpc"
import { SessionEvidenceSession } from "./SessionEvidenceSession"

import { EVIDENCE_KIND_LABELS } from "../../lib/presentation/sessionEvidence"

function MatchedText({ item }: { item: SessionEvidenceContextItem }) {
  const { matchStart, matchEnd } = item.reference
  const text = Array.from(item.text)
  if (
    matchStart == null ||
    matchEnd == null ||
    !Number.isSafeInteger(matchStart) ||
    !Number.isSafeInteger(matchEnd) ||
    matchStart < 0 ||
    matchEnd <= matchStart ||
    matchEnd > text.length
  )
    return item.text
  return (
    <>
      {text.slice(0, matchStart).join("")}
      <mark className="rounded-control bg-surface-selected text-label">
        {text.slice(matchStart, matchEnd).join("")}
      </mark>
      {text.slice(matchEnd).join("")}
    </>
  )
}

function Passage({
  item,
  matched = false,
}: {
  item: SessionEvidenceContextItem
  matched?: boolean
}) {
  return (
    <section
      aria-label={matched ? "Matched passage" : "Surrounding context"}
      className={matched ? "rounded-control bg-surface-card p-4" : "px-4 py-2"}
    >
      <div className="mb-2 flex flex-wrap items-center gap-2 type-caption text-label-secondary">
        <span>{EVIDENCE_KIND_LABELS[item.kind]}</span>
        {matched && <span className="font-semibold text-label">Matched passage</span>}
        {item.reference.scope === "delegated" && <span>Delegated thread</span>}
        {item.reference.jsonPath != null && (
          <span className="break-all font-mono">
            JSON field {item.reference.jsonPath || "/"}
          </span>
        )}
        {item.truncated && <span>Excerpt · incomplete content</span>}
      </div>
      <p className="whitespace-pre-wrap break-words type-body text-label select-text">
        {matched ? <MatchedText item={item} /> : item.text}
      </p>
      {item.kind === "thinking" && (
        <p className="mt-2 type-caption text-label-secondary">
          Recorded reasoning may include tentative or rejected ideas. It is not a verified
          conclusion.
        </p>
      )}
    </section>
  )
}

export function SessionEvidenceView({ reference }: { reference: EvidenceReference }) {
  const [session] = useState(() => new SessionEvidenceSession(reference))
  const state = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
    session.getSnapshot,
  )
  const context = state.context
  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="type-headline text-label">Recorded evidence</h2>
      </div>
      <p className="type-callout text-label-secondary">
        Retained text from this session. Surrounding context is limited to nearby available
        passages.
      </p>
      {state.phase === "loading" && (
        <p role="status" className="sr-only">
          Reading recorded evidence
        </p>
      )}
      {state.phase === "unavailable" && (
        <p role="status" className="type-body text-label-secondary">
          This passage is no longer available. Search again to find current evidence.
        </p>
      )}
      {state.phase === "error" && (
        <div
          role="status"
          className="flex flex-wrap items-center gap-3 type-body text-label-secondary"
        >
          <p>Could not read this passage.</p>
          <button
            type="button"
            className="rounded-control px-3 py-2 text-label hover:bg-surface-hover"
            onClick={session.retry}
          >
            Retry
          </button>
        </div>
      )}
      {state.phase === "ready" && context?.match && (
        <div className="flex flex-col gap-3">
          {context.previous && <Passage item={context.previous} />}
          <Passage item={context.match} matched />
          {context.next && <Passage item={context.next} />}
        </div>
      )}
    </div>
  )
}
