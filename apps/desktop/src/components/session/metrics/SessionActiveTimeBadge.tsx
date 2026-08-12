// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { Clock } from 'lucide-react';

import { formatDuration } from '../../../lib/presentation/sessionAnalytics';
import { Tooltip } from '../../presentation/Tooltip';

export interface SessionActiveTimeBadgeProps {
  /** Working time with idle gaps removed — the headline figure. */
  activeSecs: number;
  /**
   * Wall-clock span, shown as the tooltip's "Overall" row. Omit when unknown;
   * the row then disappears rather than showing a figure nobody computed.
   */
  durationSecs?: number | null;
  /** Extra classes for the pill. Omit on a centered flex line. */
  className?: string;
}

/**
 * The active-time pill and its tooltip, driven entirely by the values it is
 * given.
 *
 * "Active" time is the wall-clock span minus idle gaps — the time the session
 * was genuinely being worked, not the time it was open — so it is always at or
 * below the overall duration the tooltip also shows. Leading with the smaller,
 * truer number is the point: a session left open overnight is not an
 * eight-hour session.
 */
export function SessionActiveTimeBadge({
  activeSecs,
  durationSecs,
  className = '',
}: SessionActiveTimeBadgeProps) {
  return (
    <Tooltip
      side="bottom"
      delayMs={150}
      label={
        <div className="space-y-0.5 text-left">
          <div className="flex justify-between gap-4 type-caption font-medium">
            <span>Active time</span>
            <span className="tabular-nums">{formatDuration(activeSecs)}</span>
          </div>
          {durationSecs != null && (
            <div className="flex justify-between gap-4 type-caption text-label-tertiary">
              <span>Overall</span>
              <span className="tabular-nums">{formatDuration(durationSecs)}</span>
            </div>
          )}
          <div className="type-caption text-label-tertiary">Excludes idle gaps</div>
        </div>
      }
    >
      <span
        className={`flex shrink-0 items-center gap-0.5 rounded-full bg-label/[0.06] px-1.5 py-px type-caption font-medium leading-[13px] text-label-secondary tabular-nums${
          className ? ` ${className}` : ''
        }`}
      >
        <Clock size={11} className="shrink-0" aria-hidden="true" />
        {formatDuration(activeSecs)}
      </span>
    </Tooltip>
  );
}
