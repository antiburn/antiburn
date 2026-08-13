// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { Card } from '../../components/ui/Card';
import { Pane } from '../../components/ui/Pane';
import { Row } from '../../components/ui/Row';
import { SectionGroup } from '../../components/ui/SectionGroup';
import { revealSource, type AppInfo } from '../../lib/ipc';
import type { SettingsPane } from '../../lib/settingsPanes';
import { PushButton } from '../../components/ui/PushButton';

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
export function AboutPane({
  info,
  onOpenPane,
}: {
  info: AppInfo | null;
  /** Move the window to another pane; About links to content, not to URLs. */
  onOpenPane?: (pane: SettingsPane) => void;
}) {
  return (
    <Pane title="About">
      <SectionGroup title="Build">
        <Card>
          <Row
            label="antiburn"
            description="Local-first visibility into your AI coding-agent sessions."
            trailing={
              <span className="type-body tabular-nums text-label-secondary">
                {info?.appVersion ?? '—'}
              </span>
            }
          />
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
            // Stated, not linked. The full text ships in the bundle and every
            // source file carries the header, so this is checkable without a
            // browser — which is the point of putting it in an offline app.
            description="antiburn is free software under the Mozilla Public License 2.0. The full text ships with the application, and every source file carries its header."
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
