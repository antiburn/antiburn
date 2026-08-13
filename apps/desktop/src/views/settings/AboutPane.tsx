// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { Card } from '../../components/ui/Card';
import { Pane } from '../../components/ui/Pane';
import { Row } from '../../components/ui/Row';
import { SectionGroup } from '../../components/ui/SectionGroup';
import { revealSource, type AppInfo } from '../../lib/ipc';
import { PushButton } from '../../components/ui/PushButton';

/**
 * About: what this build is, and where its data lives.
 *
 * The data directory is shown, and revealable, because a local-first app that
 * will not tell you where it keeps your data is asking for trust it has not
 * earned.
 */
export function AboutPane({ info }: { info: AppInfo | null }) {
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
