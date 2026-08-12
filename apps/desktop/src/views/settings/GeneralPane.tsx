// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { Card } from '../../components/ui/Card';
import { Pane } from '../../components/ui/Pane';
import { RangeSlider } from '../../components/ui/RangeSlider';
import { Row } from '../../components/ui/Row';
import { SectionGroup } from '../../components/ui/SectionGroup';
import { ToggleRow } from '../../components/ui/ToggleRow';
import type { AppSettingsController } from './useAppSettings';

/** Narrowest and widest activity windows, mirroring the store's own clamp. */
const MIN_DAYS = 1;
const MAX_DAYS = 30;

function dayLabel(days: number): string {
  return days === 1 ? '1 day' : `${days} days`;
}

/**
 * General preferences: how much history the popover shows, and whether antiburn
 * should be running at all without being asked.
 */
export function GeneralPane({ settings, update }: AppSettingsController) {
  return (
    <Pane title="General">
      <SectionGroup title="Activity">
        <Card>
          <Row
            label="Show the last"
            description={`The popover lists sessions from the last ${dayLabel(
              settings.activityWindowDays,
            )}. antiburn keeps two weeks of history regardless, so widening this needs no rescan.`}
            trailing={
              <span className="type-body tabular-nums text-label-secondary">
                {dayLabel(settings.activityWindowDays)}
              </span>
            }
          >
            <RangeSlider
              className="mt-2 w-full"
              value={settings.activityWindowDays}
              min={MIN_DAYS}
              max={MAX_DAYS}
              ariaLabel="Days of activity to show"
              ariaValueText={dayLabel(settings.activityWindowDays)}
              onChange={(days) => void update({ activityWindowDays: days })}
            />
          </Row>
        </Card>
      </SectionGroup>

      <SectionGroup title="Startup">
        <Card>
          <ToggleRow
            label="Open antiburn at login"
            // Stated plainly rather than hidden: the preference is recorded, and
            // this build has no login-item registration behind it. Saying so is
            // better than a switch that silently does nothing.
            description="Recorded now and applied the next time login items are registered. This build does not install one yet."
            checked={settings.launchAtLogin}
            onChange={(next) => void update({ launchAtLogin: next })}
          />
        </Card>
      </SectionGroup>
    </Pane>
  );
}
