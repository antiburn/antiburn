// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { useEffect, useRef, useState } from 'react';

import { Card } from '../../components/ui/Card';
import { Pane } from '../../components/ui/Pane';
import { Row } from '../../components/ui/Row';
import { SectionGroup } from '../../components/ui/SectionGroup';
import { revealSource, type AppInfo } from '../../lib/ipc';
import { detectPlatform, type Platform } from '../../lib/platform';
import type { SettingsPane } from '../../lib/settingsPanes';
import { PushButton } from '../../components/ui/PushButton';
import { AboutDocumentView, type AboutDocumentId } from './AboutDocumentView';
import { UpdatesSection } from './UpdatesSection';
import type { AppSettingsController } from './useAppSettings';

/** Display names for the masthead; `detectPlatform` reports lowercase ids. */
const PLATFORM_LABELS: Record<Platform, string> = {
  macos: 'macOS',
  windows: 'Windows',
  linux: 'Linux',
  unknown: 'Unknown',
};

/**
 * About: what this build is, where its data lives, and what it is licensed
 * under.
 *
 * The data directory is shown, and revealable, because a local-first app that
 * will not tell you where it keeps your data is asking for trust it has not
 * earned.
 *
 * # Why there are no external links here
 *
 * The matrix's About row lists release-notes, source, security, and support
 * links. They are deliberately absent: this repository is not public yet, so
 * every one of those would be a link that opens a browser at a 404. A dead link
 * in the pane whose job is to establish provenance is worse than an honest
 * silence, so what is here instead is the material that genuinely ships with
 * the app — the licence it is under, and the privacy pane that says what it
 * does with your data. The deferral is recorded in `docs/deviations.md` and the
 * links land with publication.
 *
 * Every one of those destinations is reached the same way: a row with an `Open`
 * button. Three of them push an `AboutDocumentView` over this pane; the fourth
 * moves the window to the Privacy pane, which is a section in its own right.
 */
export interface AboutPaneProps extends AppSettingsController {
  info: AppInfo | null;
  /** Move the window to another pane; About links to content, not to URLs. */
  onOpenPane?: (pane: SettingsPane) => void;
}

/** Ids of the `Open` buttons, so focus can return to the one that was pressed
 *  once its document closes — by then React has unmounted the original node. */
const OPEN_BUTTON_ID: Record<AboutDocumentId, string> = {
  licence: 'about-open-licence',
  notices: 'about-open-notices',
  attributions: 'about-open-attributions',
};

export function AboutPane({ settings, update, loaded, info, onOpenPane }: AboutPaneProps) {
  const [openDocument, setOpenDocument] = useState<AboutDocumentId | null>(null);
  // A ref, not state: which row to hand focus back to is a note to the next
  // effect, and nothing renders differently for it.
  const returnFocusTo = useRef<AboutDocumentId | null>(null);

  // Back from a document returns focus to the row that opened it, so a keyboard
  // reader resumes where they left the card rather than at the top of About.
  useEffect(() => {
    if (openDocument || !returnFocusTo.current) return;
    document.getElementById(OPEN_BUTTON_ID[returnFocusTo.current])?.focus();
    returnFocusTo.current = null;
  }, [openDocument]);

  if (openDocument) {
    return (
      <AboutDocumentView
        id={openDocument}
        onBack={() => {
          returnFocusTo.current = openDocument;
          setOpenDocument(null);
        }}
      />
    );
  }

  return (
    <Pane title="About">
      {/* The app identity sits directly on the window surface rather than in a
          Card: it is a masthead, not a group of settings rows, and card chrome
          would read as an empty container around it. */}
      <div className="flex flex-col items-center gap-3 py-2 text-center">
        <div>
          <p className="type-title-1 text-label">antiburn</p>
          <p className="type-callout text-label-secondary">
            Local-first visibility into your AI coding-agent sessions.
          </p>
        </div>
        <div>
          <p className="type-body tabular-nums text-label">Version {info?.appVersion ?? '—'}</p>
          <p className="type-footnote text-label-secondary">
            {PLATFORM_LABELS[detectPlatform()]}
            {info?.arch ? ` · ${info.arch}` : ''}
          </p>
        </div>
      </div>

      <UpdatesSection settings={settings} update={update} loaded={loaded} info={info} />

      <SectionGroup title="Build">
        <Card>
          <Row
            label="Pricing catalog"
            description="Review date of the bundled price list every cost estimate is computed from. Prices are never fetched."
            trailing={
              <span className="type-body tabular-nums text-label-secondary">
                {info?.pricingCatalogVersion ?? '—'}
              </span>
            }
          />
          <Row
            label="Local database"
            description="Schema version of antiburn's own store. It keeps local session data needed for visibility and analysis; nothing in it is uploaded. See Privacy."
            trailing={
              <span className="type-body tabular-nums text-label-secondary">
                v{info?.schemaVersion ?? '—'}
              </span>
            }
          />
        </Card>
      </SectionGroup>

      {/* Four peer rows, every one of them a door. The two legal documents were
          disclosures here until it became clear what that meant in practice:
          expanding the licence dropped seventeen kilobytes of MPL into the
          middle of the pane and pushed Data past the fold. Each `Open` carries
          its own accessible name — four buttons all called "Open" is a list a
          screen-reader user cannot navigate. */}
      <SectionGroup title="Licence and data handling">
        <Card>
          <Row
            label="Licence"
            // Stated and readable here, never linked. Keeping the full text in
            // the app makes it checkable without a browser — which matters for
            // a local app that a reader may want to check without going
            // online at all.
            description="antiburn is free software under the Mozilla Public License 2.0. The full text is readable here, and every source file carries its header."
            trailing={
              <div className="flex items-center gap-2">
                <span className="type-body tabular-nums text-label-secondary">MPL-2.0</span>
                <PushButton
                  id={OPEN_BUTTON_ID.licence}
                  ariaLabel="Open licence text"
                  onClick={() => setOpenDocument('licence')}
                >
                  Open
                </PushButton>
              </div>
            }
          />
          {onOpenPane && (
            <Row
              label="Privacy and data handling"
              description="What antiburn reads, what it stores, how long it keeps it, and what it does and doesn't send online. The long form lives in Privacy."
              trailing={
                <PushButton
                  ariaLabel="Open privacy and data handling"
                  onClick={() => onOpenPane('privacy')}
                >
                  Open
                </PushButton>
              }
            />
          )}
          <Row
            label="Legal notices"
            description="Who holds the copyright in antiburn and where the work came from."
            trailing={
              <PushButton
                id={OPEN_BUTTON_ID.notices}
                ariaLabel="Open legal notices"
                onClick={() => setOpenDocument('notices')}
              >
                Open
              </PushButton>
            }
          />
          <Row
            label="Third-party attributions"
            description="The third-party material bundled with the app, and the terms it is used under."
            trailing={
              <PushButton
                id={OPEN_BUTTON_ID.attributions}
                ariaLabel="Open third-party attributions"
                onClick={() => setOpenDocument('attributions')}
              >
                Open
              </PushButton>
            }
          />
        </Card>
      </SectionGroup>

      <SectionGroup title="Data">
        <Card>
          <Row
            label="Data folder"
            description={info?.dataDir ?? 'Unavailable outside the antiburn app.'}
            trailing={
              <PushButton
                onClick={() => {
                  if (info?.dataDir) void revealSource(info.dataDir);
                }}
                disabled={!info?.dataDir}
              >
                Reveal
              </PushButton>
            }
          />
        </Card>
      </SectionGroup>
    </Pane>
  );
}
