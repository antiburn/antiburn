// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { useCallback, useEffect, useState } from 'react';

import { Card } from '../../components/ui/Card';
import { Pane } from '../../components/ui/Pane';
import { PushButton } from '../../components/ui/PushButton';
import { RangeSlider } from '../../components/ui/RangeSlider';
import { Row } from '../../components/ui/Row';
import { SectionGroup } from '../../components/ui/SectionGroup';
import { ToggleRow } from '../../components/ui/ToggleRow';
import {
  cancelScan,
  getScanStatus,
  onScanEvent,
  scanNow,
  type AppInfo,
  type ScanStatus,
} from '../../lib/ipc';
import { relativeTime } from '../../lib/presentation/relativeTime';
import type { AppSettingsController } from './useAppSettings';

/** Narrowest and widest activity windows, mirroring the store's own clamp. */
// Mirrors the shell's MIN/MAX_ACTIVITY_DAYS: the ceiling equals the store's
// two-week retention, so the slider can never promise days that cannot exist.
const MIN_DAYS = 1;
const MAX_DAYS = 14;

function dayLabel(days: number): string {
  return days === 1 ? '1 day' : `${days} days`;
}

/** A byte count at a readable scale. Two significant places is enough for a
 *  settings row: the question being answered is "is this large?". */
export function byteLabel(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 KB';
  const units = ['KB', 'MB', 'GB'];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`;
}

/** What the historical-scan row says about the last (or current) pass. */
export function scanSummary(status: ScanStatus | null): string {
  if (status?.running) return 'Scanning now…';
  if (status?.error) return 'The last scan did not finish.';
  if (status?.cancelled) return 'The last scan was stopped before it finished.';
  if (status?.finishedAt) return `Last scanned ${relativeTime(status.finishedAt)}.`;
  return 'Nothing has been scanned yet.';
}

export interface GeneralPaneProps extends AppSettingsController {
  /** Absent until the shell answers; `null` outside the shell entirely. */
  info: AppInfo | null;
}

/**
 * General preferences: how much history the popover shows, whether antiburn
 * keeps looking on its own, and how much it has accumulated.
 *
 * The monitoring toggle here and the pause control in the popover footer are
 * the same preference. That is deliberate: the footer is where a reader notices
 * background work happening, and Settings is where they go looking for the
 * switch that stops it.
 */
export function GeneralPane({ settings, update, info }: GeneralPaneProps) {
  const [scanStatus, setScanStatus] = useState<ScanStatus | null>(null);

  useEffect(() => {
    let active = true;
    void getScanStatus()
      .then((status) => {
        if (active) setScanStatus(status);
      })
      .catch(() => {});
    const pending = onScanEvent((status) => {
      if (active) setScanStatus(status);
    });
    return () => {
      active = false;
      void pending.then((unlisten) => unlisten());
    };
  }, []);

  const handleScan = useCallback(async () => {
    const status = await scanNow().catch(() => null);
    if (status) setScanStatus(status);
  }, []);

  const handleCancel = useCallback(async () => {
    const status = await cancelScan().catch(() => null);
    if (status) setScanStatus(status);
  }, []);

  const running = scanStatus?.running ?? false;

  return (
    <Pane title="General">
      <SectionGroup title="Activity">
        <Card>
          <Row
            label="Show the last"
            description={`The popover lists sessions from the last ${dayLabel(
              settings.activityWindowDays,
            )}. antiburn keeps two weeks of history; widening this shows it right away and asks the next scan to fill anything missing.`}
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

      <SectionGroup title="Monitoring">
        <Card>
          <ToggleRow
            label="Keep looking for new sessions"
            description="antiburn re-reads your agents' session files while the popover is open. Turning this off stops the background pass — everything already indexed stays readable, and you can still scan on demand."
            checked={!settings.discoveryPaused}
            onChange={(next) => void update({ discoveryPaused: !next })}
          />
          <Row
            label="Historical scan"
            description={`Read every session file antiburn can find on this machine, from the start. ${scanSummary(
              scanStatus,
            )}`}
            trailing={
              running ? (
                <PushButton onClick={() => void handleCancel()}>Stop</PushButton>
              ) : (
                <PushButton onClick={() => void handleScan()}>Scan now</PushButton>
              )
            }
          />
        </Card>
      </SectionGroup>

      <SectionGroup title="Local storage">
        <Card>
          <Row
            label="Indexed sessions"
            description="What antiburn currently has on this machine. Sessions older than two weeks are dropped automatically; your agents' own files are never touched. Settings → Privacy can clear all of it."
            trailing={
              <span className="type-body tabular-nums text-label-secondary">
                {info ? `${info.indexedSessions} · ${byteLabel(info.databaseBytes)}` : '—'}
              </span>
            }
          />
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
