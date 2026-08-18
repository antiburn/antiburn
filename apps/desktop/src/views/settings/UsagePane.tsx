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
 * Plan limits are never read without asking first — there is nothing cached
 * on this machine for antiburn to read on its own. The switch below is what
 * turns that reading on: with it on, antiburn asks each provider directly for
 * your current usage, using the credentials your own coding tools already
 * hold, entirely over your own connection. With it off, this pane has nothing
 * to show.
 *
 * The copy says what turning it on does in both directions — it makes
 * readings possible at all, *and* it lets milestone notifications fire —
 * because one switch with two consequences has to name both or it is not
 * consent.
 */

export type UsagePaneProps = AppSettingsController;

/** What a failed source means, phrased as something a reader could act on. */
function errorNote(category: string): string {
  switch (category) {
    case 'authentication':
      return 'antiburn could not sign in to read your plan usage. Sign in again with your coding tool, then reopen this view.';
    case 'rateLimited':
      return 'Your provider asked antiburn to slow down. It will try again later.';
    case 'schema':
      return 'Your provider reported usage in a shape antiburn does not recognise.';
    default:
      return 'antiburn could not reach your provider for usage. It will try again later.';
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
            label="Keep my plan limits current"
            description="Asks each provider directly for your current usage, about every ten minutes, using the credentials your own coding tools already have — that's your own connection, made as you; no antiburn server is involved. When a provider can't be reached directly, antiburn falls back to asking your coding tool's own local process the same question. Turning this on also lets usage milestone notifications fire, since they need readings that keep moving."
            checked={on}
            onChange={(next) => void update({ liveUsageEnabled: next })}
          />
          <Row
            label="Without this"
            description="antiburn shows no plan limits at all — there is nothing cached here for it to read on its own, and it makes no request for you until you turn this on."
          />
        </Card>
      </SectionGroup>

      <SectionGroup title="What antiburn can currently see">
        <Card>
          {live.providers.length === 0 && live.errors.length === 0 && (
            <Row
              label="No plan limits found"
              description={
                on
                  ? 'No provider credentials were found on this machine yet. Sign in with a coding tool and this fills in.'
                  : 'Turn the switch above on to ask your providers for current plan limits.'
              }
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
