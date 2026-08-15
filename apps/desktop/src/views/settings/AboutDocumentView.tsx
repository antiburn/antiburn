// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { ChevronLeft } from 'lucide-react';
import { useEffect, useRef } from 'react';

import { PaneHeader } from '../../components/ui/Pane';
import { ATTRIBUTIONS, LICENSE_TEXT, NOTICE_TEXT } from '../../lib/legalNotices';

/** The legal documents About can open. */
export type AboutDocumentId = 'licence' | 'notices' | 'attributions';

/** Titles, shared so a row and the view it opens can never drift apart. */
export const ABOUT_DOCUMENTS: Record<AboutDocumentId, { title: string }> = {
  licence: { title: 'Licence text' },
  notices: { title: 'Legal notices' },
  attributions: { title: 'Third-party attributions' },
};

/** Long-form legal prose. Plain text, never anchors — the licence body carries
 *  bare URLs and About's no-external-links rule covers them too. */
function DocumentBody({ id }: { id: AboutDocumentId }) {
  if (id === 'licence') {
    return (
      <p className="type-footnote whitespace-pre-wrap text-label-secondary">
        {LICENSE_TEXT.trim()}
      </p>
    );
  }
  if (id === 'notices') {
    return (
      <p className="type-footnote whitespace-pre-wrap text-label-secondary">
        {NOTICE_TEXT.trim()}
      </p>
    );
  }
  return (
    <div className="flex flex-col gap-3">
      {ATTRIBUTIONS.map((attribution) => (
        <div key={attribution.title}>
          <p className="type-footnote font-medium text-label">{attribution.title}</p>
          <p className="type-footnote text-label-secondary">{attribution.body}</p>
        </div>
      ))}
    </div>
  );
}

/**
 * One legal document, read on its own surface.
 *
 * These were disclosures inside About until the obvious problem with that
 * showed itself: expanding the licence dropped seventeen kilobytes of MPL into
 * the middle of the pane and pushed everything after it past the fold. A
 * document is not an aside, so it gets a view — reached by an `Open` row, left
 * by the back arrow, and mounted only while it is being read.
 *
 * `PaneHeader` is composed directly rather than going through `Pane`: the
 * `leading` slot exists for exactly this arrow, and `Pane` does not forward it.
 * No `ScrollPane` here — the settings window owns one around the whole panel,
 * and a second would nest two scrollers inside one column.
 */
export function AboutDocumentView({ id, onBack }: { id: AboutDocumentId; onBack: () => void }) {
  const heading = useRef<HTMLHeadingElement>(null);

  // Move focus into the view as it takes over the pane. Two jobs at once: a
  // keyboard or screen-reader user lands on the document instead of being left
  // on the button that just vanished, and the shared viewport scrolls back to
  // the top rather than opening a fresh document mid-page.
  useEffect(() => {
    heading.current?.focus();
  }, [id]);

  return (
    <>
      {/* The arrow and the title are two things. Wrapping the heading text
          inside the button would make a screen reader announce the whole title
          as the name of the control that leaves the view, and leave the view
          with no heading at all. */}
      <PaneHeader
        headingRef={heading}
        title={ABOUT_DOCUMENTS[id].title}
        leading={
          <button
            type="button"
            onClick={onBack}
            aria-label="Back to About"
            className="-ml-1 inline-flex h-7 shrink-0 items-center rounded-control px-1 text-label-secondary transition-colors duration-[120ms] ease-out hover:bg-surface-hover hover:text-label"
          >
            <ChevronLeft size={18} strokeWidth={2} aria-hidden="true" className="shrink-0" />
          </button>
        }
      />
      <DocumentBody id={id} />
    </>
  );
}
