import { ChevronLeft } from "lucide-react"
import { useCallback } from "react"

import { PaneHeader } from "../../components/ui/Pane"
import { LICENSE_TEXT, NOTICE_TEXT, THIRD_PARTY_NOTICES_TEXT } from "../../lib/legalNotices"

/** The legal documents About can open. */
export type AboutDocumentId = "licence" | "notices" | "attributions"

/** Titles, shared so a row and the view it opens can never drift apart. */
const ABOUT_DOCUMENTS: Record<AboutDocumentId, { title: string }> = {
  licence: { title: "Licence text" },
  notices: { title: "Legal notices" },
  attributions: { title: "Third-party attributions" },
}

/** Render the selected legal file as plain text. */
function DocumentBody({ id }: { id: AboutDocumentId }) {
  if (id === "licence") {
    return (
      <p className="type-footnote whitespace-pre-wrap text-label-secondary">
        {LICENSE_TEXT.trim()}
      </p>
    )
  }
  if (id === "notices") {
    return (
      <p className="type-footnote whitespace-pre-wrap text-label-secondary">
        {NOTICE_TEXT.trim()}
      </p>
    )
  }
  return (
    <p className="type-footnote whitespace-pre-wrap text-label-secondary">
      {THIRD_PARTY_NOTICES_TEXT.trim()}
    </p>
  )
}

/** Show one legal document on the settings scroll surface. */
export function AboutDocumentView({ id, onBack }: { id: AboutDocumentId; onBack: () => void }) {
  // Focus the new heading and reset the shared scroll surface without an effect.
  const headingRef = useCallback((node: HTMLHeadingElement | null) => {
    node?.focus()
  }, [])

  return (
    <>
      <PaneHeader
        headingRef={headingRef}
        title={ABOUT_DOCUMENTS[id].title}
        leading={
          <button
            type="button"
            onClick={onBack}
            aria-label="Back to About"
            className="-ml-1 inline-flex h-7 shrink-0 items-center rounded-control px-1 text-label-secondary transition-colors duration-[var(--duration-fast)] ease-out hover:bg-surface-hover hover:text-label"
          >
            <ChevronLeft size={18} strokeWidth={2} aria-hidden="true" className="shrink-0" />
          </button>
        }
      />
      <DocumentBody id={id} />
    </>
  )
}
