// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { useEffect, useState } from 'react';

import { Card } from '../../components/ui/Card';
import { Pane } from '../../components/ui/Pane';
import { Row } from '../../components/ui/Row';
import { SectionGroup } from '../../components/ui/SectionGroup';
import { ToggleRow } from '../../components/ui/ToggleRow';
import { getLiveUsage, EMPTY_LIVE_USAGE, type LiveUsageSummaryPayload } from '../../lib/ipc';
import { liveSourceNote } from '../../lib/presentation/liveUsage';
import type { AppSettingsController } from './useAppSettings';

/**
 * Usage: where the plan limits come from, and the one switch that changes it.
 *
 * The pane exists because this feature has two tiers with genuinely different
 * costs, and a reader cannot consent to the second without being told which is
 * which:
 *
 * - **Reading what your agent cached** happens always. It is a file on this
 *   disk, and it needs no permission any more than the session index does.
 * - **Asking your agent to refresh** runs the agent, which goes online. That
 *   is the switch below, and it is off until a reader turns it on.
 *
 * The copy says what turning it on does in both directions — it makes readings
 * current, *and* it lets milestone notifications fire — because one switch
 * with two consequences has to name both or it is not consent.
 */

export type UsagePaneProps = AppSettingsController;

/** What a failed source means, phrased as something a reader could act on. */
function errorNote(category: string): string {
  switch (category) {
    case 'authentication':
      return 'Your agent could not read your plan usage. Sign in again there.';
    case 'rateLimited':
      return 'Your provider asked antiburn to slow down. It will try again later.';
    case 'schema':
      return 'Your agent reported usage in a shape antiburn does not recognise.';
    default:
      return 'antiburn could not reach your agent’s usage. It will try again later.';
  }
}

export function UsagePane({ settings, update }: UsagePaneProps) {
  const [live, setLive] = useState<LiveUsageSummaryPayload>(EMPTY_LIVE_USAGE);

  // One read on open, and one after the switch moves. Not a subscription: this
  // pane is a place a reader visits deliberately, and a limit figure that
  // ticked over while they were looking at a preference would be noise.
  useEffect(() => {
    let active = true;
    void getLiveUsage()
      .then((next) => {
        if (active) setLive(next);
      })
      .catch(() => {
        if (active) setLive(EMPTY_LIVE_USAGE);
      });
    return () => {
      active = false;
    };
  }, [settings?.liveUsageEnabled]);

  const on = settings?.liveUsageEnabled ?? false;

  return (
    <Pane title="Usage">
      <SectionGroup title="Keeping limits current">
        <Card>
          <ToggleRow
            label="Ask my agent to refresh"
            description="Runs your coding agent in the background about every ten minutes to refresh its own usage reading, then reads the file it writes. Your agent goes online to do this — antiburn still opens no connection of its own. Turning this on also lets usage milestone notifications fire, since they need readings that keep moving."
            checked={on}
            onChange={(next) => void update({ liveUsageEnabled: next })}
          />
          <Row
            label="Without this"
            description="antiburn reads whatever usage figure your agent last cached on this machine. Nothing goes online, and every reading on the Usage screen says how old it is."
          />
        </Card>
      </SectionGroup>

      <SectionGroup title="What antiburn can currently see">
        <Card>
          {live.providers.length === 0 && live.errors.length === 0 && (
            <Row
              label="No plan limits found"
              description="No agent on this machine has cached a usage reading yet. Use your agent once and this fills in."
            />
          )}
          {live.providers.map((provider) => (
            <Row
              key={provider.provider}
              label={provider.displayName}
              description={`${provider.sourceLabel}. ${provider.windows.length} limit${
                provider.windows.length === 1 ? '' : 's'
              } reported.`}
              trailing={
                <span className="type-caption tabular-nums text-label-tertiary">
                  {liveSourceNote(provider)}
                </span>
              }
            />
          ))}
          {live.errors.map((error) => (
            <Row
              key={error.source}
              label="Could not read usage"
              description={errorNote(error.category)}
            />
          ))}
        </Card>
      </SectionGroup>
    </Pane>
  );
}
