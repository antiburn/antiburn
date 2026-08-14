// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { Card } from '../../components/ui/Card';
import { Disclosure, DisclosureGroup } from '../../components/ui/Disclosure';
import { Pane } from '../../components/ui/Pane';
import { Row } from '../../components/ui/Row';
import { SectionGroup } from '../../components/ui/SectionGroup';
import { ATTRIBUTIONS, LICENSE_TEXT, NOTICE_TEXT } from '../../lib/legalNotices';
import { revealSource, type AppInfo } from '../../lib/ipc';
import { detectPlatform, type Platform } from '../../lib/platform';
import type { SettingsPane } from '../../lib/settingsPanes';
import { PushButton } from '../../components/ui/PushButton';
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
 */
export interface AboutPaneProps extends AppSettingsController {
  info: AppInfo | null;
  /** Move the window to another pane; About links to content, not to URLs. */
  onOpenPane?: (pane: SettingsPane) => void;
}

export function AboutPane({ settings, update, loaded, info, onOpenPane }: AboutPaneProps) {
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
            // Narrower than "never transcript content", because that was not
            // true: the store keeps a session's title and each skill's
            // one-line description, both capped. Privacy settings carry the
            // long form; this says enough to not mislead.
            description="Schema version of antiburn's own store. It holds derived analysis, plus short capped excerpts for titles and skill descriptions — no message text, tool arguments, or file contents. See Privacy."
            trailing={
              <span className="type-body tabular-nums text-label-secondary">
                v{info?.schemaVersion ?? '—'}
              </span>
            }
          />
        </Card>
      </SectionGroup>

      <SectionGroup title="Licence and data handling">
        <Card>
          <Row
            label="Licence"
            // Stated and readable here, never linked. Keeping the full text in
            // the pane makes it checkable without a browser — which is the
            // point of putting it in an offline app.
            description="antiburn is free software under the Mozilla Public License 2.0. The full text is readable below, and every source file carries its header."
            trailing={
              <span className="type-body tabular-nums text-label-secondary">MPL-2.0</span>
            }
          />
          {onOpenPane && (
            <Row
              label="Privacy and data handling"
              description="What antiburn reads, what it stores, how long it keeps it, and the one time it uses the network. The long form lives in Privacy."
              trailing={<PushButton onClick={() => onOpenPane('privacy')}>Open</PushButton>}
            />
          )}
        </Card>
        {/* Disclosures rather than rows: legal text is explanatory prose, and
            the bodies stay unmounted while collapsed so two long documents do
            not sit in the accessibility tree of every About visit. Plain text
            only — the pane's no-external-links rule applies to the licence's
            own URLs too, so nothing here is an anchor. */}
        <DisclosureGroup>
          <Disclosure label="Legal notices">
            <div className="flex flex-col gap-3">
              <p className="type-footnote whitespace-pre-wrap text-label-secondary">
                {NOTICE_TEXT.trim()}
              </p>
              {ATTRIBUTIONS.map((attribution) => (
                <div key={attribution.title}>
                  <p className="type-footnote font-medium text-label">{attribution.title}</p>
                  <p className="type-footnote text-label-secondary">{attribution.body}</p>
                </div>
              ))}
            </div>
          </Disclosure>
          <Disclosure label="Licence text">
            <p className="type-footnote whitespace-pre-wrap text-label-secondary">
              {LICENSE_TEXT.trim()}
            </p>
          </Disclosure>
        </DisclosureGroup>
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
